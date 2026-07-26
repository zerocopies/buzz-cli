pub mod groq;
pub mod anthropic;
pub mod gemini;

pub use groq::GroqProvider;
pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Groq,
    Anthropic,
    Gemini,
    Local,
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    pub token_count: u64,
    pub stop_reason: Option<String>,
}

#[async_trait::async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn generate(&self, prompt: &str) -> Result<ProviderResponse, Box<dyn std::error::Error + Send + Sync>>;
}
