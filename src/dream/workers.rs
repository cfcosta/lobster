//! Background maintenance workers.
//!
//! Each worker handles a specific type of maintenance task:
//! entity merge proposals, task timeline updates, noise flagging,
//! and workflow pattern detection.

use grafeo::GrafeoDB;

use crate::store::{
    db::LobsterDb,
    ids::RawId,
    schema::{Entity, Task, TaskStatus},
};

/// Scan for tasks that haven't been seen in recent episodes and
/// mark them as potentially stale.
///
/// Returns the number of stale tasks found.
#[must_use]
pub fn scan_stale_tasks(db: &LobsterDb) -> Vec<(RawId, String)> {
    let mut stale = Vec::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return stale;
    };
    let Ok(iter) = db.tasks.iter(&rtxn) else {
        return stale;
    };

    for entry in iter.flatten() {
        let (key, value) = entry;
        if let Ok(task) = serde_json::from_slice::<Task>(value) {
            if task.status == TaskStatus::Open {
                let key_bytes: [u8; 16] = key.try_into().unwrap_or([0; 16]);
                stale.push((RawId::from_bytes(key_bytes), task.title.clone()));
            }
        }
    }

    stale
}

/// Find entities with the same canonical name but different IDs.
///
/// These are merge candidates that the dreaming scheduler can
/// propose for deduplication.
#[must_use]
pub fn find_duplicate_entities(db: &LobsterDb) -> Vec<(String, Vec<RawId>)> {
    let mut by_name: std::collections::HashMap<String, Vec<RawId>> =
        std::collections::HashMap::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return vec![];
    };
    let Ok(iter) = db.entities.iter(&rtxn) else {
        return vec![];
    };

    for entry in iter.flatten() {
        let (key, value) = entry;
        if let Ok(entity) = serde_json::from_slice::<Entity>(value) {
            let key_bytes: [u8; 16] = key.try_into().unwrap_or([0; 16]);
            by_name
                .entry(entity.canonical_name.to_lowercase())
                .or_default()
                .push(RawId::from_bytes(key_bytes));
        }
    }

    // Only return names with multiple IDs
    by_name
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .collect()
}

/// Result of a workflow mining pass.
#[derive(Debug, Default)]
pub struct WorkflowMiningResult {
    /// Number of episodes scanned.
    pub episodes_scanned: usize,
    /// Number of new workflows created.
    pub workflows_created: usize,
    /// Number of existing workflows updated with new sources.
    pub workflows_updated: usize,
}

/// Mine recurring tool-use patterns across Ready episodes and
/// promote them into `Workflow` entities and `ToolSequence` artifacts.
///
/// This worker:
/// 1. Loads all Ready episodes
/// 2. Extracts their tool-use sequences from raw events
/// 3. Runs n-gram pattern detection
/// 4. For patterns above threshold, creates `Workflow` entities and
///    `ToolSequence` artifacts (or updates existing ones)
/// 5. Projects workflow entities into Grafeo with edges to episodes
///
/// The worker is idempotent: running it twice produces the same
/// graph state.
#[allow(clippy::too_many_lines)]
pub fn scan_workflow_patterns(
    db: &LobsterDb,
    _grafeo: &GrafeoDB,
    config: &super::patterns::PatternConfig,
) -> WorkflowMiningResult {
    use crate::{
        dream::patterns::detect_patterns,
        episodes::sequences::{extract_event_sequence, sequence_label},
        store::{
            crud,
            ids::{RepoId, WorkflowId},
            schema::{Episode, ProcessingState, ToolSequence},
        },
    };

    let mut result = WorkflowMiningResult::default();

    // 1. Load all Ready episodes
    let mut episodes: Vec<Episode> = Vec::new();
    {
        let Ok(rtxn) = db.env.read_txn() else {
            return result;
        };
        let Ok(iter) = db.episodes.iter(&rtxn) else {
            return result;
        };
        for entry in iter.flatten() {
            let (_, value) = entry;
            if let Ok(ep) = serde_json::from_slice::<Episode>(value) {
                if ep.processing_state == ProcessingState::Ready {
                    episodes.push(ep);
                }
            }
        }
    }

    result.episodes_scanned = episodes.len();

    if episodes.len() < config.min_frequency {
        return result;
    }

    // 2. Extract tool-use sequences
    let sequences: Vec<Vec<crate::store::schema::EventKind>> = episodes
        .iter()
        .map(|ep| extract_event_sequence(db, ep.start_seq, ep.end_seq))
        .collect();

    // 3. Run pattern detection
    let patterns = detect_patterns(&sequences, config);

    if patterns.is_empty() {
        return result;
    }

    // Get existing workflows to check for duplicates
    let existing = crud::list_tool_sequences(db);

    // Determine the repo_id from the first episode
    let repo_id = episodes
        .first()
        .map_or_else(|| RepoId::derive(b"unknown"), |ep| ep.repo_id);

    // 4. For each detected pattern, create or update
    for detected in &patterns {
        // Derive a content-addressed workflow ID from the pattern
        let pattern_bytes: Vec<u8> = detected
            .pattern
            .iter()
            .flat_map(|k| serde_json::to_vec(k).unwrap_or_default())
            .collect();
        let workflow_id = WorkflowId::derive(&pattern_bytes);

        // Map source indices back to episode IDs
        let source_episodes: Vec<_> = detected
            .source_indices
            .iter()
            .filter_map(|&idx| episodes.get(idx).map(|ep| ep.episode_id))
            .collect();

        // Check if this pattern already exists
        let already_exists =
            existing.iter().any(|ts| ts.workflow_id == workflow_id);

        if already_exists {
            // Update: merge new source episodes
            if let Ok(mut existing_ts) =
                crud::get_tool_sequence(db, &workflow_id.raw())
            {
                let mut changed = false;
                for ep_id in &source_episodes {
                    if !existing_ts.source_episodes.contains(ep_id) {
                        existing_ts.source_episodes.push(*ep_id);
                        changed = true;
                    }
                }
                if changed {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        existing_ts.frequency =
                            existing_ts.source_episodes.len() as u32;
                    }
                    let _ = crud::put_tool_sequence(db, &existing_ts);
                    result.workflows_updated += 1;
                }
            }
        } else {
            // Create new workflow
            let label = sequence_label(&detected.pattern);
            let now_ms = chrono::Utc::now().timestamp_millis();

            let ts = ToolSequence {
                workflow_id,
                repo_id,
                pattern: detected.pattern.clone(),
                label: label.clone(),
                #[allow(clippy::cast_possible_truncation)]
                frequency: detected.frequency as u32,
                source_episodes: source_episodes.clone(),
                detected_ts_utc_ms: now_ms,
            };

            if crud::put_tool_sequence(db, &ts).is_ok() {
                // ToolSequence artifacts are kept for internal
                // pattern analysis, but we no longer promote them
                // into Entity records or project them into Grafeo.
                // The "ToolUse→Prompt→..." labels were noise in
                // retrieval and polluted the entity namespace.
                result.workflows_created += 1;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        crud,
        db,
        ids::{EntityId, EpisodeId, RepoId, TaskId},
        schema::{Entity, EntityKind, Task, TaskStatus},
    };

    #[test]
    fn test_scan_stale_tasks_empty() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let stale = scan_stale_tasks(&database);
        assert!(stale.is_empty());
    }

    #[test]
    fn test_scan_stale_tasks_finds_open() {
        let (database, _dir) = db::open_in_memory().unwrap();

        let task = Task {
            task_id: TaskId::derive(b"t1"),
            repo_id: RepoId::derive(b"repo"),
            title: "Build memory search".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep1"),
        };
        crud::put_task(&database, &task).unwrap();

        let completed = Task {
            task_id: TaskId::derive(b"t2"),
            repo_id: RepoId::derive(b"repo"),
            title: "Fix bug".into(),
            status: TaskStatus::Completed,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep2"),
        };
        crud::put_task(&database, &completed).unwrap();

        let stale = scan_stale_tasks(&database);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].1, "Build memory search");
    }

    #[test]
    fn test_find_duplicate_entities_none() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let entity = Entity {
            entity_id: EntityId::derive(b"e1"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
            first_seen_episode: None,
            last_seen_ts_utc_ms: None,
            mention_count: 0,
        };
        crud::put_entity(&database, &entity).unwrap();

        let dupes = find_duplicate_entities(&database);
        assert!(dupes.is_empty());
    }

    #[test]
    fn test_find_duplicate_entities_detects() {
        let (database, _dir) = db::open_in_memory().unwrap();

        // Two entities with same name but different IDs
        let e1 = Entity {
            entity_id: EntityId::derive(b"e1"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
            first_seen_episode: None,
            last_seen_ts_utc_ms: None,
            mention_count: 0,
        };
        let e2 = Entity {
            entity_id: EntityId::derive(b"e2"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "grafeo".into(), // different case
            first_seen_episode: None,
            last_seen_ts_utc_ms: None,
            mention_count: 0,
        };
        crud::put_entity(&database, &e1).unwrap();
        crud::put_entity(&database, &e2).unwrap();

        let dupes = find_duplicate_entities(&database);
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].1.len(), 2);
    }

    // ── Workflow mining tests ───────────────────────────────

    use crate::{
        dream::patterns::PatternConfig,
        graph::db as grafeo_db,
        store::schema::{EventKind, ProcessingState, RawEvent},
    };

    /// Create a Ready episode with raw events of the given kinds.
    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    fn setup_episode_with_events(
        database: &LobsterDb,
        ep_suffix: &[u8],
        start_seq: u64,
        kinds: &[EventKind],
    ) {
        let ep = crate::store::schema::Episode {
            episode_id: EpisodeId::derive(ep_suffix),
            repo_id: RepoId::derive(b"repo"),
            start_seq,
            end_seq: start_seq + kinds.len().saturating_sub(1) as u64,
            task_id: None,
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1_700_000_000_000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(database, &ep).unwrap();

        for (i, kind) in kinds.iter().enumerate() {
            let event = RawEvent {
                seq: start_seq + i as u64,
                repo_id: RepoId::derive(b"repo"),
                ts_utc_ms: 1_700_000_000_000 + i as i64,
                event_kind: kind.clone(),
                payload_hash: [0u8; 32],
                payload_bytes: vec![],
            };
            crud::append_raw_event(database, &event).unwrap();
        }
    }

    #[test]
    fn test_workflow_mining_empty_db() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();
        let config = PatternConfig::default();

        let result = scan_workflow_patterns(&database, &grafeo, &config);
        assert_eq!(result.episodes_scanned, 0);
        assert_eq!(result.workflows_created, 0);
    }

    #[test]
    fn test_workflow_mining_detects_recurring_pattern() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Same tool sequence in 3 episodes
        let pattern = [
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];
        setup_episode_with_events(&database, b"ep1", 0, &pattern);
        setup_episode_with_events(&database, b"ep2", 100, &pattern);
        setup_episode_with_events(&database, b"ep3", 200, &pattern);

        let config = PatternConfig {
            min_frequency: 2,
            ..PatternConfig::default()
        };
        let result = scan_workflow_patterns(&database, &grafeo, &config);

        assert_eq!(result.episodes_scanned, 3);
        assert!(
            result.workflows_created >= 1,
            "should detect at least one workflow"
        );

        // Verify stored in redb
        let stored = crud::list_tool_sequences(&database);
        assert!(!stored.is_empty(), "workflows should be persisted to redb");
    }

    #[test]
    fn test_workflow_mining_idempotent() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let pattern = [EventKind::ToolUse, EventKind::FileWrite];
        setup_episode_with_events(&database, b"ep1", 0, &pattern);
        setup_episode_with_events(&database, b"ep2", 100, &pattern);

        let config = PatternConfig::default();

        // Run twice
        let r1 = scan_workflow_patterns(&database, &grafeo, &config);
        let r2 = scan_workflow_patterns(&database, &grafeo, &config);

        // Second run should not create new workflows
        assert_eq!(r2.workflows_created, 0);
        // Total stored should be same as after first run
        let stored = crud::list_tool_sequences(&database);
        assert_eq!(stored.len(), r1.workflows_created);
    }

    #[test]
    fn test_workflow_mining_below_threshold() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Only 1 episode — can't meet min_frequency=2
        let pattern = [EventKind::FileEdit, EventKind::TestRun];
        setup_episode_with_events(&database, b"ep1", 0, &pattern);

        let config = PatternConfig::default();
        let result = scan_workflow_patterns(&database, &grafeo, &config);

        assert_eq!(result.episodes_scanned, 1);
        assert_eq!(result.workflows_created, 0);
    }

    // -- Property: worker only creates workflows for patterns
    //    meeting threshold --
    #[hegel::test(test_cases = 30)]
    fn prop_workflows_meet_threshold(tc: hegel::TestCase) {
        use hegel::generators as gs;

        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let n_eps: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let kinds_pool = vec![
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
            EventKind::ToolUse,
            EventKind::FileWrite,
        ];

        for i in 0..n_eps {
            let len: usize =
                tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
            let mut kinds = Vec::with_capacity(len);
            for _ in 0..len {
                kinds.push(tc.draw(gs::sampled_from(kinds_pool.clone())));
            }
            #[allow(clippy::cast_possible_truncation)]
            setup_episode_with_events(
                &database,
                &(i as u32).to_le_bytes(),
                (i * 1000) as u64,
                &kinds,
            );
        }

        let min_freq: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(n_eps));
        let config = PatternConfig {
            min_frequency: min_freq,
            ..PatternConfig::default()
        };

        let _ = scan_workflow_patterns(&database, &grafeo, &config);

        let stored = crud::list_tool_sequences(&database);
        for ts in &stored {
            assert!(
                ts.frequency as usize >= min_freq,
                "workflow {} has frequency {} < min {}",
                ts.workflow_id,
                ts.frequency,
                min_freq
            );
        }
    }

    /// Workflow mining must NOT create Entity records or Grafeo nodes.
    /// `ToolSequence` artifacts are internal; promoting them to entities
    /// polluted retrieval with noise like "ToolUse→Prompt→ToolUse".
    #[hegel::test(test_cases = 20)]
    fn prop_workflow_mining_no_entities(tc: hegel::TestCase) {
        use hegel::generators as gs;

        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Use a fixed shared pattern to guarantee detection
        let pattern = [
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];
        let n_eps: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        for i in 0..n_eps {
            #[allow(clippy::cast_possible_truncation)]
            setup_episode_with_events(
                &database,
                &(i as u32).to_le_bytes(),
                (i * 1000) as u64,
                &pattern,
            );
        }

        let config = PatternConfig {
            min_frequency: 2,
            ..PatternConfig::default()
        };

        let entities_before = crud::count_entities(&database);
        let grafeo_nodes_before = grafeo.node_count();

        let result = scan_workflow_patterns(&database, &grafeo, &config);

        let entities_after = crud::count_entities(&database);
        let grafeo_nodes_after = grafeo.node_count();

        // Workflows were detected...
        if n_eps >= 2 {
            assert!(
                result.workflows_created > 0 || result.workflows_updated > 0,
                "should detect patterns in {n_eps} episodes"
            );
        }
        // ...but no entities or graph nodes were created
        assert_eq!(
            entities_before, entities_after,
            "workflow mining must not create Entity records"
        );
        assert_eq!(
            grafeo_nodes_before, grafeo_nodes_after,
            "workflow mining must not project into Grafeo"
        );
    }
}
