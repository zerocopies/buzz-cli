use serde::{Deserialize, Serialize};
use crate::policy::RoutingConfig;
use super::privacy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteProvider {
    Local,
    Groq,
    Anthropic,
    Gemini,
    HuggingFace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub provider: RouteProvider,
    pub reason: String,
    pub confidence: f32,  // 0.0 - 1.0
}

/// Main routing decision function — called for every prompt
pub fn decide_route(prompt: &str, config: &RoutingConfig) -> Route {
    // Check privacy sensitivity FIRST
    if config.always_local_if_sensitive && privacy::scan_text(prompt) {
        return Route {
            provider: RouteProvider::Local,
            reason: "sensitive content detected (PII/secrets)".to_string(),
            confidence: 0.98,
        };
    }

    // Analyze complexity
    let complexity = analyze_complexity(prompt);
    
    // Simple queries → local
    if complexity <= 2 {
        return Route {
            provider: RouteProvider::Local,
            reason: format!("simple query (complexity={})", complexity),
            confidence: 0.85,
        };
    }

    // Complex code/reasoning → cloud
    if complexity >= 6 && !config.cloud_fallback_order.is_empty() {
        let first = &config.cloud_fallback_order[0];
        let provider = match first.to_lowercase().as_str() {
            "anthropic" => RouteProvider::Anthropic,
            "gemini" => RouteProvider::Gemini,
            "huggingface" | "hf" => RouteProvider::HuggingFace,
            _ => RouteProvider::Groq,
        };
        
        return Route {
            provider,
            reason: format!("complex task (complexity={}), using {}", complexity, provider_to_name(provider)),
            confidence: 0.90,
        };
    }

    // Medium complexity → prefer local, fallback to groq
    Route {
        provider: RouteProvider::Local,
        reason: format!("medium complexity (complexity={}), local preferred", complexity),
        confidence: 0.75,
    }
}

fn provider_to_name(p: RouteProvider) -> String {
    match p {
        RouteProvider::Local => "local".to_string(),
        RouteProvider::Groq => "groq".to_string(),
        RouteProvider::Anthropic => "anthropic".to_string(),
        RouteProvider::Gemini => "gemini".to_string(),
        RouteProvider::HuggingFace => "huggingface".to_string(),
    }
}

/// Score prompt complexity 1-10 based on characteristics
pub fn analyze_complexity(prompt: &str) -> u32 {
    let mut score = 1u32;
    let lower = prompt.to_lowercase();
    let words = prompt.split_whitespace().count();

    // Length-based scoring
    if words > 100 { score += 3; }
    else if words > 50 { score += 2; }
    else if words > 20 { score += 1; }

    // Code-related keywords
    let code_patterns = vec![
        "fn ", "def ", "function ", "class ", "import ", 
        "include!", "impl ", "trait ", "struct ", "enum ",
        "write ", "read ", "file ", "database ", "sql ",
        "algorithm ", "optimize ", "refactor ", "debug "
    ];
    
    if code_patterns.iter().any(|p| lower.contains(p)) {
        score += 2;
    }

    // Reasoning-intensive patterns
    let reasoning_patterns = vec![
        "explain", "why", "how does", "compare", "analyze",
        "design", "architecture", "tradeoff", "best practice",
        "solve", "implement", "create from scratch"
    ];
    
    if reasoning_patterns.iter().any(|p| lower.contains(p)) {
        score += 2;
    }

    // Math/logic
    if lower.contains("calculate") || lower.contains("math") || prompt.chars().filter(|c| c.is_ascii_digit()).count() > 5 {
        score += 1;
    }

    // Multi-part questions
    if prompt.matches('?').count() > 2 {
        score += 1;
    }

    score.min(10)
}

pub fn analyze_complexity_full(prompt: &str) -> (u32, Vec<String>) {
    let mut factors = Vec::new();
    let mut score = 1u32;
    let lower = prompt.to_lowercase();

    // Factor tracking
    if prompt.split_whitespace().count() > 50 {
        factors.push("long prompt (>50 words)".to_string());
        score += 2;
    }

    if ["explain", "why", "how"].iter().any(|k| lower.contains(*k)) {
        factors.push("explanation request".to_string());
        score += 1;
    }

    if ["code", "function", "implement", "write"].iter().any(|k| lower.contains(*k)) {
        factors.push("coding task".to_string());
        score += 2;
    }

    (score.min(10), factors)
}
