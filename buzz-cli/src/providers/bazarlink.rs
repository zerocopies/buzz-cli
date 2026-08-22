use buzz_core::provider::{InferenceProvider, ProviderResponse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct BazaarLinkRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct BazaarLinkResponse {
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    completion_tokens: u64,
    prompt_tokens: u64,
}

pub struct BazaarLinkProvider {
    api_key: String,
    model: String,
}

impl BazaarLinkProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| "auto:free".to_string()), // Default to free tier
        }
    }
}

impl InferenceProvider for BazaarLinkProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ProviderResponse, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::new();
        let request = BazaarLinkRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            max_tokens: 1000,
        };

        let start = std::time::Instant::now();
        let response = client.post("https://api.bazaarlink.ai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let err_text = response.text().await?;
            return Err(format!("BazaarLink API error {}: {}", status, err_text).into());
        }

        let data: BazaarLinkResponse = response.json().await?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        let content = data.choices.first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        on_token(&content);

        Ok(ProviderResponse {
            content,
            input_tokens: data.usage.prompt_tokens,
            output_tokens: data.usage.completion_tokens,
            elapsed_ms: elapsed,
        })
    }
}
