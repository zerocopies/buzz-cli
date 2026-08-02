use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::policy::AuditConfig;

/// Marks the hash-chain's starting point — the value used as `prev_hash`
/// for the very first chained entry ever written to a given log.
const GENESIS: &str = "genesis";

/// Per-path state guarded by the same lock as the file itself: how much
/// is currently *reserved* (approved but not yet committed to disk) for
/// this log. Living under the file lock means a reservation's read of
/// "current total" and its write of "my share is now accounted for"
/// happen as one atomic step — see `reserve`.
#[derive(Default)]
struct PathState {
    reserved: f64,
}

/// One lock per resolved audit-log path — every read and every write
/// against a given `audit.jsonl` serializes through the same lock, closing
/// the gap between "read current state" and "write new state" that let
/// concurrent callers (multiple Tokio tasks in buzz-gateway) race:
///   - `spend_today`/`read_entries` could read the file mid-append.
///   - `append_entry` read-last-line-then-hash-then-write with nothing
///     stopping two concurrent writers from both reading the same last
///     line and both appending — corrupting the hash chain (and,
///     since `writeln!` on a `serde_json::Value` isn't a single atomic
///     write syscall, potentially interleaving raw bytes too).
///   - two concurrent budget checks reading the same pre-write "spent
///     today" total and both approving, together overspending the cap —
///     closed by `reserve` folding its read and its write into this same
///     lock, via the `reserved` field above.
///
/// Keyed by path rather than one global lock so unrelated logs (e.g. the
/// distinct temp files each test uses) never contend with each other —
/// only concurrent access to the *same* file serializes. buzz-cli's single-
/// caller usage always resolves to one path, so this degrades to an
/// uncontended lock/unlock per call — no meaningful overhead added there.
static FILE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<PathState>>>>> = OnceLock::new();

fn lock_for(path: &Path) -> Arc<Mutex<PathState>> {
    let registry = FILE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    registry
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(PathState::default())))
        .clone()
}

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
    /// Who made this request — a caller-supplied identity (buzz-gateway's
    /// `X-Buzz-Client` header or request `user` field, or "unknown"), or
    /// `"cli"` for buzz-cli's own one-shot/TUI usage. `#[serde(default)]`
    /// so entries written before this field existed still parse (empty
    /// string, not a parse failure) — same convention as `prev_hash`
    /// above for pre-chaining legacy lines.
    #[serde(default)]
    pub caller: String,
    /// Per-request correlation ID — buzz-gateway's `chatcmpl-<uuid>`, or a
    /// `cli-<nanos>` ID for buzz-cli. Lets a compliance export line up
    /// this entry with request-level logs elsewhere. `#[serde(default)]`
    /// for the same backward-compatibility reason as `caller`.
    #[serde(default)]
    pub request_id: String,
    /// True for a request that was rejected by the budget cap before any
    /// reservation existed (nothing spent, nothing to commit/release) —
    /// distinct from every other entry here, which represents a request
    /// that actually ran. `#[serde(default)]` (false) for entries written
    /// before this field existed, and for every normal `log_route`/
    /// `commit_reservation` entry today.
    #[serde(default)]
    pub budget_rejected: bool,
    /// True if privacy-sensitivity detection forced this request to
    /// `local` regardless of what would otherwise have been routed —
    /// slide 08's "Forced to local model (sensitive)" figure.
    /// `#[serde(default)]` (false) for entries written before this field
    /// existed, and for every request that reached `local` for any other
    /// reason (an explicit override, low complexity, medium-complexity
    /// fallback).
    #[serde(default)]
    pub sensitivity_forced_local: bool,
}

/// `pub(crate)`, not private — the compliance-report export
/// (`compliance.rs`) needs to hash a specific line itself (the range's
/// "terminal hash") using the exact same function the chain uses, rather
/// than reimplementing SHA-256-of-a-line a second time.
pub(crate) fn hash_line(line: &str) -> String {
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
#[allow(clippy::too_many_arguments)]
pub fn log_route(
    config: &AuditConfig,
    caller: &str,
    request_id: &str,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    sensitivity_forced_local: bool,
) {
    if !config.enabled {
        return;
    }
    if let Err(e) = append_entry(
        config,
        caller,
        request_id,
        provider,
        reason,
        privacy_flags,
        input_tokens,
        output_tokens,
        cost,
        false,
        sensitivity_forced_local,
    ) {
        eprintln!(
            "[buzz] warning: could not write audit log ({}): {}",
            config.log_path, e
        );
    }
}

/// A request rejected by the budget cap before any reservation existed —
/// `budget::reserve` (or the plain `check`) refused it outright, so
/// nothing was spent and there's no `Reservation` to `commit`/`release`.
/// Distinct from `log_route`: always writes `budget_rejected: true` with
/// zeroed tokens/cost, so a compliance export can count rejections as
/// their own category instead of conflating them with real (free local,
/// or paid cloud) requests. `sensitivity_forced_local` is meaningless for
/// a request that never got routed at all, so this always logs `false`
/// rather than exposing a parameter nothing can honestly fill in.
pub fn log_rejection(
    config: &AuditConfig,
    caller: &str,
    request_id: &str,
    provider: &str,
    reason: &str,
) {
    if !config.enabled {
        return;
    }
    if let Err(e) = append_entry(
        config,
        caller,
        request_id,
        provider,
        reason,
        &[],
        0,
        0,
        0.0,
        true,
        false,
    ) {
        eprintln!(
            "[buzz] warning: could not write audit log ({}): {}",
            config.log_path, e
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn append_entry(
    config: &AuditConfig,
    caller: &str,
    request_id: &str,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    budget_rejected: bool,
    sensitivity_forced_local: bool,
) -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Everything from here down — reading the current last line to derive
    // prev_hash, through the write that appends onto it — is one critical
    // section. Releasing the lock between the read and the write is
    // exactly the race this exists to close.
    let lock = lock_for(&path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    append_entry_at(
        &path,
        caller,
        request_id,
        provider,
        reason,
        privacy_flags,
        input_tokens,
        output_tokens,
        cost,
        budget_rejected,
        sensitivity_forced_local,
    )
}

/// The actual read-prev-hash-then-write, assuming the caller already
/// holds `lock_for(path)`. Never call this without holding that lock —
/// it's the one thing standing between this and the original race.
#[allow(clippy::too_many_arguments)]
fn append_entry_at(
    path: &Path,
    caller: &str,
    request_id: &str,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    cost: f64,
    budget_rejected: bool,
    sensitivity_forced_local: bool,
) -> std::io::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Chain onto whatever the current last line actually is — including a
    // pre-hash-chaining legacy line, if that's what the log currently ends
    // with. That binds the transition point itself into the chain instead
    // of silently starting a fresh, disconnected chain partway through an
    // existing log.
    let prev_hash = last_line(path)
        .map(|l| hash_line(&l))
        .unwrap_or_else(|| GENESIS.to_string());

    let entry = serde_json::json!({
        "timestamp": timestamp,
        "caller": caller,
        "request_id": request_id,
        "provider": provider,
        "reason": reason,
        "privacy_flags": privacy_flags,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cost_usd": cost,
        "budget_rejected": budget_rejected,
        "sensitivity_forced_local": sensitivity_forced_local,
        "prev_hash": prev_hash,
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", entry)?;
    Ok(())
}

/// Every parseable entry in the audit log. Lines that fail to parse (e.g.
/// hand-edited or from an older schema) are silently skipped rather than
/// failing the whole read — this is a best-effort log, not a database.
pub fn read_entries(config: &AuditConfig) -> Vec<AuditEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    // Held for the whole read so this can never observe a write mid-append
    // (e.g. a torn last line from another thread's in-progress writeln!).
    let lock = lock_for(&path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    read_entries_at(&path)
}

/// Assumes the caller already holds `lock_for(path)`.
fn read_entries_at(path: &Path) -> Vec<AuditEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
        .collect()
}

/// Every parseable entry paired with its *exact* raw line text —
/// `read_entries` alone loses that (a re-serialized `AuditEntry` isn't
/// guaranteed to be byte-identical to what was actually written), and a
/// compliance export needs the real bytes to compute a sub-range's
/// "terminal hash" with the same `hash_line` the chain itself uses.
pub fn read_entries_with_raw_lines(config: &AuditConfig) -> Vec<(String, AuditEntry)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    let lock = lock_for(&path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<AuditEntry>(line)
                .ok()
                .map(|entry| (line.to_string(), entry))
        })
        .collect()
}

/// A hold against the daily budget for a cloud request that's been
/// approved but hasn't completed yet — created by `reserve`. Should be
/// resolved exactly once: `commit_reservation` if the request succeeded
/// (replaces the hold with a real logged entry for the actual cost),
/// `release` if it failed before spending anything. If neither happens —
/// e.g. the future holding it is cancelled outright, as buzz-gateway's
/// 120s `TimeoutLayer` does to an in-flight handler — `Drop` below
/// releases it automatically, so a cancelled request's hold on the daily
/// cap can't outlive the request itself.
#[derive(Debug)]
pub struct Reservation {
    log_path: PathBuf,
    amount: f64,
    /// Set by `release`/`commit_reservation` once they've done their
    /// work, so `Drop` knows not to release a second time on the normal
    /// path. Only ever `false` at drop time for a reservation that was
    /// dropped without going through either — the abnormal/cancellation
    /// case `Drop` exists to catch.
    resolved: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.resolved {
            return;
        }
        // Safety net only — the normal paths (`release`,
        // `commit_reservation`) already mark `resolved` before returning,
        // so this only fires when a reservation is dropped without ever
        // reaching either, i.e. cancellation. Deliberately mirrors
        // `release`'s behavior (decrement only, log nothing): there's no
        // way to know here whether the request would have succeeded, so
        // treat it as "nothing was actually spent," same as an explicit
        // failure. This is pure in-memory bookkeeping behind the same
        // already-poison-recovering Mutex every other access uses — no
        // file I/O, so nothing here can fail in a way that would need a
        // Result `Drop` has no way to return anyway.
        release_amount(&self.log_path, self.amount);
    }
}

/// Shared by `release` and `Drop::drop` — the two places outstanding
/// budget gets returned without writing an audit entry (nothing was
/// actually spent, so there's nothing to log). `commit_reservation`
/// deliberately does NOT go through this: it needs the decrement and the
/// audit-entry write to happen inside one lock acquisition, not two.
fn release_amount(log_path: &Path, amount: f64) {
    let lock = lock_for(log_path);
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    state.reserved = (state.reserved - amount).max(0.0);
}

/// Atomically checks whether `estimated_cost` — added to today's already-
/// committed spend *and* every other reservation currently outstanding
/// against this log — would exceed `daily_budget_usd`, and if not,
/// reserves it. The read of "current total" and the write that reserves
/// this request's share happen under the same lock, so no concurrent
/// caller can read a total that doesn't yet reflect this reservation —
/// the gap that let two concurrent requests both see "under budget" and
/// both proceed.
///
/// On rejection, returns the total already accounted for (committed +
/// outstanding reservations), so the caller can build an accurate error
/// message.
pub fn reserve(
    config: &AuditConfig,
    estimated_cost: f64,
    daily_budget_usd: f64,
) -> Result<Reservation, f64> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = expand_tilde(&config.log_path, &home);
    let lock = lock_for(&path);
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());

    let committed_today: f64 = read_entries_at(&path)
        .iter()
        .filter(|e| e.timestamp >= start_of_today_unix())
        .map(|e| e.cost_usd)
        .sum();
    let already_accounted = committed_today + state.reserved;

    if already_accounted + estimated_cost > daily_budget_usd {
        return Err(already_accounted);
    }
    state.reserved += estimated_cost;
    Ok(Reservation {
        log_path: path,
        amount: estimated_cost,
        resolved: false,
    })
}

/// The reserved request failed before spending anything — release its
/// hold on the daily budget without logging anything.
pub fn release(mut reservation: Reservation) {
    release_amount(&reservation.log_path, reservation.amount);
    reservation.resolved = true;
}

/// The reserved request completed — release the reservation and log the
/// *actual* cost (which may differ from the original estimate) as one
/// atomic step under the same lock, so no window exists where this
/// request's spend is neither reserved nor committed.
#[allow(clippy::too_many_arguments)]
pub fn commit_reservation(
    mut reservation: Reservation,
    config: &AuditConfig,
    caller: &str,
    request_id: &str,
    provider: &str,
    reason: &str,
    privacy_flags: &[String],
    input_tokens: u64,
    output_tokens: u64,
    actual_cost: f64,
    sensitivity_forced_local: bool,
) {
    let lock = lock_for(&reservation.log_path);
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    state.reserved = (state.reserved - reservation.amount).max(0.0);
    reservation.resolved = true;

    if !config.enabled {
        return;
    }
    if let Err(e) = append_entry_at(
        &reservation.log_path,
        caller,
        request_id,
        provider,
        reason,
        privacy_flags,
        input_tokens,
        output_tokens,
        actual_cost,
        false,
        sensitivity_forced_local,
    ) {
        eprintln!(
            "[buzz] warning: could not write audit log ({}): {}",
            config.log_path, e
        );
    }
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

/// `pub(crate)` so `signing.rs` can resolve `~/.buzz/audit_signing.key`
/// the same way every other `~/.buzz/*` path in this crate does, instead
/// of a third copy of this same three-line function.
pub(crate) fn expand_tilde(path: &str, home: &str) -> std::path::PathBuf {
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
    let lock = lock_for(&path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
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
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "one",
            &[],
            1,
            1,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "groq",
            "two",
            &[],
            1,
            1,
            0.1,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "three",
            &[],
            1,
            1,
            0.0,
            false,
        );

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
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "one",
            &[],
            1,
            1,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "two",
            &[],
            1,
            1,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "three",
            &[],
            1,
            1,
            0.0,
            false,
        );

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
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "post-chain entry",
            &[],
            1,
            1,
            0.0,
            false,
        );

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

        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "simple",
            &[],
            10,
            10,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "sensitive",
            &["SSN pattern detected".to_string()],
            10,
            10,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "groq",
            "complex",
            &[],
            100,
            100,
            0.25,
            false,
        );

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

        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "first",
            &[],
            1,
            1,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "second",
            &[],
            1,
            1,
            0.0,
            false,
        );
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "third",
            &[],
            1,
            1,
            0.0,
            false,
        );

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
        log_route(
            &config,
            "test-caller",
            "test-req",
            "local",
            "test",
            &[],
            1,
            1,
            0.0,
            false,
        );
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
            "test-caller",
            "test-req",
            "local",
            "simple query",
            &["Email address detected".to_string()],
            10,
            20,
            0.0,
            false,
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
        log_route(
            &config,
            "test-caller",
            "test-req",
            "groq",
            "cloud call",
            &[],
            100,
            100,
            0.50,
            false,
        );
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

    /// Regression test for the concurrent-access race: before the
    /// per-path lock, N threads hammering `spend_today` (a read) and
    /// `log_route` (a read-then-write) against the same file could
    /// interleave raw bytes mid-`writeln!`, corrupt the hash chain by
    /// having two writers both chain from the same last line, or drop
    /// entries entirely (a corrupted line silently fails to parse in
    /// `read_entries`). A `std::sync::Barrier` releases every thread at
    /// once so the race window is actually hit, not just theoretically
    /// possible.
    #[test]
    fn concurrent_writers_and_readers_do_not_corrupt_the_chain_or_lose_writes() {
        use std::sync::Barrier;

        let path = std::env::temp_dir().join(format!(
            "buzz-audit-concurrent-test-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let config = AuditConfig {
            enabled: true,
            log_path: path.to_string_lossy().to_string(),
        };

        const WRITERS: usize = 40;
        const COST_PER_ENTRY: f64 = 0.01;

        let barrier = Arc::new(Barrier::new(WRITERS));
        let mut handles = Vec::new();
        for i in 0..WRITERS {
            let config = config.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                // Same shape as a real request: read the current spend
                // (budget::check's path), then log this request's cost.
                let _ = spend_today(&config);
                log_route(
                    &config,
                    "test-caller",
                    &format!("concurrent-req-{i}"),
                    "groq",
                    &format!("concurrent-{i}"),
                    &[],
                    10,
                    10,
                    COST_PER_ENTRY,
                    false,
                );
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // No entry silently lost or corrupted into an unparseable line —
        // read_entries() skips anything it can't parse, so this catches
        // byte-level interleaving from concurrent writeln! calls.
        let entries = read_entries(&config);
        assert_eq!(
            entries.len(),
            WRITERS,
            "expected exactly {WRITERS} parseable entries, found {} — \
             some concurrent write was lost or corrupted",
            entries.len()
        );

        // Exactly WRITERS * COST_PER_ENTRY — not less (a lost update) and
        // not more (a torn read double-counting a partial write).
        let total = spend_today(&config);
        let expected = WRITERS as f64 * COST_PER_ENTRY;
        assert!(
            (total - expected).abs() < 1e-9,
            "expected spend total {expected:.6}, got {total:.6}"
        );

        // The hash chain must still be one unbroken line — this is what
        // catches two concurrent writers both chaining from the same
        // prev_hash.
        assert_eq!(
            verify_chain(&config),
            ChainStatus::Verified {
                chained_count: WRITERS,
                legacy_count: 0,
            }
        );

        let _ = std::fs::remove_file(&path);
    }
}
