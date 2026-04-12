//! Event capture: convert hook events into raw events and persist.
//!
//! This is the entry point for ALL data into Lobster. Every hook
//! event is appended to redb as a `RawEvent`.

use redb::Database;
use sha2::{Digest, Sha256};

use crate::{
    hooks::{
        events::{HookEvent, HookType},
        redact,
    },
    store::{
        crud,
        ids::RepoId,
        schema::{EventKind, RawEvent},
    },
};

/// Convert a hook event into a raw event and persist it.
///
/// Returns the sequence number assigned to the event, or an error.
///
/// # Errors
///
/// Returns `StoreError` if persistence fails.
pub fn capture_event(
    db: &Database,
    event: &HookEvent,
    seq: u64,
) -> Result<u64, crud::StoreError> {
    let repo_id = event.working_directory.as_deref().map_or_else(
        || RepoId::derive(b"unknown"),
        |d| RepoId::derive(d.as_bytes()),
    );

    let event_kind = match event.hook_type {
        HookType::UserPromptSubmit => EventKind::UserPromptSubmit,
        HookType::PostToolUse => EventKind::ToolUse,
        HookType::PostToolUseFailure => EventKind::ToolUseFailure,
        HookType::NotificationPost => EventKind::AssistantResponse,
    };

    // Serialize the event payload
    let payload_json = serde_json::to_vec(event).unwrap_or_default();

    // Check for secrets and redact if needed
    let payload_str = String::from_utf8_lossy(&payload_json);
    let payload_bytes = match redact::redact_payload(&payload_str) {
        redact::RedactResult::Clean => payload_json,
        redact::RedactResult::Redacted(cleaned) => cleaned.into_bytes(),
        redact::RedactResult::Dropped(_) => {
            return Ok(seq); // Skip this event
        }
    };

    // Compute payload hash
    let mut hasher = Sha256::new();
    hasher.update(&payload_bytes);
    let hash: [u8; 32] = hasher.finalize().into();

    let raw_event = RawEvent {
        seq,
        repo_id,
        ts_utc_ms: event.timestamp_ms,
        event_kind,
        payload_hash: hash,
        payload_bytes,
    };

    crud::append_raw_event(db, &raw_event)?;
    Ok(seq)
}

/// Get the next sequence number by scanning existing events.
///
/// In a real system this would use an atomic counter, but for
/// the single-writer hook model, scanning the last key works.
#[must_use]
pub fn next_seq(db: &Database) -> u64 {
    use redb::{ReadableDatabase, ReadableTable};

    let Ok(read_txn) = db.begin_read() else {
        return 0;
    };
    let Ok(table) = read_txn.open_table(crate::store::tables::RAW_EVENTS)
    else {
        return 0;
    };
    let Ok(last) = table.last() else {
        return 0;
    };

    last.map_or(0, |(k, _)| k.value() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db;

    fn make_event(prompt: &str) -> HookEvent {
        HookEvent {
            hook_type: HookType::UserPromptSubmit,
            session_id: "test".into(),
            tool_name: None,
            tool_input: None,
            tool_output: None,
            user_prompt: Some(prompt.into()),
            assistant_response: None,
            working_directory: Some("/test/repo".into()),
            timestamp_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn test_capture_persists_event() {
        let database = db::open_in_memory().unwrap();
        let event = make_event("fix the bug");

        let seq = capture_event(&database, &event, 0).unwrap();
        assert_eq!(seq, 0);

        // Read it back
        let loaded = crud::get_raw_event(&database, 0).unwrap();
        assert_eq!(loaded.event_kind, EventKind::UserPromptSubmit);
        assert_ne!(loaded.payload_hash, [0; 32]);
        assert!(!loaded.payload_bytes.is_empty());
    }

    #[test]
    fn test_capture_multiple_events() {
        let database = db::open_in_memory().unwrap();

        capture_event(&database, &make_event("first"), 0).unwrap();
        capture_event(&database, &make_event("second"), 1).unwrap();

        let e0 = crud::get_raw_event(&database, 0).unwrap();
        let e1 = crud::get_raw_event(&database, 1).unwrap();

        // Different payloads should have different hashes
        assert_ne!(e0.payload_hash, e1.payload_hash);
    }

    #[test]
    fn test_next_seq_empty_db() {
        let database = db::open_in_memory().unwrap();
        assert_eq!(next_seq(&database), 0);
    }

    #[test]
    fn test_next_seq_after_capture() {
        let database = db::open_in_memory().unwrap();
        capture_event(&database, &make_event("test"), 0).unwrap();
        assert_eq!(next_seq(&database), 1);

        capture_event(&database, &make_event("test2"), 1).unwrap();
        assert_eq!(next_seq(&database), 2);
    }

    #[test]
    fn test_capture_redacts_secrets() {
        let database = db::open_in_memory().unwrap();
        let mut event = make_event("fix the bug");
        // Inject a secret-like pattern in the prompt
        event.user_prompt = Some("key=sk-secret123abc".into());

        capture_event(&database, &event, 0).unwrap();

        let loaded = crud::get_raw_event(&database, 0).unwrap();
        let payload_str = String::from_utf8_lossy(&loaded.payload_bytes);
        // The redaction happens on the JSON payload, so the
        // raw bytes should contain [REDACTED] for lines with
        // the secret pattern
        assert!(
            !payload_str.contains("sk-secret123abc")
                || payload_str.contains("[REDACTED]"),
            "secret should be redacted or removed"
        );
    }

    use hegel::{TestCase, generators as gs};

    /// Property: capture always produces a valid `RawEvent` that
    /// can be read back.
    #[hegel::test(test_cases = 50)]
    fn prop_capture_roundtrip(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let prompt: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(100)
                .alphabet("abcdefghijklmnopqrstuvwxyz "),
        );
        let event = make_event(&prompt);
        let seq: u64 =
            tc.draw(gs::integers::<u64>().min_value(0).max_value(10000));

        capture_event(&database, &event, seq).unwrap();
        let loaded = crud::get_raw_event(&database, seq).unwrap();
        assert_eq!(loaded.seq, seq);
        assert_eq!(loaded.event_kind, EventKind::UserPromptSubmit);
    }
}
