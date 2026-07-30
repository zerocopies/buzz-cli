use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref SSN_REGEX: Regex = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
    static ref EMAIL_REGEX: Regex = Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap();
    static ref PHONE_REGEX: Regex = Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b").unwrap();
    static ref CREDIT_CARD_REGEX: Regex = Regex::new(r"\b(?:\d{4}[- ]?){3}\d{4}\b").unwrap();
    // Unlabeled API keys / tokens: long runs of mixed-case letters, digits,
    // and - or _, the shape of most vendor tokens (sk-..., ghp_..., etc.)
    // regardless of whether the word "key" or "secret" appears nearby.
    static ref API_KEY_REGEX: Regex = Regex::new(r"\b[A-Za-z0-9_-]{24,}\b").unwrap();
    // IPv4 addresses.
    static ref IP_REGEX: Regex = Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap();
}

pub fn scan_text(text: &str) -> bool {
    let (detected, _) = analyze_privacy(text);
    detected
}

pub fn contains_pii(text: &str) -> bool {
    SSN_REGEX.is_match(text)
        || EMAIL_REGEX.is_match(text)
        || PHONE_REGEX.is_match(text)
        || CREDIT_CARD_REGEX.is_match(text)
        || API_KEY_REGEX.is_match(text)
        || IP_REGEX.is_match(text)
}

pub fn contains_sensitive_keywords(text: &str) -> bool {
    let lower = text.to_lowercase();
    let keywords = [
        "password",
        "secret",
        "api_key",
        "apikey",
        "credential",
        "private key",
        "bank account",
        "social security",
        "ssn",
        "confidential",
        "internal",
        "proprietary",
        "classified",
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
    if API_KEY_REGEX.is_match(text) {
        flags.push("Possible API key or token detected".to_string());
    }
    if IP_REGEX.is_match(text) {
        flags.push("IP address detected".to_string());
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
    format!(
        "Privacy alerts:\n{}",
        flags
            .iter()
            .map(|f| format!("  • {}", f))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ssn() {
        assert!(scan_text("my ssn is 123-45-6789"));
    }

    #[test]
    fn detects_email() {
        assert!(scan_text("reach me at jane.doe@example.com"));
    }

    #[test]
    fn detects_phone() {
        assert!(scan_text("call 555-123-4567"));
        assert!(scan_text("call 555.123.4567"));
    }

    #[test]
    fn detects_credit_card() {
        assert!(scan_text("card: 4111 1111 1111 1111"));
        assert!(scan_text("card: 4111-1111-1111-1111"));
    }

    #[test]
    fn detects_unlabeled_api_key_shaped_token() {
        assert!(scan_text(
            "here's the token: sk-proj-aZ90bQeR7tLm3xVn2kPq8sYw"
        ));
    }

    #[test]
    fn detects_ip_address() {
        assert!(scan_text("connect to 192.168.1.42"));
    }

    #[test]
    fn detects_sensitive_keywords() {
        assert!(scan_text("here is the password for the account"));
        assert!(scan_text("this document is confidential"));
    }

    #[test]
    fn plain_text_is_not_flagged() {
        assert!(!scan_text("what's a good recipe for banana bread?"));
        assert!(!scan_text("explain how quicksort works"));
    }

    #[test]
    fn short_alphanumeric_words_are_not_flagged_as_keys() {
        assert!(!scan_text("the meeting is at 10am on tuesday"));
    }

    #[test]
    fn analyze_privacy_reports_specific_flags() {
        let (detected, flags) = analyze_privacy("my email is a@b.com");
        assert!(detected);
        assert!(flags.iter().any(|f| f.contains("Email")));
    }
}
