use reqwest::Client;
use serde_json::{json, Value};
use std::error::Error;

const HF_ENDPOINT: &str = "https://api-inference.huggingface.co/models";
const DEFAULT_MODEL: &str = "meta-llama/Llama-3.3-70B-Instruct";

pub struct HuggingFaceProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl HuggingFaceProvider {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    pub async fn generate(&self, prompt: &str) -> Result<(String, u64, f64), Box<dyn Error + Send + Sync>> {
        let url = format!("{}/{}", HF_ENDPOINT, self.model);

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&json!({
                "inputs": prompt,
                "parameters": {
                    "temperature": 0.7,
                    "max_new_tokens": 1024,
                    "return_full_text": false
                }
            }))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(format!("HuggingFace {} - {}", status, body).into());
        }

        let body: Value = response.json().await?;

        // HF returns array of objects with "generated_text"
        let content = body[0]["generated_text"]
            .as_str()
            .or_else(|| body["generated_text"].as_str())
            .or_else(|| body[0]["summary_text"].as_str())
            .ok_or("No content in HuggingFace response")?
            .to_string();

        let estimated_tokens = prompt.len() as u64 / 4 + content.len() as u64 / 4;
        let cost = 0.0; // HF free tier

        Ok((content, estimated_tokens, cost))
    }
}
