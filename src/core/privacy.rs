use regex::Regex;

pub fn scan_text(text: &str) -> bool {
    // SSN: ###-##-####
    let ssn_re = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
    if ssn_re.is_match(text) {
        return true;
    }

    // Credit card: #### #### #### #### (16 digits, spaces or no spaces)
    let cc_re = Regex::new(r"\b(?:\d[ -]*?){13,16}\b").unwrap();
    if cc_re.is_match(text) {
        return true;
    }

    // Email
    let email_re = Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap();
    if email_re.is_match(text) {
        return true;
    }

    // US Phone: (123) 456-7890 or 123-456-7890
    let phone_re = Regex::new(r"\b\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap();
    if phone_re.is_match(text) {
        return true;
    }

    false
}
