use std::path::Path;

#[derive(Debug, Clone)]
pub struct Policy {
    pub providers: Providers,
    pub routing: Routing,
    pub cost: Cost,
    pub local: Local,
    pub audit: Audit,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            providers: Providers::default(),
            routing: Routing::default(),
            cost: Cost::default(),
            local: Local::default(),
            audit: Audit::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Providers {
    pub groq: Option<String>,
    pub anthropic: Option<String>,
    pub gemini: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Routing {
    pub always_local_if_sensitive: bool,
    pub cloud_threshold: String, // "complex" or "any"
    pub cloud_fallback_order: Vec<String>, // e.g., ["groq", "anthropic"]
}

#[derive(Debug, Clone, Default)]
pub struct Cost {
    pub total_spent_usd: f64,
    pub daily_budget_usd: f64,
    pub max_per_request_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Local {
    pub model_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct Audit {
    pub enabled: bool,
    pub log_path: String,
}

impl Policy {
    pub fn default_model_path() -> String {
        if cfg!(windows) {
            format!("{}\\buzz\\models\\qwen2.5-1.5b-q4.gguf", std::env::var("APPDATA").unwrap_or_else(|_| "C:\\Users\\Public".to_string()))
        } else {
            format!("{}/.buzz/models/qwen2.5-1.5b-q4.gguf", std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
        }
    }

    pub fn default_audit_path() -> String {
        if cfg!(windows) {
            format!("{}\\buzz\\audit.jsonl", std::env::var("APPDATA").unwrap_or_else(|_| "C:\\Users\\Public".to_string()))
        } else {
            format!("{}/.buzz/audit.jsonl", std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
        }
    }
}
