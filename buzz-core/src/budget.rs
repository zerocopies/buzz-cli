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
        let estimated = estimate_cost("hello", RouteProvider::Anthropic);
        assert!(estimated > 0.0);
        let result = check(&cfg, RouteProvider::Anthropic, estimated);
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
}
