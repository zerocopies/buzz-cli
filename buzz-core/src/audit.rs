use sha2::{Digest, Sha256};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::policy::AuditConfig;

/// Marks the hash-chain's starting point — the value used as `prev_hash`
/// for the very first chained entry ever written to a given log.
const GENESIS: &str = "genesis";

/// One parsed line from the audit log.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub provider: String,
    pub reason: String,
    #[serde(default)]
    pub privacy_flags: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// SHA-256 (hex) of the exact previous line in the log, or "genesis"
    /// for the first chained entry. Empty on entries written before hash
    /// chaining existed — those are informational-only, not verifiable.
    #[serde(default)]
    pub prev_hash: String,
}

fn hash_line(line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(line.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Best-effort local audit trail: proof (a session log you can grep) rather
/// than just an assertion that sensitive prompts stayed local. Never logs
/// raw prompt text — only the provider chosen, the routing reason, and the
/// privacy flag *descriptions* already produced by `core::privacy::analyze_privacy`
/// (e.g. "Email address detected"), which name a category without repeating
/// the matched secret.
pub fn log_route(
    config: &AuditConfig,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
) {
    if !config.enabled {
        return;
    }
    if let Err(e) = append_entry(
        config,
        provider,
        reason,
        privacy_flags,
        input_tokens,
        output_tokens,
        cost,
    ) {
        eprintln!(
            "[buzz] warning: could not write audit log ({}): {}",
            config.log_path, e
        );
    }
}

fn append_entry(
    config: &AuditConfig,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
) -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Chain onto whatever the current last line actually is — including a
    // pre-hash-chaining legacy line, if that's what the log currently ends
    // with. That binds the transition point itself into the chain instead
    // of silently starting a fresh, disconnected chain partway through an
    // existing log.
    let prev_hash = last_line(&path)
        .map(|l| hash_line(&l))
        .unwrap_or_else(|| GENESIS.to_string());

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entry = serde_json::json!({
        "timestamp": timestamp,
        "provider": provider,
        "reason": reason,
        "privacy_flags": privacy_flags,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cost_usd": cost,
        "prev_hash": prev_hash,
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", entry)?;
    Ok(())
}

/// Every parseable entry in the audit log. Lines that fail to parse (e.g.
/// hand-edited or from an older schema) are silently skipped rather than
/// failing the whole read — this is a best-effort log, not a database.
pub fn read_entries(config: &AuditConfig) -> Vec<AuditEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
        .collect()
}

/// Aggregate view over the whole audit log — the data behind "prove it
/// stayed local" instead of just asserting it.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditSummary {
    pub total_requests: usize,
    pub local_count: usize,
    pub cloud_count: usize,
    pub sensitive_count: usize,
    pub total_cost: f64,
    pub earliest_timestamp: Option<u64>,
    pub latest_timestamp: Option<u64>,
}

pub fn summarize(config: &AuditConfig) -> AuditSummary {
    let entries = read_entries(config);
    let total_requests = entries.len();
    let local_count = entries.iter().filter(|e| e.provider == "local").count();
    let sensitive_count = entries
        .iter()
        .filter(|e| !e.privacy_flags.is_empty())
        .count();
    let total_cost = entries.iter().map(|e| e.cost_usd).sum();
    let earliest_timestamp = entries.iter().map(|e| e.timestamp).min();
    let latest_timestamp = entries.iter().map(|e| e.timestamp).max();
    AuditSummary {
        total_requests,
        local_count,
        cloud_count: total_requests - local_count,
        sensitive_count,
        total_cost,
        earliest_timestamp,
        latest_timestamp,
    }
}

/// The most recent `n` entries, newest first — for a quick "what actually
/// happened" glance without opening the raw JSONL file.
pub fn recent(config: &AuditConfig, n: usize) -> Vec<AuditEntry> {
    let mut entries = read_entries(config);
    entries.reverse();
    entries.truncate(n);
    entries
}

/// Coarse "N ago" phrasing for a unix timestamp, dependency-free (no
/// chrono) — good enough for a quick audit glance, not a precise clock.
pub fn relative_time(now: u64, timestamp: u64) -> String {
    let elapsed = now.saturating_sub(timestamp);
    if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86400)
    }
}

/// Total cost logged since the start of the current UTC day. Used to
/// enforce `cost.daily_budget_usd` — deliberately a UTC boundary rather
/// than the user's local calendar day, to avoid pulling in a timezone
/// dependency for what's fundamentally a soft spending guard.
pub fn spend_today(config: &AuditConfig) -> f64 {
    let start_of_day = start_of_today_unix();
    read_entries(config)
        .iter()
        .filter(|e| e.timestamp >= start_of_day)
        .map(|e| e.cost_usd)
        .sum()
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn start_of_today_unix() -> u64 {
    let now = now_unix();
    now - (now % 86400)
}

fn expand_tilde(path: &str, home: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        std::path::PathBuf::from(home).join(rest)
    } else {
        std::path::PathBuf::from(path)
    }
}

fn last_line(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().last().map(|l| l.to_string())
}

/// Result of walking the audit log's hash chain.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainStatus {
    /// No entries in the log at all.
    Empty,
    /// Every chained entry's stored hash matches the actual content of the
    /// line before it — nothing in the chained portion of the log has been
    /// edited, reordered, or removed since being written. `legacy_count`
    /// entries predate hash-chaining and aren't covered by this guarantee.
    Verified {
        chained_count: usize,
        legacy_count: usize,
    },
    /// A chained entry's stored `prev_hash` doesn't match the actual hash
    /// of the line before it — something in the log changed after being
    /// written (edited, reordered, or a line was deleted).
    Broken { at_line: usize, reason: String },
}

/// Walks the whole audit log verifying every chained entry's `prev_hash`
/// against the actual content of the line before it. This proves the log
/// wasn't altered *after being written on this machine* — it's local
/// tamper-evidence, not a cryptographic signature an external party could
/// use to verify authenticity (that would need key management and is a
/// meaningfully bigger undertaking left for later).
pub fn verify_chain(config: &AuditConfig) -> ChainStatus {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return ChainStatus::Empty;
    };
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return ChainStatus::Empty;
    }

    let mut chained_count = 0;
    let mut legacy_count = 0;
    let mut expected_prev = GENESIS.to_string();

    for (i, line) in lines.iter().enumerate() {
        let Ok(entry) = serde_json::from_str::<AuditEntry>(line) else {
            return ChainStatus::Broken {
                at_line: i + 1,
                reason: "unparseable entry".to_string(),
            };
        };
        if entry.prev_hash.is_empty() {
            legacy_count += 1;
        } else {
            if entry.prev_hash != expected_prev {
                return ChainStatus::Broken {
                    at_line: i + 1,
                    reason: format!(
                        "stored prev_hash doesn't match the actual preceding line (expected {}, found {})",
                        &expected_prev[..expected_prev.len().min(12)],
                        &entry.prev_hash[..entry.prev_hash.len().min(12)]
                    ),
                };
            }
            chained_count += 1;
        }
        // Always advance, whether this line was chained or legacy — a
        // legacy line can still be the prev-line a later chained entry
        // hashes against, since append_entry chains onto whatever the
        // actual last line is, not just the last *chained* one.
        expected_prev = hash_line(line);
    }

    ChainStatus::Verified {
        chained_count,
        legacy_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> AuditConfig {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-{name}-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        }
    }

    #[test]
    fn empty_log_has_empty_chain_status() {
        let config = temp_config("chain-empty");
        assert_eq!(verify_chain(&config), ChainStatus::Empty);
    }

    #[test]
    fn fresh_log_chains_and_verifies_cleanly() {
        let config = temp_config("chain-fresh");
        log_route(&config, "local", "one", &[], 1, 1, 0.0);
        log_route(&config, "groq", "two", &[], 1, 1, 0.1);
        log_route(&config, "local", "three", &[], 1, 1, 0.0);

        assert_eq!(
            verify_chain(&config),
            ChainStatus::Verified {
                chained_count: 3,
                legacy_count: 0
            }
        );

        let _ = std::fs::remove_file(expand_tilde(
            &config.log_path,
            &std::env::var("HOME").unwrap_or_default(),
        ));
    }

    #[test]
    fn detects_tampering_with_a_middle_entry() {
        let config = temp_config("chain-tamper");
        log_route(&config, "local", "one", &[], 1, 1, 0.0);
        log_route(&config, "local", "two", &[], 1, 1, 0.0);
        log_route(&config, "local", "three", &[], 1, 1, 0.0);

        let home = std::env::var("HOME").unwrap_or_default();
        let path = expand_tilde(&config.log_path, &home);
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        // Tamper with the middle entry's reason after the fact, exactly
        // the attack this feature exists to catch.
        lines[1] = lines[1].replace("\"two\"", "\"two - tampered\"");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        match verify_chain(&config) {
            ChainStatus::Broken { at_line, .. } => assert_eq!(at_line, 3),
            other => panic!("expected Broken, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_entries_without_prev_hash_are_informational_not_broken() {
        let config = temp_config("chain-legacy");
        let home = std::env::var("HOME").unwrap_or_default();
        let path = expand_tilde(&config.log_path, &home);
        // Simulate an existing log written before hash-chaining existed.
        let legacy = serde_json::json!({
            "timestamp": now_unix(), "provider": "local", "reason": "pre-chain entry",
            "privacy_flags": [], "input_tokens": 1, "output_tokens": 1, "cost_usd": 0.0
        });
        std::fs::write(&path, format!("{legacy}\n")).unwrap();

        // A new, chained entry gets appended on top of the legacy one.
        log_route(&config, "local", "post-chain entry", &[], 1, 1, 0.0);

        assert_eq!(
            verify_chain(&config),
            ChainStatus::Verified {
                chained_count: 1,
                legacy_count: 1
            }
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn summarize_counts_local_cloud_and_sensitive() {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-summary-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        log_route(&config, "local", "simple", &[], 10, 10, 0.0);
        log_route(
            &config,
            "local",
            "sensitive",
            &["SSN pattern detected".to_string()],
            10,
            10,
            0.0,
        );
        log_route(&config, "groq", "complex", &[], 100, 100, 0.25);

        let summary = summarize(&config);
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.local_count, 2);
        assert_eq!(summary.cloud_count, 1);
        assert_eq!(summary.sensitive_count, 1);
        assert!((summary.total_cost - 0.25).abs() < 1e-9);
        assert!(summary.earliest_timestamp.is_some());
        assert!(summary.latest_timestamp.is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recent_returns_newest_first_and_respects_limit() {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-recent-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        log_route(&config, "local", "first", &[], 1, 1, 0.0);
        log_route(&config, "local", "second", &[], 1, 1, 0.0);
        log_route(&config, "local", "third", &[], 1, 1, 0.0);

        let last_two = recent(&config, 2);
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[0].reason, "third");
        assert_eq!(last_two[1].reason, "second");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn relative_time_formats_coarse_buckets() {
        assert_eq!(relative_time(1000, 990), "just now");
        assert_eq!(relative_time(1000, 400), "10m ago");
        assert_eq!(relative_time(90_000, 10_000), "22h ago");
        assert_eq!(relative_time(1_000_000, 10_000), "11d ago");
    }

    #[test]
    fn disabled_audit_writes_nothing() {
        let config = AuditConfig {
            enabled: false,
            log_path: "/nonexistent/should/not/be/created.jsonl".to_string(),
        };
        log_route(&config, "local", "test", &[], 1, 1, 0.0);
        assert!(!std::path::Path::new(&config.log_path).exists());
    }

    #[test]
    fn enabled_audit_appends_a_json_line() {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        log_route(
            &config,
            "local",
            "simple query",
            &["Email address detected".to_string()],
            10,
            20,
            0.0,
        );

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"provider\":\"local\""));
        assert!(content.contains("Email address detected"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn spend_today_sums_only_todays_entries() {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-spend-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        // A real entry from "now" (via log_route)...
        log_route(&config, "groq", "cloud call", &[], 100, 100, 0.50);
        // ...plus a hand-crafted entry from 10 days ago, which must not count.
        let ten_days_ago = start_of_today_unix() - (10 * 86400);
        let old_entry = serde_json::json!({
            "timestamp": ten_days_ago, "provider": "groq", "reason": "old",
            "privacy_flags": [], "input_tokens": 1, "output_tokens": 1, "cost_usd": 99.0
        });
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", old_entry).unwrap();

        let total = spend_today(&config);
        assert!(
            (total - 0.50).abs() < 1e-9,
            "expected only today's $0.50, got {total}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_entries_skips_unparseable_lines() {
        let path = std::env::temp_dir().join(format!(
            "buzz-audit-read-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "not json\n{\"also\": \"not an entry\"}\n").unwrap();
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        assert!(read_entries(&config).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn expand_tilde_resolves_against_given_home() {
        assert_eq!(
            expand_tilde("~/.buzz/audit.jsonl", "/home/testuser"),
            std::path::PathBuf::from("/home/testuser/.buzz/audit.jsonl")
        );
        assert_eq!(
            expand_tilde("/abs/path.jsonl", "/home/testuser"),
            std::path::PathBuf::from("/abs/path.jsonl")
        );
    }
}
