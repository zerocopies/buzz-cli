use super::{InferenceProvider, ProviderResponse};
use std::path::PathBuf;

pub struct LocalQwenProvider {
    pub model_path: PathBuf,
}

impl LocalQwenProvider {
    pub fn new(model_path: PathBuf) -> Self { Self { model_path } }
    pub fn model_exists(&self) -> bool { self.model_path.exists() }
}

#[async_trait::async_trait]
impl InferenceProvider for LocalQwenProvider {
    async fn generate(&self, _prompt: &str) -> Result<ProviderResponse, Box<dyn std::error::Error + Send + Sync>> {
        Ok(ProviderResponse { content: "Local placeholder".to_string(), token_count: 0, stop_reason: None })
    }
}
