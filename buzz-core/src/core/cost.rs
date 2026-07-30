use super::decision::RouteProvider;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPricing {
    pub input_per_token: f64,
    pub output_per_token: f64,
}

/// Single source of truth for per-token pricing — every caller (cloud
/// providers and the audit log) must go through this instead of hardcoding
/// its own rates, which is how the Groq price table drifted into two
/// disagreeing numbers before this consolidation.
pub fn get_pricing(provider: RouteProvider) -> ProviderPricing {
    match provider {
        RouteProvider::Groq => ProviderPricing {
            input_per_token: 0.00000059,
            output_per_token: 0.00000079,
        },
        RouteProvider::Anthropic => ProviderPricing {
            input_per_token: 0.000008,
            output_per_token: 0.000024,
        },
        RouteProvider::Gemini => ProviderPricing {
            input_per_token: 0.00000025,
            output_per_token: 0.00000050,
        },
        RouteProvider::HuggingFace => ProviderPricing {
            input_per_token: 0.0,
            output_per_token: 0.0,
        },
        RouteProvider::Local => ProviderPricing {
            input_per_token: 0.0,
            output_per_token: 0.0,
        },
    }
}

pub fn calculate_cost(input_tokens: u64, output_tokens: u64, provider: RouteProvider) -> f64 {
    let p = get_pricing(provider);
    (input_tokens as f64 * p.input_per_token) + (output_tokens as f64 * p.output_per_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_is_always_free() {
        assert_eq!(calculate_cost(1000, 1000, RouteProvider::Local), 0.0);
    }

    #[test]
    fn huggingface_is_free_tier() {
        assert_eq!(calculate_cost(1000, 1000, RouteProvider::HuggingFace), 0.0);
    }

    #[test]
    fn cloud_providers_charge_by_token() {
        for provider in [
            RouteProvider::Groq,
            RouteProvider::Anthropic,
            RouteProvider::Gemini,
        ] {
            let cost = calculate_cost(1000, 1000, provider);
            assert!(cost > 0.0, "{provider:?} should have nonzero cost");
        }
    }

    #[test]
    fn cost_scales_linearly_with_tokens() {
        let one = calculate_cost(100, 100, RouteProvider::Groq);
        let ten = calculate_cost(1000, 1000, RouteProvider::Groq);
        assert!((ten - one * 10.0).abs() < 1e-12);
    }
}
