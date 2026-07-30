use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

use super::sse::for_each_sse_data;
use buzz_core::{InferenceProvider, ProviderResponse};

// gemini-1.5-flash (the original default here) has been fully retired —
// confirmed live against /v1beta/models, a 404 even with a valid key.
// Using the "-latest" alias instead of a dated snapshot this time, so it
// tracks Google's current fast/cheap tier automatically instead of going
// stale the same way again.
const DEFAULT_MODEL: &str = "gemini-flash-latest";

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
}

impl InferenceProvider for GeminiProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ProviderResponse, Box<dyn Error>> {
        // streamGenerateContent + alt=sse is Gemini's documented streaming
        // endpoint — same request body as generateContent, different path
        // suffix and an SSE response instead of one JSON blob.
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model, self.api_key
        );

        let start = std::time::Instant::now();
        let response = self
            .client
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

        let mut content = String::new();
        // Real usage, if a chunk happens to include usageMetadata; falls
        // back to the same length-based estimate the non-streaming path
        // already used, rather than depending on an unconfirmed field.
        let mut real_input_tokens: Option<u64> = None;
        let mut real_output_tokens: Option<u64> = None;

        for_each_sse_data(response, |data| {
            let Ok(chunk) = serde_json::from_str::<Value>(data) else {
                return true;
            };
            if let Some(piece) = chunk["candidates"][0]["content"]["parts"][0]["text"].as_str() {
                content.push_str(piece);
                on_token(piece);
            }
            if let Some(usage) = chunk.get("usageMetadata") {
                real_input_tokens = usage["promptTokenCount"].as_u64();
                real_output_tokens = usage["candidatesTokenCount"].as_u64();
            }
            true
        })
        .await?;

        if content.is_empty() {
            return Err("No content in response".into());
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
