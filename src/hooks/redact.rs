//! Deterministic redaction filter for secrets and sensitive content.
//!
//! Filtering decisions are deterministic, logged, and repo-configurable.

/// Result of running the redaction filter on a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactResult {
    /// Payload is clean — no redaction needed.
    Clean,
    /// Payload was redacted — contains the cleaned version.
    Redacted(String),
    /// Payload should be dropped entirely (e.g., binary blob).
    Dropped(String),
}

/// Patterns that indicate secrets or sensitive content.
const SECRET_PATTERNS: &[&str] = &[
    "AKIA",           // AWS access key prefix
    "sk-",            // OpenAI/Stripe key prefix
    "ghp_",           // GitHub PAT prefix
    "gho_",           // GitHub OAuth prefix
    "Bearer ",        // Auth header
    "Authorization:", // Auth header
    "password=",
    "secret=",
    "token=",
    "api_key=",
    "apikey=",
    "-----BEGIN", // PEM certificates/keys
];

/// File patterns that should be ignored entirely.
const SENSITIVE_FILE_PATTERNS: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    "credentials.json",
    "secrets.yaml",
    "secrets.yml",
    "id_rsa",
    "id_ed25519",
    ".pem",
    ".key",
];

/// Check if a payload contains secret-like patterns.
#[must_use]
pub fn scan_for_secrets(text: &str) -> Vec<&'static str> {
    SECRET_PATTERNS
        .iter()
        .filter(|pat| text.contains(**pat))
        .copied()
        .collect()
}

/// Check if a file path matches sensitive file patterns.
#[must_use]
pub fn is_sensitive_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    SENSITIVE_FILE_PATTERNS
        .iter()
        .any(|pat| lower.ends_with(pat) || lower.contains(&format!("{pat}/")))
}

/// Redact a text payload by replacing detected secrets with
/// `[REDACTED]`.
#[must_use]
pub fn redact_payload(text: &str) -> RedactResult {
    let secrets = scan_for_secrets(text);
    if secrets.is_empty() {
        return RedactResult::Clean;
    }

    let mut redacted = text.to_string();
    for pattern in &secrets {
        // Replace the line containing the pattern
        let lines: Vec<&str> = redacted.lines().collect();
        let cleaned: Vec<String> = lines
            .iter()
            .map(|line| {
                if line.contains(pattern) {
                    "[REDACTED]".to_string()
                } else {
                    (*line).to_string()
                }
            })
            .collect();
        redacted = cleaned.join("\n");
    }

    RedactResult::Redacted(redacted)
}

/// Check if a path should be ignored based on ignore rules.
#[must_use]
pub fn should_ignore_path(path: &str, ignore_patterns: &[String]) -> bool {
    if is_sensitive_path(path) {
        return true;
    }
    let lower = path.to_lowercase();
    ignore_patterns
        .iter()
        .any(|pat| lower.contains(&pat.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: redaction is deterministic --
    #[hegel::test(test_cases = 200)]
    fn prop_redact_deterministic(tc: TestCase) {
        let text: String = tc.draw(gs::text().max_size(500));
        let r1 = redact_payload(&text);
        let r2 = redact_payload(&text);
        assert_eq!(r1, r2);
    }

    // -- Property: clean text stays clean --
    #[hegel::test(test_cases = 200)]
    fn prop_clean_text_not_redacted(tc: TestCase) {
        // Generate text that won't contain secret patterns
        let text: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(100)
                .alphabet("abcdefghijklmnopqrstuvwxyz "),
        );
        assert_eq!(redact_payload(&text), RedactResult::Clean);
    }

    // -- Unit tests --
    #[test]
    fn test_detects_aws_key() {
        let secrets = scan_for_secrets("key=AKIAIOSFODNN7EXAMPLE");
        assert!(!secrets.is_empty());
        assert!(secrets.contains(&"AKIA"));
    }

    #[test]
    fn test_detects_openai_key() {
        let secrets = scan_for_secrets("sk-proj-abc123");
        assert!(secrets.contains(&"sk-"));
    }

    #[test]
    fn test_redacts_line_with_secret() {
        let text = "normal line\nkey=sk-secret123\nanother line";
        match redact_payload(text) {
            RedactResult::Redacted(cleaned) => {
                assert!(cleaned.contains("[REDACTED]"));
                assert!(!cleaned.contains("sk-secret123"));
                assert!(cleaned.contains("normal line"));
            }
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn test_sensitive_paths() {
        assert!(is_sensitive_path(".env"));
        assert!(is_sensitive_path("config/.env.local"));
        assert!(is_sensitive_path("secrets.yaml"));
        assert!(is_sensitive_path("id_rsa"));
        assert!(!is_sensitive_path("src/main.rs"));
        assert!(!is_sensitive_path("Cargo.toml"));
    }

    #[test]
    fn test_ignore_custom_patterns() {
        let patterns = vec!["node_modules".to_string(), "target/".to_string()];
        assert!(should_ignore_path("node_modules/foo", &patterns));
        assert!(should_ignore_path("target/debug/build", &patterns));
        assert!(!should_ignore_path("src/main.rs", &patterns));
    }
}
