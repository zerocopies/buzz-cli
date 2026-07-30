use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Providers {
    pub groq_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub hf_api_key: Option<String>,
    #[serde(default)]
    pub groq: String,
    #[serde(default)]
    pub anthropic: String,
    #[serde(default)]
    pub gemini: String,
    #[serde(default)]
    pub hf: String,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            groq_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
            hf_api_key: None,
            groq: String::new(),
            anthropic: String::new(),
            gemini: String::new(),
            hf: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "RoutingConfig::default_always_local_if_sensitive")]
    pub always_local_if_sensitive: bool,
    #[serde(default = "RoutingConfig::default_cloud_fallback_order")]
    pub cloud_fallback_order: Vec<String>,
}

impl RoutingConfig {
    fn default_always_local_if_sensitive() -> bool {
        true
    }

    fn default_cloud_fallback_order() -> Vec<String> {
        vec!["groq".to_string()]
    }
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            always_local_if_sensitive: true,
            cloud_fallback_order: vec!["groq".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    #[serde(default = "CostConfig::default_max_per_request_usd")]
    pub max_per_request_usd: f64,
    #[serde(default = "CostConfig::default_daily_budget_usd")]
    pub daily_budget_usd: f64,
}

impl CostConfig {
    fn default_max_per_request_usd() -> f64 {
        0.01
    }

    fn default_daily_budget_usd() -> f64 {
        5.0
    }
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            max_per_request_usd: 0.01,
            daily_budget_usd: 5.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    #[serde(default = "LocalConfig::default_model_path")]
    pub model_path: String,
    #[serde(default = "LocalConfig::default_model_name")]
    pub model_name: String,
    #[serde(default = "LocalConfig::default_max_context_size")]
    pub max_context_size: usize,
}

impl LocalConfig {
    fn default_model_path() -> String {
        "~/.buzz/models/qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string()
    }

    fn default_model_name() -> String {
        "qwen2.5-1.5b-instruct".to_string()
    }

    fn default_max_context_size() -> usize {
        4096
    }
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            model_path: "~/.buzz/models/qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string(),
            model_name: "qwen2.5-1.5b-instruct".to_string(),
            max_context_size: 4096,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default = "AuditConfig::default_enabled")]
    pub enabled: bool,
    #[serde(default = "AuditConfig::default_log_path")]
    pub log_path: String,
}

impl AuditConfig {
    fn default_enabled() -> bool {
        true
    }

    fn default_log_path() -> String {
        "~/.buzz/audit.jsonl".to_string()
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_path: "~/.buzz/audit.jsonl".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub groq_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub hf_api_key: Option<String>,
    pub budget_limit: Option<f64>,
    pub default_provider: Option<String>,
    #[serde(default)]
    pub providers: Providers,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub cost: CostConfig,
    #[serde(default)]
    pub local: LocalConfig,
    #[serde(default)]
    pub audit: AuditConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            groq_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
            hf_api_key: None,
            budget_limit: Some(10.0),
            default_provider: Some("groq".to_string()),
            providers: Providers::default(),
            routing: RoutingConfig::default(),
            cost: CostConfig::default(),
            local: LocalConfig::default(),
            audit: AuditConfig::default(),
        }
    }
}

impl Config {
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string(self)?;
        std::fs::write(path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}
