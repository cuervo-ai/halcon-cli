use regex::{Regex, RegexSet};

/// PII detector using compiled regex patterns.
///
/// Uses `RegexSet` for SIMD-accelerated parallel matching (detect/contains_pii)
/// and individual `Regex` patterns for locate-and-replace (redact).
///
/// Patterns cover: emails, phone numbers, SSNs, credit cards,
/// IP addresses, AWS keys, SSH private keys, JWT tokens, GitHub tokens,
/// Anthropic API keys, and other common PII formats.
pub struct PiiDetector {
    patterns: RegexSet,
    pattern_names: Vec<String>,
    individual_patterns: Vec<Regex>,
}

impl PiiDetector {
    /// Create a detector with default PII patterns.
    pub fn new() -> Self {
        let (patterns, names) = Self::default_patterns();
        let regex_set = RegexSet::new(&patterns).expect("invalid PII regex patterns");
        let individual = patterns
            .iter()
            .map(|p| Regex::new(p).expect("invalid individual PII regex"))
            .collect();
        Self {
            patterns: regex_set,
            pattern_names: names,
            individual_patterns: individual,
        }
    }

    /// Scan text for PII and return detected types.
    pub fn detect(&self, text: &str) -> Vec<String> {
        self.patterns
            .matches(text)
            .into_iter()
            .map(|idx| self.pattern_names[idx].clone())
            .collect()
    }

    /// Check if text contains any PII.
    pub fn contains_pii(&self, text: &str) -> bool {
        self.patterns.is_match(text)
    }

    /// Redact all PII in text, replacing matches with `[REDACTED:<type>]`.
    ///
    /// Uses individual regex patterns for precise locate-and-replace.
    pub fn redact(&self, text: &str) -> String {
        let matching_indices: Vec<usize> = self.patterns.matches(text).into_iter().collect();
        if matching_indices.is_empty() {
            return text.to_string();
        }
        let mut result = text.to_string();
        // Apply replacements for each matching pattern.
        // Process in reverse index order so replacements don't shift positions of later patterns.
        for &idx in matching_indices.iter().rev() {
            let re = &self.individual_patterns[idx];
            let name = &self.pattern_names[idx];
            result = re
                .replace_all(&result, format!("[REDACTED:{name}]"))
                .to_string();
        }
        result
    }

    fn default_patterns() -> (Vec<String>, Vec<String>) {
        let patterns: Vec<(&str, &str)> = vec![
            // Email
            (r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}", "email"),
            // Phone (international)
            (r"\+?[1-9]\d{1,14}", "phone"),
            // SSN (US)
            (r"\b\d{3}-\d{2}-\d{4}\b", "ssn"),
            // Credit card (basic Luhn-eligible)
            (r"\b(?:\d{4}[- ]?){3}\d{4}\b", "credit_card"),
            // IPv4
            (r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "ipv4"),
            // AWS access key
            (r"AKIA[0-9A-Z]{16}", "aws_access_key"),
            // Generic API key pattern
            (
                r#"(?i)(api[_-]?key|secret[_-]?key|access[_-]?token)\s*[:=]\s*['"]?[\w-]{20,}"#,
                "api_key",
            ),
            // SSH / PEM private key header
            (
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
                "private_key",
            ),
            // JWT token (three base64url segments)
            (
                r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
                "jwt_token",
            ),
            // GitHub personal access token (classic)
            (r"ghp_[A-Za-z0-9]{36}", "github_token"),
            // GitHub fine-grained personal access token
            (
                r"github_pat_[A-Za-z0-9]{22}_[A-Za-z0-9]{59}",
                "github_fine_grained_token",
            ),
            // Anthropic API key
            (r"sk-ant-api\d{2}-[A-Za-z0-9_-]{80,}", "anthropic_api_key"),
        ];

        let (regexes, names): (Vec<_>, Vec<_>) = patterns
            .into_iter()
            .map(|(p, n)| (p.to_string(), n.to_string()))
            .unzip();

        (regexes, names)
    }
}

impl Default for PiiDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_email() {
        let detector = PiiDetector::new();
        let text = "contact me at user@example.com ok";
        let result = detector.detect(text);
        assert!(result.contains(&"email".to_string()));
    }

    #[test]
    fn detects_ssn() {
        let detector = PiiDetector::new();
        assert!(detector.contains_pii("my SSN is 123-45-6789"));
    }

    #[test]
    fn detects_aws_key() {
        let detector = PiiDetector::new();
        let text = "key: AKIAIOSFODNN7EXAMPLE";
        let result = detector.detect(text);
        assert!(result.contains(&"aws_access_key".to_string()));
    }

    #[test]
    fn no_false_positive_on_clean_code() {
        let detector = PiiDetector::new();
        let text = r#"fn main() { println!("hello world"); }"#;
        let result = detector.detect(text);
        assert!(!result.contains(&"email".to_string()));
        assert!(!result.contains(&"ssn".to_string()));
        assert!(!result.contains(&"credit_card".to_string()));
        assert!(!result.contains(&"aws_access_key".to_string()));
    }

    // --- New pattern tests ---

    #[test]
    fn detects_ssh_private_key() {
        let detector = PiiDetector::new();
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAK...";
        assert!(detector.detect(text).contains(&"private_key".to_string()));

        let text2 = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1...";
        assert!(detector.detect(text2).contains(&"private_key".to_string()));

        let text3 = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgk...";
        assert!(detector.detect(text3).contains(&"private_key".to_string()));
    }

    #[test]
    fn detects_jwt_token() {
        let detector = PiiDetector::new();
        // A realistic-looking JWT (header.payload.signature)
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let result = detector.detect(jwt);
        assert!(result.contains(&"jwt_token".to_string()));
    }

    #[test]
    fn detects_github_pat() {
        let detector = PiiDetector::new();
        let text = "token: ghp_ABCDEFghijklmnopqrstuvwxyz0123456789";
        let result = detector.detect(text);
        assert!(result.contains(&"github_token".to_string()));
    }

    #[test]
    fn detects_github_fine_grained_pat() {
        let detector = PiiDetector::new();
        let text = "github_pat_1234567890ABCDEFghijkl_ABCDEFghijklmnopqrstuvwxyz0123456789ABCDEFghijklmnopqrstuvwxy";
        let result = detector.detect(text);
        assert!(result.contains(&"github_fine_grained_token".to_string()));
    }

    #[test]
    fn detects_anthropic_key() {
        let detector = PiiDetector::new();
        let key = format!("sk-ant-api03-{}", "A".repeat(95));
        let result = detector.detect(&key);
        assert!(
            result.contains(&"anthropic_api_key".to_string()),
            "should detect anthropic key, got: {result:?}"
        );
    }

    #[test]
    fn no_false_positive_on_base64_code() {
        let detector = PiiDetector::new();
        // Short base64 strings in code should not trigger JWT detection
        let text = r#"let encoded = base64::encode("hello");"#;
        let result = detector.detect(text);
        assert!(
            !result.contains(&"jwt_token".to_string()),
            "short base64 in code should not trigger JWT"
        );
    }

    // --- Redaction tests ---

    #[test]
    fn redact_replaces_email() {
        let detector = PiiDetector::new();
        let text = "Contact user@example.com for info";
        let redacted = detector.redact(text);
        assert!(
            redacted.contains("[REDACTED:email]"),
            "should redact email, got: {redacted}"
        );
        assert!(!redacted.contains("user@example.com"));
    }

    #[test]
    fn redact_replaces_ssh_key() {
        let detector = PiiDetector::new();
        let text = "key: -----BEGIN RSA PRIVATE KEY-----\ndata";
        let redacted = detector.redact(text);
        assert!(
            redacted.contains("[REDACTED:private_key]"),
            "should redact SSH key header, got: {redacted}"
        );
    }

    #[test]
    fn redact_preserves_clean_text() {
        let detector = PiiDetector::new();
        let text = "This is perfectly clean text with no secrets.";
        let redacted = detector.redact(text);
        assert_eq!(redacted, text);
    }

    #[test]
    fn redact_multiple_types() {
        let detector = PiiDetector::new();
        let text = "email: admin@corp.com, key: AKIAIOSFODNN7EXAMPLE";
        let redacted = detector.redact(text);
        assert!(redacted.contains("[REDACTED:email]"));
        assert!(redacted.contains("[REDACTED:aws_access_key]"));
        assert!(!redacted.contains("admin@corp.com"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }
}
