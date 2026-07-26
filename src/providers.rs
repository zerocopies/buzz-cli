use crate::core::decision::RouteProvider;

#[derive(Debug, Clone)]
pub enum RouteProvider {
    Local,
    Cloud(String),
}

// Cloud providers (placeholder — will be fleshed out later)
pub struct Groq {}
pub struct Anthropic {}
pub struct Gemini {}

impl Groq {
    pub fn generate_response(&self, prompt: &str) -> String {
        format!("Groq: {}", prompt)
    }
}

impl Anthropic {
    pub fn generate_response(&self, prompt: &str) -> String {
        format!("Anthropic: {}", prompt)
    }
}

impl Gemini {
    pub fn generate_response(&self, prompt: &str) -> String {
        format!("Gemini: {}", prompt)
    }
}

// Local inference (placeholder — will use qfz3 later)
pub fn generate_local_response(prompt: &str) -> String {
    format!("Local: {}", prompt)
}
