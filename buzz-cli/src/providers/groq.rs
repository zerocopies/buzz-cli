use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

use super::sse::for_each_sse_data;
use buzz_core::{InferenceProvider, ProviderResponse};

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

    #[allow(dead_code)] // reserved for a future "test API key" command
    pub async fn test_connection(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let response = self
            .client
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
}

impl InferenceProvider for GroqProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ProviderResponse, Box<dyn Error>> {
        let start = std::time::Instant::now();
        let response = self
            .client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.7,
                "max_tokens": 1024,
                "stream": true
            }))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(format!("Groq {} - {}", status, body).into());
        }

        let mut content = String::new();
        // Real usage, if a chunk happens to include it (OpenAI-compatible
        // APIs vary on this); falls back to a length-based estimate below
        // if never populated, same conservative approach already used for
        // Gemini/HuggingFace rather than depending on an unconfirmed field.
        let mut real_input_tokens: Option<u64> = None;
        let mut real_output_tokens: Option<u64> = None;

        for_each_sse_data(response, |data| {
            let Ok(chunk) = serde_json::from_str::<Value>(data) else {
                return true; // skip a malformed chunk rather than aborting the whole stream
            };
            if let Some(piece) = chunk["choices"][0]["delta"]["content"].as_str() {
                content.push_str(piece);
                on_token(piece);
            }
            if let Some(usage) = chunk.get("usage") {
                real_input_tokens = usage["prompt_tokens"].as_u64();
                real_output_tokens = usage["completion_tokens"].as_u64();
            }
            true
        })
        .await?;

        if content.is_empty() {
            return Err("No content in Groq response".into());
        }

        let input_tokens = real_input_tokens.unwrap_or_else(|| (prompt.len() as u64 / 4).max(1));
        let output_tokens = real_output_tokens.unwrap_or_else(|| (content.len() as u64 / 4).max(1));

        Ok(ProviderResponse {
            content,
            input_tokens,
            output_tokens,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }
}
