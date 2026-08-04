use super::privacy;
use crate::policy::RoutingConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteProvider {
    Local,
    Groq,
    Gemini,
    HuggingFace,
}

impl RouteProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteProvider::Local => "local",
            RouteProvider::Groq => "groq",
            RouteProvider::Gemini => "gemini",
            RouteProvider::HuggingFace => "huggingface",
        }
    }
}

/// Unrecognized provider name passed to `str::parse::<RouteProvider>()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownProvider(pub String);

impl std::fmt::Display for UnknownProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown provider: {}", self.0)
    }
}

impl std::error::Error for UnknownProvider {}

impl std::str::FromStr for RouteProvider {
    type Err = UnknownProvider;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(RouteProvider::Local),
            "groq" => Ok(RouteProvider::Groq),
            "gemini" => Ok(RouteProvider::Gemini),
            "huggingface" | "hf" => Ok(RouteProvider::HuggingFace),
            other => Err(UnknownProvider(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub provider: RouteProvider,
    pub reason: String,
    pub confidence: f32, // 0.0 - 1.0
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
            "gemini" => RouteProvider::Gemini,
            "huggingface" | "hf" => RouteProvider::HuggingFace,
            _ => RouteProvider::Groq,
        };

        return Route {
            provider,
            reason: format!(
                "complex task (complexity={}), using {}",
                complexity,
                provider.as_str()
            ),
            confidence: 0.90,
        };
    }

    // Medium complexity → prefer local, fallback to groq
    Route {
        provider: RouteProvider::Local,
        reason: format!(
            "medium complexity (complexity={}), local preferred",
            complexity
        ),
        confidence: 0.75,
    }
}

/// Score prompt complexity 1-10 based on characteristics
pub fn analyze_complexity(prompt: &str) -> u32 {
    let mut score = 1u32;
    let lower = prompt.to_lowercase();
    let words = prompt.split_whitespace().count();

    // Length-based scoring
    if words > 100 {
        score += 3;
    } else if words > 50 {
        score += 2;
    } else if words > 20 {
        score += 1;
    }

    // Code-related keywords
    let code_patterns = vec![
        "fn ",
        "def ",
        "function ",
        "class ",
        "import ",
        "include!",
        "impl ",
        "trait ",
        "struct ",
        "enum ",
        "write ",
        "read ",
        "file ",
        "database ",
        "sql ",
        "algorithm ",
        "optimize ",
        "refactor ",
        "debug ",
    ];

    if code_patterns.iter().any(|p| lower.contains(p)) {
        score += 2;
    }

    // Reasoning-intensive patterns
    let reasoning_patterns = vec![
        "explain",
        "why",
        "how does",
        "compare",
        "analyze",
        "design",
        "architecture",
        "tradeoff",
        "best practice",
        "solve",
        "implement",
        "create from scratch",
    ];

    if reasoning_patterns.iter().any(|p| lower.contains(p)) {
        score += 2;
    }

    // Math/logic
    if lower.contains("calculate")
        || lower.contains("math")
        || prompt.chars().filter(|c| c.is_ascii_digit()).count() > 5
    {
        score += 1;
    }

    // Multi-part questions
    if prompt.matches('?').count() > 2 {
        score += 1;
    }

    score.min(10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::RoutingConfig;

    fn config(fallback: &[&str]) -> RoutingConfig {
        RoutingConfig {
            always_local_if_sensitive: true,
            cloud_fallback_order: fallback.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn trivial_prompt_is_low_complexity() {
        assert_eq!(analyze_complexity("hi"), 1);
    }

    #[test]
    fn long_prompt_raises_complexity() {
        let short = analyze_complexity("write a function");
        let long = analyze_complexity(&"write a function that does something useful ".repeat(20));
        assert!(long > short);
    }

    #[test]
    fn complexity_is_capped_at_ten() {
        let huge = format!(
            "{} explain why compare analyze design architecture tradeoff best practice solve implement calculate math 123456 ??? ",
            "algorithm optimize refactor debug ".repeat(50)
        );
        assert_eq!(analyze_complexity(&huge), 10);
    }

    #[test]
    fn simple_query_routes_local() {
        let route = decide_route("hi", &config(&["groq"]));
        assert_eq!(route.provider, RouteProvider::Local);
    }

    #[test]
    fn sensitive_content_always_routes_local_even_if_complex() {
        let cfg = config(&["gemini"]);
        let complex_but_sensitive = format!(
            "{} my ssn is 123-45-6789, please explain and analyze the architecture tradeoffs",
            "implement refactor optimize debug algorithm database ".repeat(20)
        );
        let route = decide_route(&complex_but_sensitive, &cfg);
        assert_eq!(route.provider, RouteProvider::Local);
        assert!(route.reason.contains("sensitive"));
    }

    #[test]
    fn complex_prompt_routes_to_first_cloud_fallback() {
        let complex = format!(
            "{} please explain and analyze the architecture tradeoffs and design",
            "implement refactor optimize debug algorithm database ".repeat(20)
        );
        assert!(analyze_complexity(&complex) >= 6);

        assert_eq!(
            decide_route(&complex, &config(&["gemini"])).provider,
            RouteProvider::Gemini
        );
        assert_eq!(
            decide_route(&complex, &config(&["hf"])).provider,
            RouteProvider::HuggingFace
        );
        assert_eq!(
            decide_route(&complex, &config(&["groq"])).provider,
            RouteProvider::Groq
        );
    }

    #[test]
    fn provider_name_round_trips() {
        for p in [
            RouteProvider::Local,
            RouteProvider::Groq,
            RouteProvider::Gemini,
            RouteProvider::HuggingFace,
        ] {
            assert_eq!(p.as_str().parse(), Ok(p));
        }
        assert_eq!("hf".parse(), Ok(RouteProvider::HuggingFace));
        assert!("nonsense".parse::<RouteProvider>().is_err());
    }

    #[test]
    fn complex_prompt_with_empty_fallback_order_stays_local() {
        let complex = format!(
            "{} please explain and analyze the architecture tradeoffs and design",
            "implement refactor optimize debug algorithm database ".repeat(20)
        );
        let route = decide_route(&complex, &config(&[]));
        assert_eq!(route.provider, RouteProvider::Local);
    }
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

    if ["code", "function", "implement", "write"]
        .iter()
        .any(|k| lower.contains(*k))
    {
        factors.push("coding task".to_string());
        score += 2;
    }

    (score.min(10), factors)
}
