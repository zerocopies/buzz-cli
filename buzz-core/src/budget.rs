use crate::audit;
use crate::core::cost::calculate_cost;
use crate::core::decision::RouteProvider;
use crate::policy::Config;

/// Every real cloud provider caps a single response at 1024 output tokens
/// today (confirmed against each provider's request body) — used as the
/// worst case for a pre-flight cost estimate, since actual token counts
/// aren't known until the response returns.
const MAX_OUTPUT_TOKENS_ESTIMATE: u64 = 1024;

/// Conservative pre-flight cost estimate for a prompt against a given
/// provider, assuming the worst case (max possible output). Local is
/// always free regardless of length.
pub fn estimate_cost(prompt: &str, provider: RouteProvider) -> f64 {
    if provider == RouteProvider::Local {
        return 0.0;
    }
    let input_tokens = (prompt.len() as u64 / 4).max(1);
    calculate_cost(input_tokens, MAX_OUTPUT_TOKENS_ESTIMATE, provider)
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetCheck {
    Ok,
    ExceedsPerRequest {
        estimated: f64,
        limit: f64,
    },
    ExceedsDaily {
        /// Committed spend *plus* every outstanding reservation, not just
        /// logged spend — this is `reserve`'s "already accounted for"
        /// total, so it reflects in-flight holds that haven't committed
        /// (or released) yet.
        spent_today: f64,
        estimated: f64,
        limit: f64,
    },
}

impl BudgetCheck {
    pub fn is_ok(&self) -> bool {
        matches!(self, BudgetCheck::Ok)
    }
}

impl std::fmt::Display for BudgetCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetCheck::Ok => write!(f, "within budget"),
            BudgetCheck::ExceedsPerRequest { estimated, limit } => write!(
                f,
                "estimated cost ${estimated:.6} exceeds max_per_request_usd (${limit:.2}). \
                 Raise it with /settings, or edit ~/.buzz/config.toml."
            ),
            BudgetCheck::ExceedsDaily {
                spent_today,
                estimated,
                limit,
            } => write!(
                f,
                "today's spend so far (${spent_today:.6}) plus this request's estimate (${estimated:.6}) \
                 would exceed daily_budget_usd (${limit:.2}). Raise it with /settings budget, \
                 wait for the day to roll over, or use local."
            ),
        }
    }
}

/// Local is exempt (always free); cloud requests are checked against both
/// the per-request cap and the rolling daily total from the audit log.
///
/// This is a plain read: it answers "would this fit right now," with no
/// hold on the answer. Two concurrent callers can both read the same
/// pre-write total and both get `Ok` — correct for buzz-cli's single-
/// caller usage (nothing else is in flight to race against), but not
/// under concurrent load. `reserve` below closes that gap; prefer it for
/// any caller with more than one request in flight at a time.
pub fn check(cfg: &Config, provider: RouteProvider, estimated_cost: f64) -> BudgetCheck {
    if provider == RouteProvider::Local {
        return BudgetCheck::Ok;
    }
    if estimated_cost > cfg.cost.max_per_request_usd {
        return BudgetCheck::ExceedsPerRequest {
            estimated: estimated_cost,
            limit: cfg.cost.max_per_request_usd,
        };
    }
    let spent_today = audit::spend_today(&cfg.audit);
    if spent_today + estimated_cost > cfg.cost.daily_budget_usd {
        return BudgetCheck::ExceedsDaily {
            spent_today,
            estimated: estimated_cost,
            limit: cfg.cost.daily_budget_usd,
        };
    }
    BudgetCheck::Ok
}

/// A hold on the daily budget for one in-flight cloud request. Opaque —
/// resolve it exactly once via `commit` (request succeeded) or `release`
/// (request failed before spending anything). Holding it and never
/// resolving it leaks that much of the daily cap for the rest of the day.
#[derive(Debug)]
pub struct Reservation(audit::Reservation);

/// Atomically checks *and* reserves `estimated_cost` against the daily
/// cap — the fix for `check`'s race: the read of "current total" and the
/// write that reserves this request's share happen under one lock, so a
/// second concurrent `reserve` can't read a total that doesn't yet
/// account for the first. `Local` is exempt and reserves nothing
/// (`Ok(None)`), matching `check`.
///
/// The caller must resolve the returned `Some(Reservation)` exactly once
/// — `commit` on success, `release` on failure — or its hold on the cap
/// never clears.
pub fn reserve(
    cfg: &Config,
    provider: RouteProvider,
    estimated_cost: f64,
) -> Result<Option<Reservation>, BudgetCheck> {
    if provider == RouteProvider::Local {
        return Ok(None);
    }
    if estimated_cost > cfg.cost.max_per_request_usd {
        return Err(BudgetCheck::ExceedsPerRequest {
            estimated: estimated_cost,
            limit: cfg.cost.max_per_request_usd,
        });
    }
    audit::reserve(&cfg.audit, estimated_cost, cfg.cost.daily_budget_usd)
        .map(|r| Some(Reservation(r)))
        .map_err(|already_accounted| BudgetCheck::ExceedsDaily {
            spent_today: already_accounted,
            estimated: estimated_cost,
            limit: cfg.cost.daily_budget_usd,
        })
}

/// The reserved request failed before spending anything — release its
/// hold on the daily cap. A no-op for `None` (Local never reserves).
pub fn release(reservation: Option<Reservation>) {
    if let Some(Reservation(r)) = reservation {
        audit::release(r);
    }
}

/// The reserved request completed — log the actual outcome. For a cloud
/// request (`Some`), this atomically releases the reservation and writes
/// the real entry, so there's never a window where the spend is neither
/// reserved nor committed. For `None` (Local, nothing was reserved),
/// this is just the ordinary unconditional audit-trail write every
/// request gets.
#[allow(clippy::too_many_arguments)]
pub fn commit(
    reservation: Option<Reservation>,
    cfg: &Config,
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
    match reservation {
        Some(Reservation(r)) => audit::commit_reservation(
            r,
            &cfg.audit,
            caller,
            request_id,
            provider,
            reason,
            privacy_flags,
            input_tokens,
            output_tokens,
            actual_cost,
            sensitivity_forced_local,
        ),
        None => audit::log_route(
            &cfg.audit,
            caller,
            request_id,
            provider,
            reason,
            privacy_flags,
            input_tokens,
            output_tokens,
            actual_cost,
            sensitivity_forced_local,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AuditConfig, CostConfig};

    fn cfg_with(cost: CostConfig, log_path: &str) -> Config {
        let mut cfg = Config::default();
        cfg.cost = cost;
        cfg.audit = AuditConfig {
            enabled: true,
            log_path: log_path.to_string(),
        };
        cfg
    }

    #[test]
    fn local_is_always_ok_regardless_of_length() {
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 0.0,
                daily_budget_usd: 0.0,
            },
            "/nonexistent.jsonl",
        );
        let huge_prompt = "x".repeat(100_000);
        assert_eq!(estimate_cost(&huge_prompt, RouteProvider::Local), 0.0);
        assert!(check(&cfg, RouteProvider::Local, 0.0).is_ok());
    }

    #[test]
    fn blocks_when_per_request_estimate_exceeds_limit() {
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 0.000001,
                daily_budget_usd: 1000.0,
            },
            "/nonexistent.jsonl",
        );
        let estimated = estimate_cost("hello", RouteProvider::Gemini);
        assert!(estimated > 0.0);
        let result = check(&cfg, RouteProvider::Gemini, estimated);
        assert!(matches!(result, BudgetCheck::ExceedsPerRequest { .. }));
    }

    #[test]
    fn allows_when_within_both_limits() {
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                daily_budget_usd: 10.0,
            },
            "/nonexistent.jsonl",
        );
        let estimated = estimate_cost("hello", RouteProvider::Groq);
        assert_eq!(check(&cfg, RouteProvider::Groq, estimated), BudgetCheck::Ok);
    }

    fn temp_log_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "buzz-budget-{name}-{:?}.jsonl",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reserve_grants_local_nothing_and_reserves_cloud() {
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                daily_budget_usd: 10.0,
            },
            temp_log_path("reserve-basic").to_str().unwrap(),
        );

        assert!(reserve(&cfg, RouteProvider::Local, 5.0).unwrap().is_none());

        let reservation = reserve(&cfg, RouteProvider::Groq, 1.0).unwrap();
        assert!(reservation.is_some());
        // A second reservation for the rest of the cap must still fit —
        // proves reserve() actually holds the first one open rather than
        // forgetting about it once returned.
        let second = reserve(&cfg, RouteProvider::Groq, 8.0).unwrap();
        assert!(second.is_some());
        // Nothing left for a third.
        assert!(reserve(&cfg, RouteProvider::Groq, 1.5).is_err());

        release(reservation);
        release(second);
    }

    #[test]
    fn release_frees_the_reserved_amount_for_reuse() {
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                daily_budget_usd: 1.0,
            },
            temp_log_path("release-frees").to_str().unwrap(),
        );

        let reservation = reserve(&cfg, RouteProvider::Groq, 1.0).unwrap().unwrap();
        assert!(reserve(&cfg, RouteProvider::Groq, 0.01).is_err());
        release(Some(reservation));
        // The full cap should be available again now that nothing's held.
        assert!(reserve(&cfg, RouteProvider::Groq, 1.0).is_ok());
    }

    #[test]
    fn commit_logs_actual_cost_and_clears_the_reservation() {
        let path = temp_log_path("commit-basic");
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                daily_budget_usd: 1.0,
            },
            path.to_str().unwrap(),
        );

        let reservation = reserve(&cfg, RouteProvider::Groq, 0.5).unwrap();
        commit(
            reservation,
            &cfg,
            "test-caller",
            "test-req",
            "groq",
            "test commit",
            &[],
            10,
            10,
            // Actual cost differs from the estimate — commit should log
            // the real figure, not the reservation's original hold.
            0.2,
            false,
        );

        assert!((audit::spend_today(&cfg.audit) - 0.2).abs() < 1e-9);
        // The reservation's $0.50 hold is gone (replaced by the $0.20
        // logged entry), so there's room for another $0.79 today.
        assert!(reserve(&cfg, RouteProvider::Groq, 0.79).is_ok());

        let _ = std::fs::remove_file(&path);
    }

    /// Regression test for the race `reserve` exists to close: under the
    /// old `check` (a plain read with no hold), N concurrent callers could
    /// all read the same pre-write "spent today" total and all pass,
    /// together overspending the cap. A `Barrier` releases every thread
    /// at once so the race window is actually hit.
    #[test]
    fn concurrent_reserve_never_overcommits_the_daily_cap() {
        use std::sync::{Arc, Barrier};

        let path = temp_log_path("concurrent-reserve");

        const COST_PER_REQUEST: f64 = 0.25; // exact in binary fp — no rounding noise
        const CAP_SLOTS: usize = 10; // exactly this many requests should fit
        const ATTEMPTERS: usize = 40; // far more than can fit

        let cfg = Arc::new(cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                // Comfortable margin above CAP_SLOTS worth, so the
                // accept/reject line isn't sitting on a fp rounding edge.
                daily_budget_usd: CAP_SLOTS as f64 * COST_PER_REQUEST + COST_PER_REQUEST / 2.0,
            },
            path.to_str().unwrap(),
        ));

        let barrier = Arc::new(Barrier::new(ATTEMPTERS));
        let handles: Vec<_> = (0..ATTEMPTERS)
            .map(|_| {
                let cfg = Arc::clone(&cfg);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    reserve(&cfg, RouteProvider::Groq, COST_PER_REQUEST)
                })
            })
            .collect();

        let granted: Vec<Reservation> = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter_map(Result::ok)
            .flatten()
            .collect();

        // The exact assertion that would have failed pre-fix: with no
        // hold on an in-flight reservation, more than CAP_SLOTS could all
        // read the same stale total and all get approved.
        assert_eq!(
            granted.len(),
            CAP_SLOTS,
            "expected exactly {CAP_SLOTS} reservations granted out of {ATTEMPTERS} attempts, got {}",
            granted.len()
        );

        // Commit every granted reservation concurrently too, exercising
        // commit_reservation's own lock.
        let commit_handles: Vec<_> = granted
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let cfg = Arc::clone(&cfg);
                std::thread::spawn(move || {
                    commit(
                        Some(r),
                        &cfg,
                        "test-caller",
                        &format!("concurrent-req-{i}"),
                        "groq",
                        &format!("concurrent-{i}"),
                        &[],
                        10,
                        10,
                        COST_PER_REQUEST,
                        false,
                    )
                })
            })
            .collect();
        for h in commit_handles {
            h.join().unwrap();
        }

        let total = audit::spend_today(&cfg.audit);
        let expected = CAP_SLOTS as f64 * COST_PER_REQUEST;
        assert!(
            (total - expected).abs() < 1e-9,
            "expected committed total {expected:.6}, got {total:.6}"
        );
        assert_eq!(
            audit::verify_chain(&cfg.audit),
            audit::ChainStatus::Verified {
                chained_count: CAP_SLOTS,
                legacy_count: 0,
            }
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Regression test for the reservation leak on cancellation: dropping
    /// a `Reservation` without ever calling `commit` or `release` on it —
    /// exactly what happens when buzz-gateway's `TimeoutLayer` cancels an
    /// in-flight handler future after `reserve` succeeded but before the
    /// provider call returned — must still release its hold on the daily
    /// cap automatically, not leak it until process restart.
    #[test]
    fn dropping_an_unresolved_reservation_releases_it_automatically() {
        let path = temp_log_path("drop-releases");
        let cfg = cfg_with(
            CostConfig {
                max_per_request_usd: 10.0,
                daily_budget_usd: 1.0,
            },
            path.to_str().unwrap(),
        );

        {
            let reservation = reserve(&cfg, RouteProvider::Groq, 1.0).unwrap();
            assert!(reservation.is_some());
            // Simulates cancellation: dropped here without ever calling
            // commit() or release() — the exact shape of an Axum handler
            // future getting cancelled mid-flight.
        }

        // If the drop above hadn't released it, this is rejected — a
        // second $1.00 reservation only fits under a $1.00 cap if the
        // first one actually freed its hold when it went out of scope.
        let second = reserve(&cfg, RouteProvider::Groq, 1.0);
        assert!(
            second.is_ok(),
            "second reservation was rejected — the dropped one leaked instead of auto-releasing"
        );

        // Auto-release mirrors explicit release, not commit: nothing was
        // actually spent, so nothing should have been logged.
        assert_eq!(audit::spend_today(&cfg.audit), 0.0);

        release(second.unwrap());
        let _ = std::fs::remove_file(&path);
    }
}
