//! Compliance-report export/verify (deck slide 08) — the mockup's
//! "requests audited / forced-to-local count / budget-cap rejections /
//! hash-chain integrity verified / sha256 / signed by fleet key #003",
//! built for real against `audit::AuditEntry` and signed with the
//! (locally generated — see `signing.rs`) Ed25519 key.

use crate::audit::{self, ChainStatus};
use crate::policy::AuditConfig;
use crate::signing::{self, encode_hex};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The signed artifact. Every field here (except `signature` itself) is
/// exactly what gets hashed and signed — see `signable_bytes` — so
/// tampering with any one of them after export invalidates the
/// signature. `signature`/`signed_by_key_id` are metadata *about* the
/// signing, not inputs to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComplianceReport {
    pub from_timestamp: u64,
    pub to_timestamp: u64,
    pub requests_audited: usize,
    pub cost_by_provider: BTreeMap<String, f64>,
    pub total_cost: f64,
    pub sensitivity_forced_local_count: usize,
    pub budget_cap_rejections: usize,
    pub hash_chain_verified: bool,
    /// Entries predating hash-chaining, if any fall inside the exported
    /// range — informational (see `audit::ChainStatus::Verified`), not a
    /// failure.
    pub legacy_entry_count: usize,
    /// SHA-256 (hex) of the last raw log line in `[from_timestamp,
    /// to_timestamp]`, via the exact same `hash_line` the chain itself
    /// uses. `None` if nothing fell inside the range.
    pub terminal_hash: Option<String>,
    pub signed_by_key_id: String,
    /// Hex-encoded Ed25519 signature over `signable_bytes(&self)`.
    pub signature: String,
}

#[derive(Debug, PartialEq)]
pub enum ExportError {
    /// The chain is broken somewhere in the *whole* log (not just the
    /// requested range) — signing a summary derived from a tampered log
    /// would just launder the tampering behind a valid signature, so
    /// this refuses outright rather than exporting anyway with a
    /// `hash_chain_verified: false` flag someone could miss.
    ChainBroken { at_line: usize, reason: String },
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::ChainBroken { at_line, reason } => write!(
                f,
                "refusing to export — audit log's hash chain is broken at line {at_line}: {reason}"
            ),
        }
    }
}

impl std::error::Error for ExportError {}

/// Every field that goes into the signature — a private mirror of
/// `ComplianceReport` minus `signed_by_key_id`/`signature` themselves
/// (metadata *about* the signature can't also be an input to it).
/// `cost_by_provider` as `BTreeMap` (not `HashMap`) is load-bearing: it's
/// what makes this serialize to the same bytes every time regardless of
/// insertion order, both at export and again at verify.
#[derive(Serialize)]
struct SignablePayload<'a> {
    from_timestamp: u64,
    to_timestamp: u64,
    requests_audited: usize,
    cost_by_provider: &'a BTreeMap<String, f64>,
    total_cost: f64,
    sensitivity_forced_local_count: usize,
    budget_cap_rejections: usize,
    hash_chain_verified: bool,
    legacy_entry_count: usize,
    terminal_hash: &'a Option<String>,
}

fn signable_bytes(report: &ComplianceReport) -> Vec<u8> {
    let payload = SignablePayload {
        from_timestamp: report.from_timestamp,
        to_timestamp: report.to_timestamp,
        requests_audited: report.requests_audited,
        cost_by_provider: &report.cost_by_provider,
        total_cost: report.total_cost,
        sensitivity_forced_local_count: report.sensitivity_forced_local_count,
        budget_cap_rejections: report.budget_cap_rejections,
        hash_chain_verified: report.hash_chain_verified,
        legacy_entry_count: report.legacy_entry_count,
        terminal_hash: &report.terminal_hash,
    };
    // `to_vec` on a struct with fixed field order + a BTreeMap is
    // deterministic — no separate "canonical JSON" step needed.
    serde_json::to_vec(&payload).expect("SignablePayload has no non-serializable fields")
}

/// Builds and signs a report over every entry in `[from_timestamp,
/// to_timestamp]` (inclusive). Reuses `audit::verify_chain` as-is rather
/// than reimplementing chain verification for just the range — chain
/// integrity is a whole-log property (each entry's `prev_hash` refers to
/// the actual preceding line in the file, range or not), so the whole
/// log is what gets verified; the range only narrows the summary and the
/// terminal hash.
pub fn export(
    config: &AuditConfig,
    from_timestamp: u64,
    to_timestamp: u64,
    signing_key: &SigningKey,
) -> Result<ComplianceReport, ExportError> {
    let (hash_chain_verified, legacy_entry_count) = match audit::verify_chain(config) {
        ChainStatus::Broken { at_line, reason } => {
            return Err(ExportError::ChainBroken { at_line, reason })
        }
        ChainStatus::Verified { legacy_count, .. } => (true, legacy_count),
        ChainStatus::Empty => (true, 0),
    };

    let entries = audit::read_entries_with_raw_lines(config);
    let in_range: Vec<&(String, audit::AuditEntry)> = entries
        .iter()
        .filter(|(_, e)| e.timestamp >= from_timestamp && e.timestamp <= to_timestamp)
        .collect();

    let requests_audited = in_range.len();
    let mut cost_by_provider: BTreeMap<String, f64> = BTreeMap::new();
    let mut total_cost = 0.0;
    let mut sensitivity_forced_local_count = 0;
    let mut budget_cap_rejections = 0;

    for (_, entry) in &in_range {
        if entry.budget_rejected {
            budget_cap_rejections += 1;
        } else {
            // A rejection never spent anything (see `audit::log_rejection`)
            // and isn't attributable to a real provider call — excluded
            // from cost-by-provider so "n/a": 0.0 doesn't show up as if
            // it were a real provider.
            *cost_by_provider
                .entry(entry.provider.clone())
                .or_insert(0.0) += entry.cost_usd;
            total_cost += entry.cost_usd;
        }
        if entry.sensitivity_forced_local {
            sensitivity_forced_local_count += 1;
        }
    }

    // Last raw line whose entry falls in range, in file (chronological
    // append) order — hashed with the exact function the chain itself
    // uses, not a reimplementation.
    let terminal_hash = in_range
        .last()
        .map(|(raw_line, _)| audit::hash_line(raw_line));

    let key_id = signing::key_id(&signing_key.verifying_key());

    let mut report = ComplianceReport {
        from_timestamp,
        to_timestamp,
        requests_audited,
        cost_by_provider,
        total_cost,
        sensitivity_forced_local_count,
        budget_cap_rejections,
        hash_chain_verified,
        legacy_entry_count,
        terminal_hash,
        signed_by_key_id: key_id,
        signature: String::new(),
    };
    let signature = signing::sign(signing_key, &signable_bytes(&report));
    report.signature = encode_hex(&signature);
    Ok(report)
}

#[derive(Debug, PartialEq)]
pub enum VerifyError {
    /// `signature` isn't 64 bytes of valid hex — the report was hand-
    /// edited or corrupted somewhere other than a signed field.
    MalformedSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MalformedSignature => {
                write!(f, "report's signature field isn't valid 64-byte hex")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// Re-derives `signable_bytes` from `report`'s own fields (never from a
/// stored "here's what was signed" blob — that itself could be tampered)
/// and checks it against `report.signature` using `verifying_key`. A
/// report that round-trips through `export` then straight to `verify`
/// with no edits always returns `Ok(true)`; changing *any* signed field
/// — including ones that look purely cosmetic — flips it to `Ok(false)`.
pub fn verify(
    report: &ComplianceReport,
    verifying_key: &VerifyingKey,
) -> Result<bool, VerifyError> {
    let signature =
        signing::decode_hex_64(&report.signature).ok_or(VerifyError::MalformedSignature)?;
    Ok(signing::verify(
        verifying_key,
        &signable_bytes(report),
        &signature,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::load_or_generate_key_pair;

    fn temp_config(name: &str) -> AuditConfig {
        let path = std::env::temp_dir().join(format!(
            "buzz-compliance-{name}-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        }
    }

    fn temp_key_paths(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir();
        let key = dir.join(format!(
            "buzz-compliance-key-{name}-{:?}",
            std::thread::current().id()
        ));
        let pubkey = dir.join(format!(
            "buzz-compliance-pub-{name}-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&key);
        let _ = std::fs::remove_file(&pubkey);
        (key, pubkey)
    }

    #[test]
    fn export_summarizes_the_range_and_signs_it_verifiably() {
        let config = temp_config("summarize");
        let (key_path, pubkey_path) = temp_key_paths("summarize");
        let signing_key = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();

        audit::log_route(
            &config,
            "alice",
            "req-1",
            "local",
            "simple",
            &[],
            10,
            10,
            0.0,
            false,
        );
        audit::log_route(
            &config,
            "bob",
            "req-2",
            "groq",
            "complex",
            &[],
            100,
            100,
            0.25,
            false,
        );
        audit::log_route(
            &config,
            "alice",
            "req-3",
            "local",
            "sensitive",
            &[],
            10,
            10,
            0.0,
            true,
        );
        audit::log_rejection(&config, "carol", "req-4", "n/a", "budget cap exceeded");

        let report = export(&config, 0, audit::now_unix() + 1, &signing_key).unwrap();
        assert_eq!(report.requests_audited, 4);
        assert_eq!(report.sensitivity_forced_local_count, 1);
        assert_eq!(report.budget_cap_rejections, 1);
        assert!((report.total_cost - 0.25).abs() < 1e-9);
        assert_eq!(report.cost_by_provider.get("groq"), Some(&0.25));
        assert!(!report.cost_by_provider.contains_key("n/a"));
        assert!(report.hash_chain_verified);
        assert!(report.terminal_hash.is_some());

        let verifying_key = signing_key.verifying_key();
        assert_eq!(verify(&report, &verifying_key), Ok(true));

        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }

    #[test]
    fn tampering_with_the_exported_report_fails_verification() {
        let config = temp_config("tamper");
        let (key_path, pubkey_path) = temp_key_paths("tamper");
        let signing_key = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();

        audit::log_route(
            &config,
            "alice",
            "req-1",
            "groq",
            "complex",
            &[],
            100,
            100,
            0.50,
            false,
        );

        let mut report = export(&config, 0, audit::now_unix() + 1, &signing_key).unwrap();
        let verifying_key = signing_key.verifying_key();
        assert_eq!(verify(&report, &verifying_key), Ok(true));

        // Tamper with a single summary field after the fact — exactly
        // what slide 08's "tamper-evident" claim needs to actually hold
        // against, not just an untouched happy path.
        report.total_cost = 0.0;
        assert_eq!(verify(&report, &verifying_key), Ok(false));

        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }

    #[test]
    fn export_refuses_to_sign_a_broken_chain() {
        let config = temp_config("broken");
        let (key_path, pubkey_path) = temp_key_paths("broken");
        let signing_key = load_or_generate_key_pair(&key_path, &pubkey_path).unwrap();

        audit::log_route(
            &config,
            "alice",
            "req-1",
            "local",
            "one",
            &[],
            1,
            1,
            0.0,
            false,
        );
        audit::log_route(
            &config,
            "alice",
            "req-2",
            "local",
            "two",
            &[],
            1,
            1,
            0.0,
            false,
        );

        let home = std::env::var("HOME").unwrap_or_default();
        let path = audit::expand_tilde(&config.log_path, &home);
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        lines[0] = lines[0].replace("\"one\"", "\"one - tampered\"");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let result = export(&config, 0, audit::now_unix() + 1, &signing_key);
        assert!(matches!(result, Err(ExportError::ChainBroken { .. })));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pubkey_path);
    }
}
