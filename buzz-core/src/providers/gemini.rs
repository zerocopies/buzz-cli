use super::{InferenceProvider, ProviderResponse};

pub struct GeminiProvider {
    pub api_key: String,
    pub model: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| "gemini-1.5-flash".to_string()),
        }
    }
}

#[async_trait::async_trait]
impl InferenceProvider for GeminiProvider {
    async fn generate(&self, _prompt: &str) -> Result<ProviderResponse, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement actual Gemini API call
        Ok(ProviderResponse {
            content: "Gemini placeholder response".to_string(),
            token_count: 0,
            stop_reason: None,
        })
    }
}
