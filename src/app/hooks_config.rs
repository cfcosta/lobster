//! Generate Claude Code hook configuration for Lobster.
//!
//! Produces JSON matching the real Claude Code hooks.json format.

use serde::{Deserialize, Serialize};

/// A hook command entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommand {
    pub r#type: String,
    pub command: String,
    pub timeout: u32,
}

/// A hook event group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventGroup {
    pub hooks: Vec<HookCommand>,
}

/// The full hooks configuration matching Claude Code's format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(rename = "UserPromptSubmit")]
    pub user_prompt_submit: Vec<HookEventGroup>,
    #[serde(rename = "PostToolUse")]
    pub post_tool_use: Vec<HookEventGroup>,
}

/// Generate the hooks configuration.
#[must_use]
pub fn generate(lobster_path: &str) -> HooksConfig {
    let make_group = |event: &str| -> HookEventGroup {
        HookEventGroup {
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: format!("{lobster_path} hook {event}"),
                timeout: 10,
            }],
        }
    };

    HooksConfig {
        user_prompt_submit: vec![make_group("UserPromptSubmit")],
        post_tool_use: vec![make_group("PostToolUse")],
    }
}

/// Serialize as JSON for `.claude/settings.json`.
///
/// # Errors
///
/// Returns a serde error if serialization fails.
pub fn to_json(lobster_path: &str) -> Result<String, serde_json::Error> {
    let config = generate(lobster_path);
    serde_json::to_string_pretty(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_config() {
        let config = generate("lobster");
        assert_eq!(config.user_prompt_submit.len(), 1);
        assert_eq!(config.post_tool_use.len(), 1);
        assert_eq!(config.user_prompt_submit[0].hooks[0].r#type, "command");
    }

    #[test]
    fn test_serializes_to_real_format() {
        let json = to_json("lobster").unwrap();
        assert!(json.contains("UserPromptSubmit"));
        assert!(json.contains("PostToolUse"));
        assert!(json.contains("\"type\": \"command\""));
        assert!(json.contains("\"timeout\": 10"));
    }
}
