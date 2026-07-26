use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPricing {
    pub input_per_token: f64,
    pub output_per_token: f64,
}

pub fn get_pricing(provider: crate::providers::ProviderType) -> ProviderPricing {
    match provider {
        crate::providers::ProviderType::Groq => ProviderPricing {
            input_per_token: 0.00000059,
            output_per_token: 0.00000079,
        },
        crate::providers::ProviderType::Anthropic => ProviderPricing {
            input_per_token: 0.000008,
            output_per_token: 0.000024,
        },
        crate::providers::ProviderType::Gemini => ProviderPricing {
            input_per_token: 0.00000025,
            output_per_token: 0.00000050,
        },
        crate::providers::ProviderType::Local => ProviderPricing {
            input_per_token: 0.0,
            output_per_token: 0.0,
        },
    }
}

pub fn calculate_cost(input_tokens: u64, output_tokens: u64, provider: crate::providers::ProviderType) -> f64 {
    let p = get_pricing(provider);
    (input_tokens as f64 * p.input_per_token) + (output_tokens as f64 * p.output_per_token)
}
