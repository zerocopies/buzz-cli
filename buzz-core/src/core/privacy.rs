use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref SSN_REGEX: Regex = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
    static ref EMAIL_REGEX: Regex = Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap();
    static ref PHONE_REGEX: Regex = Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b").unwrap();
    static ref CREDIT_CARD_REGEX: Regex = Regex::new(r"\b(?:\d{4}[- ]?){3}\d{4}\b").unwrap();
}

pub fn scan_text(text: &str) -> bool {
    let (detected, _) = analyze_privacy(text);
    detected
}

pub fn contains_pii(text: &str) -> bool {
    SSN_REGEX.is_match(text) || 
    EMAIL_REGEX.is_match(text) || 
    PHONE_REGEX.is_match(text) || 
    CREDIT_CARD_REGEX.is_match(text)
}

pub fn contains_sensitive_keywords(text: &str) -> bool {
    let lower = text.to_lowercase();
    let keywords = [
        "password", "secret", "api_key", "apikey", "credential",
        "private key", "bank account", "social security", "ssn",
        "confidential", "internal", "proprietary", "classified"
    ];
    keywords.iter().any(|k| lower.contains(k))
}

pub fn analyze_privacy(text: &str) -> (bool, Vec<String>) {
    let mut flags = Vec::new();
    
    if SSN_REGEX.is_match(text) {
        flags.push("SSN pattern detected".to_string());
    }
    if EMAIL_REGEX.is_match(text) {
        flags.push("Email address detected".to_string());
    }
    if PHONE_REGEX.is_match(text) {
        flags.push("Phone number detected".to_string());
    }
    if CREDIT_CARD_REGEX.is_match(text) {
        flags.push("Credit card pattern detected".to_string());
    }
    if contains_sensitive_keywords(text) {
        flags.push("Sensitive keywords present".to_string());
    }
    
    (!flags.is_empty(), flags)
}

pub fn get_privacy_report(text: &str) -> String {
    let (detected, flags) = analyze_privacy(text);
    if !detected {
        return "No sensitive content detected".to_string();
    }
    format!("Privacy alerts:\n{}", flags.iter().map(|f| format!("  • {}", f)).collect::<Vec<_>>().join("\n"))
}
