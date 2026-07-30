use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

use super::sse::for_each_sse_data;
use buzz_core::{InferenceProvider, ProviderResponse};

const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
// claude-3-5-haiku-20241022 (the original default here) has been fully
// retired — confirmed live against /v1/models, it's no longer in the
// account's available model list at all. This is the current fast/cheap
// tier's equivalent.
const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";

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
}

impl InferenceProvider for AnthropicProvider {
    async fn generate(
        &mut self,
        prompt: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ProviderResponse, Box<dyn Error>> {
        let start = std::time::Instant::now();
        let response = self
            .client
            .post(ANTHROPIC_ENDPOINT)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 1024,
                "stream": true
            }))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(format!("Anthropic {} - {}", status, body).into());
        }

        let mut content = String::new();
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut stream_error: Option<String> = None;

        // Anthropic's SSE payloads carry a "type" field identifying the
        // event, duplicating the "event:" line — dispatching off that
        // field means the generic data-only SSE reader is sufficient here
        // too, no need to also track the "event:" line.
        for_each_sse_data(response, |data| {
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                return true;
            };
            match event["type"].as_str() {
                Some("content_block_delta") => {
                    if let Some(piece) = event["delta"]["text"].as_str() {
                        content.push_str(piece);
                        on_token(piece);
                    }
                }
                Some("message_start") => {
                    if let Some(t) = event["message"]["usage"]["input_tokens"].as_u64() {
                        input_tokens = t;
                    }
                }
                Some("message_delta") => {
                    if let Some(t) = event["usage"]["output_tokens"].as_u64() {
                        output_tokens = t;
                    }
                }
                Some("error") => {
                    stream_error = Some(
                        event["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown streaming error")
                            .to_string(),
                    );
                    return false;
                }
                _ => {}
            }
            true
        })
        .await?;

        if let Some(e) = stream_error {
            return Err(format!("Anthropic stream error: {e}").into());
        }
        if content.is_empty() {
            return Err("No content in response".into());
        }
        if input_tokens == 0 {
            input_tokens = (prompt.len() as u64 / 4).max(1);
        }
        if output_tokens == 0 {
            output_tokens = (content.len() as u64 / 4).max(1);
        }

        Ok(ProviderResponse {
            content,
            input_tokens,
            output_tokens,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }
}
