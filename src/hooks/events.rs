//! Claude Code hook event types.
//!
//! These types represent the JSON payloads that Claude Code sends
//! to hook handlers. Lobster captures them as `RawEvent` records.

use serde::{Deserialize, Serialize};

/// The hook lifecycle point at which the event fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookType {
    /// Before Claude generates a response to a user prompt.
    UserPromptSubmit,
    /// After a tool completes successfully.
    PostToolUse,
    /// After a tool fails.
    PostToolUseFailure,
    /// End of a notification turn.
    NotificationPost,
}

/// A hook event payload from Claude Code.
///
/// This is the raw JSON structure received from Claude Code's
/// hook system. It gets converted into a `RawEvent` for storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    pub hook_type: HookType,
    pub session_id: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_output: Option<serde_json::Value>,
    pub user_prompt: Option<String>,
    pub assistant_response: Option<String>,
    pub working_directory: Option<String>,
    pub timestamp_ms: i64,
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    #[hegel::composite]
    fn gen_hook_event(tc: hegel::TestCase) -> HookEvent {
        let hook_type: HookType = tc.draw(gs::sampled_from(vec![
            HookType::UserPromptSubmit,
            HookType::PostToolUse,
            HookType::PostToolUseFailure,
            HookType::NotificationPost,
        ]));
        HookEvent {
            hook_type,
            session_id: tc.draw(gs::text().min_size(1).max_size(36)),
            tool_name: if hook_type == HookType::PostToolUse
                || hook_type == HookType::PostToolUseFailure
            {
                Some(tc.draw(gs::text().min_size(1).max_size(50)))
            } else {
                None
            },
            tool_input: None,
            tool_output: None,
            user_prompt: if hook_type == HookType::UserPromptSubmit {
                Some(tc.draw(gs::text().min_size(1).max_size(200)))
            } else {
                None
            },
            assistant_response: None,
            working_directory: Some(
                tc.draw(gs::text().min_size(1).max_size(100)),
            ),
            timestamp_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
        }
    }

    // -- Property: HookEvent serde round-trip --
    #[hegel::test(test_cases = 200)]
    fn prop_hook_event_serde_roundtrip(tc: TestCase) {
        let event = tc.draw(gen_hook_event());
        let json = serde_json::to_string(&event).unwrap();
        let parsed: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn test_parse_user_prompt_event() {
        let json = r#"{
            "hook_type": "UserPromptSubmit",
            "session_id": "abc-123",
            "tool_name": null,
            "tool_input": null,
            "tool_output": null,
            "user_prompt": "Fix the bug in main.rs",
            "assistant_response": null,
            "working_directory": "/home/user/project",
            "timestamp_ms": 1700000000000
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.hook_type, HookType::UserPromptSubmit);
        assert_eq!(
            event.user_prompt.as_deref(),
            Some("Fix the bug in main.rs")
        );
    }
}
