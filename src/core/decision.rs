use std::error::Error;

use crate::core::privacy::scan_text;
use crate::policy::{Policy, Providers};
use crate::providers::RouteProvider;

pub struct DecisionContext {
    pub providers: Providers,
}

impl DecisionContext {
    pub fn from_config(config: &Policy) -> Result<Self, Box<dyn Error>> {
        Ok(DecisionContext {
            providers: config.providers.clone(),
        })
    }

    pub fn decide_route(&self, prompt: &str) -> RouteProvider {
        // 1. Check for privacy sensitivity
        if scan_text(prompt) {
            return RouteProvider::Local;
        }

        // 2. Check complexity (simplified: length + code-like patterns)
        let is_complex = prompt.len() > 100
            || prompt.contains("function")
            || prompt.contains("def ")
            || prompt.contains("fn ")
            || prompt.contains("class")
            || prompt.contains("import")
            || prompt.contains("return");

        // 3. Routing policy: if complex, use cloud; else local
        if is_complex && !self.providers.is_empty() {
            if let Some(first_provider) = self.providers.first_cloud() {
                return RouteProvider::Cloud(first_provider.clone());
            }
        }

        // Default: local if no cloud or not complex
        RouteProvider::Local
    }
}

impl Providers {
    pub fn first_cloud(&self) -> Option<&String> {
        if let Some(groq) = &self.groq {
            if !groq.is_empty() {
                return Some(groq);
            }
        }
        if let Some(anthropic) = &self.anthropic {
            if !anthropic.is_empty() {
                return Some(anthropic);
            }
        }
        if let Some(gemini) = &self.gemini {
            if !gemini.is_empty() {
                return Some(gemini);
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.groq.is_none()
            && self.anthropic.is_none()
            && self.gemini.is_none()
    }
}
