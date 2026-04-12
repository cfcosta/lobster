//! Claude Code hook event types.
//!
//! These types match the actual JSON payload format that Claude
//! Code sends to hook commands via stdin.

use serde::{Deserialize, Serialize};

/// A hook event payload from Claude Code.
///
/// This is the actual JSON structure received on stdin. The format
/// varies by hook type but always has `hook_event_name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// The hook event type (e.g., "`PreToolUse`", "`PostToolUse`",
    /// "`UserPromptSubmit`", "Stop").
    #[serde(default)]
    pub hook_event_name: String,

    /// Tool name (for tool-related hooks).
    #[serde(default)]
    pub tool_name: Option<String>,

    /// Tool input (varies by tool — e.g., `command` for Bash,
    /// `file_path`/`new_text` for Edit/Write).
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,

    /// Session ID.
    #[serde(default)]
    pub session_id: Option<String>,

    /// Stop reason (for Stop hooks).
    #[serde(default)]
    pub reason: Option<String>,

    /// Transcript file path (for Stop hooks).
    #[serde(default)]
    pub transcript_path: Option<String>,

    /// All other fields are captured here for forward compat.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl HookEvent {
    /// Get the hook type as a normalized enum-like string.
    #[must_use]
    pub fn hook_type(&self) -> &str {
        &self.hook_event_name
    }

    /// Extract the user prompt text if this is a
    /// `UserPromptSubmit` event.
    #[must_use]
    pub fn user_prompt(&self) -> Option<String> {
        // The prompt may be in tool_input or in extra fields
        if let Some(input) = &self.tool_input {
            if let Some(prompt) = input.get("prompt") {
                return prompt.as_str().map(String::from);
            }
            if let Some(content) = input.get("content") {
                return content.as_str().map(String::from);
            }
        }
        // Check extra fields
        self.extra
            .get("prompt")
            .or_else(|| self.extra.get("content"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// Get the working directory from the environment or input.
    #[must_use]
    pub fn working_directory(&self) -> Option<String> {
        self.extra
            .get("cwd")
            .or_else(|| self.extra.get("working_directory"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                std::env::current_dir().ok()?.to_str().map(String::from)
            })
    }

    /// Check if this is a `UserPromptSubmit` event.
    #[must_use]
    pub fn is_prompt_submit(&self) -> bool {
        self.hook_event_name == "UserPromptSubmit"
    }

    /// Check if this is a tool use event.
    #[must_use]
    pub fn is_tool_use(&self) -> bool {
        self.hook_event_name == "PostToolUse"
            || self.hook_event_name == "PreToolUse"
    }

    /// Check if this is a tool failure event.
    #[must_use]
    pub fn is_tool_failure(&self) -> bool {
        self.hook_event_name == "PostToolUseFailure"
    }
}

/// Hook output: what we write to stdout.
///
/// Claude Code expects a JSON object. An empty `{}` means "no
/// action". A `systemMessage` field injects text into the
/// conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOutput {
    /// Text injected into the conversation as a system message.
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

impl HookOutput {
    /// Empty output — no action.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            system_message: None,
        }
    }

    /// Output with a system message for recall hints.
    #[must_use]
    pub const fn with_message(msg: String) -> Self {
        Self {
            system_message: Some(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_prompt_event() {
        let json = r#"{
            "hook_event_name": "UserPromptSubmit",
            "tool_name": null,
            "tool_input": {"prompt": "Fix the bug in main.rs"},
            "session_id": "abc-123"
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_prompt_submit());
        assert_eq!(
            event.user_prompt().as_deref(),
            Some("Fix the bug in main.rs")
        );
    }

    #[test]
    fn test_parse_tool_use_event() {
        let json = r#"{
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "src/main.rs", "content": "fn main() {}"}
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_tool_use());
        assert_eq!(event.tool_name.as_deref(), Some("Write"));
    }

    #[test]
    fn test_parse_unknown_fields_preserved() {
        let json = r#"{
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/home/user/project",
            "custom_field": "value"
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(
            event.working_directory().as_deref(),
            Some("/home/user/project")
        );
    }

    #[test]
    fn test_hook_output_empty() {
        let out = HookOutput::empty();
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_hook_output_with_message() {
        let out = HookOutput::with_message("Prior decision: use redb".into());
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("systemMessage"));
        assert!(json.contains("use redb"));
    }
}
