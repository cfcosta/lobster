//! Ingestion pipeline: processes staged events into redb.
//!
//! This is the core loop that:
//! 1. Drains staged event files from the staging directory
//! 2. Parses each as a `HookEvent`
//! 3. Captures them into redb via the existing capture pipeline
//! 4. Runs segmentation and finalization when appropriate

use grafeo::GrafeoDB;

use crate::{
    hooks::{
        capture,
        events::HookEvent,
        segmentation::{self, SegmentAction},
    },
    store::{db::LobsterDb, staging},
};

/// Result of a single ingestion cycle.
#[derive(Debug, Default)]
pub struct IngestResult {
    /// Number of events successfully ingested.
    pub events_ingested: usize,
    /// Number of events that failed to parse.
    pub parse_errors: usize,
    /// Number of events that failed to capture.
    pub capture_errors: usize,
    /// Number of episodes finalized during this cycle.
    pub episodes_finalized: usize,
}

/// Ingest all staged events into redb.
///
/// Drains the staging directory, captures each event, and
/// opportunistically finalizes episodes. This is the main entry
/// point for the MCP server's ingestion loop.
pub async fn ingest_staged(
    storage_dir: &std::path::Path,
    db: &LobsterDb,
    grafeo: &GrafeoDB,
) -> IngestResult {
    let mut result = IngestResult::default();

    let events = match staging::drain_staged(storage_dir) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(error = %e, "failed to drain staged events");
            return result;
        }
    };

    if events.is_empty() {
        return result;
    }

    tracing::debug!(count = events.len(), "ingesting staged events");

    for event_json in events {
        let event: HookEvent = match serde_json::from_str(&event_json) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse staged event");
                result.parse_errors += 1;
                continue;
            }
        };

        let seq = capture::next_seq(db);
        if let Err(e) = capture::capture_event(db, &event, seq) {
            tracing::warn!(error = %e, "failed to capture staged event");
            result.capture_errors += 1;
            continue;
        }
        result.events_ingested += 1;

        // Finalize if this is a prompt submit and we have an LLM key
        if event.is_prompt_submit() && has_llm_key() {
            let finalized = try_finalize(&event, db, grafeo, seq).await;
            if finalized {
                result.episodes_finalized += 1;
            }
        }
    }

    if result.events_ingested > 0 {
        tracing::info!(
            ingested = result.events_ingested,
            finalized = result.episodes_finalized,
            errors = result.parse_errors + result.capture_errors,
            "ingestion cycle complete"
        );
    }

    result
}

/// Try to finalize an episode for the given event.
async fn try_finalize(
    event: &HookEvent,
    db: &LobsterDb,
    grafeo: &GrafeoDB,
    seq: u64,
) -> bool {
    let repo_path = event
        .working_directory()
        .unwrap_or_else(|| "unknown".to_string());
    let repo_id = crate::store::ids::RepoId::derive(repo_path.as_bytes());
    let config = crate::episodes::segmenter::SegmentationConfig::default();

    let action = segmentation::check_segmentation(
        db,
        chrono::Utc::now().timestamp_millis(),
        &repo_id,
        seq,
        &config,
    );

    let (start_seq, end_seq) = match action {
        SegmentAction::StartNew { seq: s } => (s, s),
        SegmentAction::ExtendCurrent { start_seq, end_seq } => {
            (start_seq, end_seq)
        }
    };

    // Load all raw events in the episode range so finalization
    // sees the complete episode content, not just the trigger event.
    let events_json = match crate::store::crud::get_raw_events_range(
        db, start_seq, end_seq,
    ) {
        Ok(raw_events) => {
            let payloads: Vec<&[u8]> = raw_events
                .iter()
                .map(|e| e.payload_bytes.as_slice())
                .collect();
            // Build a JSON array from the individual payloads
            let mut buf = Vec::new();
            buf.push(b'[');
            for (i, p) in payloads.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                buf.extend_from_slice(p);
            }
            buf.push(b']');
            buf
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load episode events");
            return false;
        }
    };

    let result = crate::episodes::finalize::finalize_episode(
        db,
        grafeo,
        &repo_path,
        &events_json,
        start_seq,
        end_seq,
        event.user_prompt(),
    )
    .await;

    match result {
        crate::episodes::finalize::FinalizeResult::Ready {
            episode_id,
            decisions_created,
        } => {
            tracing::debug!(
                %episode_id,
                decisions_created,
                "episode finalized via ingestion"
            );
            true
        }
        crate::episodes::finalize::FinalizeResult::RetryQueued {
            reason,
            ..
        } => {
            tracing::warn!(reason, "episode queued for retry via ingestion");
            false
        }
        crate::episodes::finalize::FinalizeResult::Failed(msg) => {
            tracing::debug!(msg, "finalization failed during ingestion");
            false
        }
    }
}

fn has_llm_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graph::db as grafeo_db,
        store::{db, staging},
    };

    fn make_tool_json(tool: &str) -> String {
        serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "tool_input": {"file_path": "test.rs"},
            "cwd": "/test/repo",
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_ingest_empty_staging() {
        let dir = tempfile::tempdir().unwrap();
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let result = ingest_staged(dir.path(), &database, &grafeo).await;
        assert_eq!(result.events_ingested, 0);
        assert_eq!(result.parse_errors, 0);
    }

    #[tokio::test]
    async fn test_ingest_captures_events() {
        let dir = tempfile::tempdir().unwrap();
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Stage two events
        staging::stage_event(dir.path(), &make_tool_json("Write")).unwrap();
        staging::stage_event(dir.path(), &make_tool_json("Read")).unwrap();

        let result = ingest_staged(dir.path(), &database, &grafeo).await;
        assert_eq!(result.events_ingested, 2);
        assert_eq!(result.parse_errors, 0);

        // Verify events are in redb
        let e0 = crate::store::crud::get_raw_event(&database, 0).unwrap();
        assert_eq!(e0.event_kind, crate::store::schema::EventKind::ToolUse);
    }

    #[tokio::test]
    async fn test_ingest_handles_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        staging::stage_event(dir.path(), "not valid json").unwrap();
        staging::stage_event(dir.path(), &make_tool_json("Write")).unwrap();

        let result = ingest_staged(dir.path(), &database, &grafeo).await;
        assert_eq!(result.events_ingested, 1);
        assert_eq!(result.parse_errors, 1);
    }

    #[tokio::test]
    async fn test_ingest_clears_staging() {
        let dir = tempfile::tempdir().unwrap();
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        staging::stage_event(dir.path(), &make_tool_json("Write")).unwrap();

        let files_before = staging::list_staged(dir.path()).unwrap();
        assert_eq!(files_before.len(), 1);

        ingest_staged(dir.path(), &database, &grafeo).await;

        let files_after = staging::list_staged(dir.path()).unwrap();
        assert!(files_after.is_empty());
    }

    use hegel::{TestCase, generators as gs};

    /// Property: ingestion count matches staged count for valid events.
    #[hegel::test(test_cases = 30)]
    fn prop_ingest_count_matches_staged(tc: TestCase) {
        let dir = tempfile::tempdir().unwrap();

        let count: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));

        for _ in 0..count {
            let tool: String = tc.draw(
                gs::text()
                    .min_size(1)
                    .max_size(20)
                    .alphabet("abcdefghijklmnopqrstuvwxyz"),
            );
            staging::stage_event(dir.path(), &make_tool_json(&tool)).unwrap();
        }

        let rt = tokio::runtime::Runtime::new().unwrap();
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let result = rt.block_on(ingest_staged(dir.path(), &database, &grafeo));
        assert_eq!(result.events_ingested, count);
        assert_eq!(result.parse_errors, 0);

        // Staging should be empty after ingestion
        let remaining = staging::list_staged(dir.path()).unwrap();
        assert!(remaining.is_empty());
    }
}
