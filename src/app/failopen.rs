//! Fail-open behavior for Lobster.
//!
//! When Lobster is degraded, Claude Code continues normally.
//! The memory system never blocks normal coding flow.

use std::path::Path;

use crate::hooks::{events::HookEvent, recall::RecallPayload};

/// Run the hook handler with fail-open semantics.
///
/// If anything goes wrong (database missing, parse error, etc.),
/// returns an empty recall payload with a warning rather than
/// an error. Claude Code should never be blocked by Lobster.
#[must_use]
pub fn run_hook_failopen(
    storage_dir: &Path,
    input_json: &str,
) -> RecallPayload {
    // Parse event — fail open on parse error
    let event: HookEvent = match serde_json::from_str(input_json) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to parse hook event, returning empty"
            );
            return empty_payload();
        }
    };

    // Open database — fail open if missing or corrupted
    let db_path = crate::app::config::db_path(storage_dir);
    let db = match crate::store::db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to open database, returning empty"
            );
            return empty_payload();
        }
    };

    let grafeo = crate::graph::db::new_in_memory();

    // Run recall — fail open on any error
    crate::hooks::recall::run_recall(&event, &db, &grafeo)
}

/// Empty payload with no recall items and zero latency.
#[must_use]
pub const fn empty_payload() -> RecallPayload {
    RecallPayload {
        items: vec![],
        truncated: None,
        latency_ms: 0,
    }
}

/// Check if Lobster is initialized for a given storage dir.
#[must_use]
pub fn is_initialized(storage_dir: &Path) -> bool {
    crate::app::config::db_path(storage_dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failopen_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let payload = run_hook_failopen(dir.path(), "not valid json");
        assert!(payload.items.is_empty());
    }

    #[test]
    fn test_failopen_on_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let event_json = r#"{
            "hook_type": "UserPromptSubmit",
            "session_id": "test",
            "tool_name": null,
            "tool_input": null,
            "tool_output": null,
            "user_prompt": "hello",
            "assistant_response": null,
            "working_directory": "/test",
            "timestamp_ms": 0
        }"#;

        // Database doesn't exist yet, but should not crash
        let payload = run_hook_failopen(dir.path(), event_json);
        // This will actually create the database (open creates it)
        // so the payload will be valid (just empty results)
        assert!(payload.items.is_empty());
    }

    #[test]
    fn test_is_initialized_false_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_initialized(dir.path()));
    }

    #[test]
    fn test_is_initialized_true_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join(".lobster");
        std::fs::create_dir_all(&storage).unwrap();
        let db_path = crate::app::config::db_path(&storage);
        crate::store::db::open(&db_path).unwrap();
        assert!(is_initialized(&storage));
    }

    #[test]
    fn test_empty_payload() {
        let p = empty_payload();
        assert!(p.items.is_empty());
        assert!(p.truncated.is_none());
        assert_eq!(p.latency_ms, 0);
    }
}
