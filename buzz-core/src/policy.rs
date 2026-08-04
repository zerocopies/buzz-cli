use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Providers {
    #[serde(default)]
    pub groq: String,
    #[serde(default)]
    pub gemini: String,
    #[serde(default)]
    pub hf: String,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct regression test for the outage this project already had once:
    /// a config saved by this binary must always be loadable by this binary.
    /// Any field the strict deserializer requires without a serde default
    /// would fail this the moment it's added, instead of failing silently
    /// at 2am in someone's ~/.buzz/config.toml.
    #[test]
    fn default_config_round_trips_through_save_and_load() {
        let path = std::env::temp_dir().join(format!(
            "buzz-config-test-{:?}.toml",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        let original = Config::default();
        original
            .save_to_file(&path)
            .expect("save_to_file should succeed");

        let loaded = Config::load_from_file(&path)
            .expect("a config this binary just wrote must parse back cleanly");

        assert_eq!(loaded.local.model_path, original.local.model_path);
        assert_eq!(
            loaded.local.max_context_size,
            original.local.max_context_size
        );
        assert_eq!(
            loaded.cost.max_per_request_usd,
            original.cost.max_per_request_usd
        );
        assert_eq!(loaded.audit.log_path, original.audit.log_path);
        assert_eq!(
            loaded.routing.cloud_fallback_order,
            original.routing.cloud_fallback_order
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_config_with_only_the_fields_a_user_would_hand_write_still_loads() {
        // Mirrors a minimal, hand-edited config: only the fields someone would
        // actually type, missing everything a strict parse used to require.
        let minimal = "[providers]\ngroq = \"sk-test\"\n";
        let path = std::env::temp_dir().join(format!(
            "buzz-config-minimal-test-{:?}.toml",
            std::thread::current().id()
        ));
        std::fs::write(&path, minimal).unwrap();

        let loaded = Config::load_from_file(&path)
            .expect("missing optional fields must fall back to defaults, not fail the whole parse");
        assert_eq!(loaded.providers.groq, "sk-test");
        assert_eq!(
            loaded.local.max_context_size,
            LocalConfig::default_max_context_size()
        );

        let _ = std::fs::remove_file(&path);
    }
}
