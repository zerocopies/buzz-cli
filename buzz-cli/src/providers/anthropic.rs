use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-3-5-haiku-20241022";

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    pub async fn generate(&self, prompt: &str) -> Result<(String, u64, f64), Box<dyn Error + Send + Sync>> {
        let response = self.client
            .post(ANTHROPIC_ENDPOINT)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 1024
            }))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(format!("Anthropic {} - {}", status, body).into());
        }

        let body: Value = response.json().await?;

        let content = body["content"][0]["text"]
            .as_str()
            .ok_or("No content in response")?
            .to_string();

        let input_tokens = body["usage"]["input_tokens"].as_u64().unwrap_or(0);
        let output_tokens = body["usage"]["output_tokens"].as_u64().unwrap_or(0);
        let total_tokens = input_tokens + output_tokens;

        let cost = (input_tokens as f64 * 0.000008) + (output_tokens as f64 * 0.000024);

        Ok((content, total_tokens, cost))
    }
}
