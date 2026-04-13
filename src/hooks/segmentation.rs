//! Hook-level episode segmentation.
//!
//! Since hooks are short-lived processes, segmentation checks
//! the last event in the database and decides whether the new
//! event starts a new episode or extends the current one.

use crate::{
    episodes::segmenter::SegmentationConfig,
    store::{db::LobsterDb, ids::RepoId, schema::RawEvent},
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
    db: &LobsterDb,
    new_event_ts_ms: i64,
    new_repo_id: &RepoId,
    new_seq: u64,
    config: &SegmentationConfig,
) -> SegmentAction {
    let last_event = get_last_event(db);

    let Some(last) = last_event else {
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

    let start_seq = find_episode_start(db, &last, config);

    SegmentAction::ExtendCurrent {
        start_seq,
        end_seq: new_seq,
    }
}

/// Get the most recent raw event.
fn get_last_event(db: &LobsterDb) -> Option<RawEvent> {
    let rtxn = db.env.read_txn().ok()?;
    let (_, value) = db.raw_events.last(&rtxn).ok()??;
    serde_json::from_slice(value).ok()
}

/// Walk backwards from a given event to find where the current
/// episode started (i.e., where the last boundary was).
fn find_episode_start(
    db: &LobsterDb,
    last: &RawEvent,
    config: &SegmentationConfig,
) -> u64 {
    let Ok(rtxn) = db.env.read_txn() else {
        return last.seq;
    };

    let mut start_seq = last.seq;
    let mut prev_ts = last.ts_utc_ms;
    let mut prev_repo = last.repo_id;

    if last.seq == 0 {
        return 0;
    }
    for seq in (0..last.seq).rev() {
        let Ok(Some(bytes)) = db.raw_events.get(&rtxn, &seq) else {
            break;
        };
        let Ok(event) = serde_json::from_slice::<RawEvent>(bytes) else {
            break;
        };

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
    use crate::store::{
        crud,
        db::{self, LobsterDb},
        ids::RepoId,
        schema::{EventKind, RawEvent},
    };

    fn insert_event(database: &LobsterDb, seq: u64, ts: i64, repo: &[u8]) {
        let event = RawEvent {
            seq,
            repo_id: RepoId::derive(repo),
            ts_utc_ms: ts,
            event_kind: EventKind::UserPromptSubmit,
            payload_hash: [0; 32],
            payload_bytes: vec![],
        };
        crud::append_raw_event(database, &event).unwrap();
    }

    #[test]
    fn test_first_event_starts_new() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = SegmentationConfig::default();
        let repo = RepoId::derive(b"/project");

        let action =
            check_segmentation(&database, 1_700_000_000_000, &repo, 0, &config);
        assert_eq!(action, SegmentAction::StartNew { seq: 0 });
    }

    #[test]
    fn test_close_events_extend() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = SegmentationConfig {
            idle_gap_ms: 5 * 60 * 1000,
        };

        insert_event(&database, 0, 1_700_000_000_000, b"/project");

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
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = SegmentationConfig {
            idle_gap_ms: 5 * 60 * 1000,
        };

        insert_event(&database, 0, 1_700_000_000_000, b"/project");

        let repo = RepoId::derive(b"/project");
        let action =
            check_segmentation(&database, 1_700_000_600_000, &repo, 1, &config);
        assert_eq!(action, SegmentAction::StartNew { seq: 1 });
    }

    #[test]
    fn test_repo_transition_starts_new() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = SegmentationConfig::default();

        insert_event(&database, 0, 1_700_000_000_000, b"/project-a");

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
