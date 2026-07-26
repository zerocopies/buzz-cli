use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

const DEFAULT_MODEL: &str = "gemini-1.5-flash";

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    pub async fn generate(&self, prompt: &str) -> Result<(String, u64, f64), Box<dyn Error + Send + Sync>> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&json!({
                "contents": [{"parts": [{"text": prompt}]}],
                "generationConfig": {"temperature": 0.7, "maxOutputTokens": 1024}
            }))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(format!("Gemini {} - {}", status, body).into());
        }

        let body: Value = response.json().await?;

        let content = body["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or("No content in response")?
            .to_string();

        let estimated_tokens = prompt.len() as u64 / 4 + content.len() as u64 / 4;
        let cost = estimated_tokens as f64 * 0.00000035;

        Ok((content, estimated_tokens, cost))
    }
}
