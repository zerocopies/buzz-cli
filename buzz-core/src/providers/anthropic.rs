use super::{InferenceProvider, ProviderResponse};

pub struct AnthropicProvider {
    pub api_key: String,
    pub model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| "claude-3-5-haiku-20241022".to_string()),
        }
    }
}

#[async_trait::async_trait]
impl InferenceProvider for AnthropicProvider {
    async fn generate(&self, _prompt: &str) -> Result<ProviderResponse, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement actual Anthropic API call
        Ok(ProviderResponse {
            content: "Anthropic placeholder response".to_string(),
            token_count: 0,
            stop_reason: None,
        })
    }
}
