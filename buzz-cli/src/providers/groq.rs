use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";

pub struct GroqProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl GroqProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: model.unwrap_or_else(|| "llama-3.3-70b-versatile".to_string()),
        }
    }

    pub async fn test_connection(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let response = self.client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(format!("Groq auth failed: {}", response.status()).into());
        }
        Ok(())
    }

    pub async fn generate(&self, prompt: &str) -> Result<(String, u64, f64), Box<dyn Error + Send + Sync>> {
        let response = self.client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.7,
                "max_tokens": 1024
            }))
            .send()
            .await?;

        let body: Value = response.json().await?;
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| "No content in Groq response")?
            .to_string();

        let input_tokens = body["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let output_tokens = body["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        let cost = (input_tokens as f64 * 0.0000005) + (output_tokens as f64 * 0.000001);

        Ok((content, input_tokens + output_tokens, cost))
    }
}
