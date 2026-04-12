//! Hook-level episode segmentation.
//!
//! Since hooks are short-lived processes, segmentation checks
//! the last event in redb and decides whether the new event
//! starts a new episode or extends the current one.

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::{
    episodes::segmenter::SegmentationConfig,
    store::{ids::RepoId, schema::RawEvent, tables},
};

/// Result of the segmentation check.
#[derive(Debug, PartialEq, Eq)]
pub enum SegmentAction {
    /// Extend the current episode (no boundary detected).
    ExtendCurrent { start_seq: u64, end_seq: u64 },
    /// Start a new episode (boundary detected).
    StartNew { seq: u64 },
}

/// Check whether a new event should extend the current episode
/// or start a new one, based on idle gap and repo transition.
#[must_use]
pub fn check_segmentation(
    db: &Database,
    new_event_ts_ms: i64,
    new_repo_id: &RepoId,
    new_seq: u64,
    config: &SegmentationConfig,
) -> SegmentAction {
    // Find the most recent event in redb
    let last_event = get_last_event(db);

    let Some(last) = last_event else {
        // No previous events — this is the first event ever
        return SegmentAction::StartNew { seq: new_seq };
    };

    // Check idle gap
    let gap_ms = new_event_ts_ms - last.ts_utc_ms;
    if gap_ms >= config.idle_gap_ms {
        return SegmentAction::StartNew { seq: new_seq };
    }

    // Check repo transition
    if last.repo_id != *new_repo_id {
        return SegmentAction::StartNew { seq: new_seq };
    }

    // No boundary — extend the current episode
    // Find the start of the current episode by walking back
    // from the last event to find where the previous boundary was
    let start_seq = find_episode_start(db, &last, config);

    SegmentAction::ExtendCurrent {
        start_seq,
        end_seq: new_seq,
    }
}

/// Get the most recent raw event from redb.
fn get_last_event(db: &Database) -> Option<RawEvent> {
    let read_txn = db.begin_read().ok()?;
    let table = read_txn.open_table(tables::RAW_EVENTS).ok()?;
    let (_, value) = table.last().ok()??;
    serde_json::from_slice(value.value()).ok()
}

/// Walk backwards from a given event to find where the current
/// episode started (i.e., where the last boundary was).
fn find_episode_start(
    db: &Database,
    last: &RawEvent,
    config: &SegmentationConfig,
) -> u64 {
    let Ok(read_txn) = db.begin_read() else {
        return last.seq;
    };
    let Ok(table) = read_txn.open_table(tables::RAW_EVENTS) else {
        return last.seq;
    };

    let mut start_seq = last.seq;
    let mut prev_ts = last.ts_utc_ms;
    let mut prev_repo = last.repo_id;

    // Walk backwards through events
    if last.seq == 0 {
        return 0;
    }
    for seq in (0..last.seq).rev() {
        let Ok(Some(guard)) = table.get(seq) else {
            break;
        };
        let Ok(event) = serde_json::from_slice::<RawEvent>(guard.value())
        else {
            break;
        };

        // Check if this event is a boundary
        let gap = prev_ts - event.ts_utc_ms;
        if gap >= config.idle_gap_ms || event.repo_id != prev_repo {
            break;
        }

        start_seq = seq;
        prev_ts = event.ts_utc_ms;
        prev_repo = event.repo_id;
    }

    start_seq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hooks::{
            capture,
            events::{HookEvent, HookType},
        },
        store::db,
    };

    fn make_event(prompt: &str, ts: i64, dir: &str) -> HookEvent {
        HookEvent {
            hook_type: HookType::UserPromptSubmit,
            session_id: "test".into(),
            tool_name: None,
            tool_input: None,
            tool_output: None,
            user_prompt: Some(prompt.into()),
            assistant_response: None,
            working_directory: Some(dir.into()),
            timestamp_ms: ts,
        }
    }

    #[test]
    fn test_first_event_starts_new() {
        let database = db::open_in_memory().unwrap();
        let config = SegmentationConfig::default();
        let repo = RepoId::derive(b"/project");

        let action =
            check_segmentation(&database, 1_700_000_000_000, &repo, 0, &config);
        assert_eq!(action, SegmentAction::StartNew { seq: 0 });
    }

    #[test]
    fn test_close_events_extend() {
        let database = db::open_in_memory().unwrap();
        let config = SegmentationConfig {
            idle_gap_ms: 5 * 60 * 1000, // 5 min
        };

        // Capture first event
        let ev1 = make_event("first", 1_700_000_000_000, "/project");
        capture::capture_event(&database, &ev1, 0).unwrap();

        // Second event 10 seconds later — should extend
        let repo = RepoId::derive(b"/project");
        let action =
            check_segmentation(&database, 1_700_000_010_000, &repo, 1, &config);
        assert!(
            matches!(action, SegmentAction::ExtendCurrent { .. }),
            "close events should extend: {action:?}"
        );
    }

    #[test]
    fn test_idle_gap_starts_new() {
        let database = db::open_in_memory().unwrap();
        let config = SegmentationConfig {
            idle_gap_ms: 5 * 60 * 1000,
        };

        let ev1 = make_event("first", 1_700_000_000_000, "/project");
        capture::capture_event(&database, &ev1, 0).unwrap();

        // Second event 10 minutes later — should start new
        let repo = RepoId::derive(b"/project");
        let action = check_segmentation(
            &database,
            1_700_000_600_000, // 10 min gap
            &repo,
            1,
            &config,
        );
        assert_eq!(action, SegmentAction::StartNew { seq: 1 });
    }

    #[test]
    fn test_repo_transition_starts_new() {
        let database = db::open_in_memory().unwrap();
        let config = SegmentationConfig::default();

        let ev1 = make_event("first", 1_700_000_000_000, "/project-a");
        capture::capture_event(&database, &ev1, 0).unwrap();

        // Same time but different repo
        let repo_b = RepoId::derive(b"/project-b");
        let action = check_segmentation(
            &database,
            1_700_000_001_000,
            &repo_b,
            1,
            &config,
        );
        assert_eq!(action, SegmentAction::StartNew { seq: 1 });
    }
}
