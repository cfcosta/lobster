//! Generate Claude Code hook configuration for Lobster.
//!
//! Produces the JSON hook entries that tell Claude Code to call
//! `lobster hook` on `UserPromptSubmit`, `PostToolUse`, etc.

use serde::{Deserialize, Serialize};

/// A single hook entry for Claude Code settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub r#type: String,
    pub command: String,
}

/// Generate the hook configuration for Lobster.
///
/// Returns hook entries that should be added to
/// `.claude/settings.json` under the `hooks` key.
#[must_use]
pub fn generate_hook_config(lobster_path: &str) -> Vec<HookEntry> {
    vec![
        HookEntry {
            r#type: "UserPromptSubmit".into(),
            command: format!("{lobster_path} hook UserPromptSubmit"),
        },
        HookEntry {
            r#type: "PostToolUse".into(),
            command: format!("{lobster_path} hook PostToolUse"),
        },
        HookEntry {
            r#type: "PostToolUseFailure".into(),
            command: format!("{lobster_path} hook PostToolUseFailure"),
        },
    ]
}

/// Serialize the hook config as JSON suitable for
/// `.claude/settings.json`.
///
/// # Errors
///
/// Returns a serde error if serialization fails.
pub fn to_json(lobster_path: &str) -> Result<String, serde_json::Error> {
    let hooks = generate_hook_config(lobster_path);
    serde_json::to_string_pretty(&hooks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_three_hooks() {
        let hooks = generate_hook_config("lobster");
        assert_eq!(hooks.len(), 3);
        assert_eq!(hooks[0].r#type, "UserPromptSubmit");
        assert_eq!(hooks[1].r#type, "PostToolUse");
        assert_eq!(hooks[2].r#type, "PostToolUseFailure");
    }

    #[test]
    fn test_uses_lobster_path() {
        let hooks = generate_hook_config("/usr/local/bin/lobster");
        assert!(hooks[0].command.starts_with("/usr/local/bin/lobster"));
    }

    #[test]
    fn test_serializes_to_json() {
        let json = to_json("lobster").unwrap();
        assert!(json.contains("UserPromptSubmit"));
        assert!(json.contains("PostToolUse"));
        // Should be valid JSON
        let parsed: Vec<HookEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
    }
}
