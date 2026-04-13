//! Generic CRUD operations for all Lobster record types stored in
//! redb.
//!
//! All records are serialized to JSON bytes for storage. Keys are
//! either `u64` sequence numbers or `[u8; 16]` raw ID bytes.
#![allow(clippy::missing_errors_doc)]

use redb::{Database, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::store::{
    ids::RawId,
    schema::{
        Decision,
        EmbeddingArtifact,
        Entity,
        Episode,
        ExtractionArtifact,
        RawEvent,
        SummaryArtifact,
        Task,
        ToolSequence,
    },
    tables,
};

/// Error type for CRUD operations.
#[derive(Debug)]
pub enum StoreError {
    Redb(String),
    Serde(serde_json::Error),
    NotFound,
}

impl StoreError {
    fn redb(e: impl std::fmt::Display) -> Self {
        Self::Redb(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Redb(e) => write!(f, "redb: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::NotFound => write!(f, "not found"),
        }
    }
}

impl std::error::Error for StoreError {}

// ── Helpers ──────────────────────────────────────────────────

fn put_by_id<V: Serialize>(
    db: &Database,
    table_def: TableDefinition<&[u8; 16], &[u8]>,
    id: &RawId,
    value: &V,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(value)?;
    let mut write_txn = db.begin_write().map_err(StoreError::redb)?;
    // Spec: Durability::Immediate for episodes, accepted
    // artifacts, and visibility-state flips. These must survive
    // crashes.
    write_txn
        .set_durability(redb::Durability::Immediate)
        .map_err(StoreError::redb)?;
    {
        let mut table =
            write_txn.open_table(table_def).map_err(StoreError::redb)?;
        table
            .insert(id.as_bytes(), bytes.as_slice())
            .map_err(StoreError::redb)?;
    }
    write_txn.commit().map_err(StoreError::redb)?;
    Ok(())
}

fn get_by_id<V: for<'de> Deserialize<'de>>(
    db: &Database,
    table_def: TableDefinition<&[u8; 16], &[u8]>,
    id: &RawId,
) -> Result<V, StoreError> {
    let read_txn = db.begin_read().map_err(StoreError::redb)?;
    let table = read_txn.open_table(table_def).map_err(StoreError::redb)?;
    let guard = table
        .get(id.as_bytes())
        .map_err(StoreError::redb)?
        .ok_or(StoreError::NotFound)?;
    let value: V = serde_json::from_slice(guard.value())?;
    Ok(value)
}

// ── RawEvent (seq-keyed) ─────────────────────────────────────

/// Append a raw event with `Durability::Immediate`.
pub fn append_raw_event(
    db: &Database,
    event: &RawEvent,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(event)?;
    let mut write_txn = db.begin_write().map_err(StoreError::redb)?;
    write_txn
        .set_durability(redb::Durability::Immediate)
        .map_err(StoreError::redb)?;
    {
        let mut table = write_txn
            .open_table(tables::RAW_EVENTS)
            .map_err(StoreError::redb)?;
        table
            .insert(event.seq, bytes.as_slice())
            .map_err(StoreError::redb)?;
    }
    write_txn.commit().map_err(StoreError::redb)?;
    Ok(())
}

/// Read a raw event by sequence number.
pub fn get_raw_event(db: &Database, seq: u64) -> Result<RawEvent, StoreError> {
    let read_txn = db.begin_read().map_err(StoreError::redb)?;
    let table = read_txn
        .open_table(tables::RAW_EVENTS)
        .map_err(StoreError::redb)?;
    let guard = table
        .get(seq)
        .map_err(StoreError::redb)?
        .ok_or(StoreError::NotFound)?;
    let event: RawEvent = serde_json::from_slice(guard.value())?;
    Ok(event)
}

// ── Episode ──────────────────────────────────────────────────

pub fn put_episode(db: &Database, ep: &Episode) -> Result<(), StoreError> {
    put_by_id(db, tables::EPISODES, &ep.episode_id.raw(), ep)
}

pub fn get_episode(db: &Database, id: &RawId) -> Result<Episode, StoreError> {
    get_by_id(db, tables::EPISODES, id)
}

// ── Decision ─────────────────────────────────────────────────

pub fn put_decision(db: &Database, dec: &Decision) -> Result<(), StoreError> {
    put_by_id(db, tables::DECISIONS, &dec.decision_id.raw(), dec)
}

pub fn get_decision(db: &Database, id: &RawId) -> Result<Decision, StoreError> {
    get_by_id(db, tables::DECISIONS, id)
}

// ── Task ─────────────────────────────────────────────────────

pub fn put_task(db: &Database, task: &Task) -> Result<(), StoreError> {
    put_by_id(db, tables::TASKS, &task.task_id.raw(), task)
}

pub fn get_task(db: &Database, id: &RawId) -> Result<Task, StoreError> {
    get_by_id(db, tables::TASKS, id)
}

// ── Entity ───────────────────────────────────────────────────

pub fn put_entity(db: &Database, entity: &Entity) -> Result<(), StoreError> {
    put_by_id(db, tables::ENTITIES, &entity.entity_id.raw(), entity)
}

pub fn get_entity(db: &Database, id: &RawId) -> Result<Entity, StoreError> {
    get_by_id(db, tables::ENTITIES, id)
}

// ── Summary Artifact ─────────────────────────────────────────

pub fn put_summary_artifact(
    db: &Database,
    art: &SummaryArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, tables::SUMMARY_ARTIFACTS, &art.episode_id.raw(), art)
}

pub fn get_summary_artifact(
    db: &Database,
    episode_id: &RawId,
) -> Result<SummaryArtifact, StoreError> {
    get_by_id(db, tables::SUMMARY_ARTIFACTS, episode_id)
}

// ── Extraction Artifact ──────────────────────────────────────

pub fn put_extraction_artifact(
    db: &Database,
    art: &ExtractionArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, tables::EXTRACTION_ARTIFACTS, &art.episode_id.raw(), art)
}

pub fn get_extraction_artifact(
    db: &Database,
    episode_id: &RawId,
) -> Result<ExtractionArtifact, StoreError> {
    get_by_id(db, tables::EXTRACTION_ARTIFACTS, episode_id)
}

// ── Embedding Artifact ───────────────────────────────────────

pub fn put_embedding_artifact(
    db: &Database,
    art: &EmbeddingArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, tables::EMBEDDING_ARTIFACTS, &art.artifact_id.raw(), art)
}

pub fn get_embedding_artifact(
    db: &Database,
    artifact_id: &RawId,
) -> Result<EmbeddingArtifact, StoreError> {
    get_by_id(db, tables::EMBEDDING_ARTIFACTS, artifact_id)
}

// ── Tool Sequence (procedural memory) ────────────────────────

pub fn put_tool_sequence(
    db: &Database,
    ts: &ToolSequence,
) -> Result<(), StoreError> {
    put_by_id(db, tables::TOOL_SEQUENCES, &ts.workflow_id.raw(), ts)
}

pub fn get_tool_sequence(
    db: &Database,
    workflow_id: &RawId,
) -> Result<ToolSequence, StoreError> {
    get_by_id(db, tables::TOOL_SEQUENCES, workflow_id)
}

/// List all stored tool sequences.
#[must_use]
pub fn list_tool_sequences(db: &Database) -> Vec<ToolSequence> {
    use redb::ReadableTable;

    let mut results = Vec::new();

    let Ok(read_txn) = db.begin_read() else {
        return results;
    };
    let Ok(table) = read_txn.open_table(tables::TOOL_SEQUENCES) else {
        return results;
    };
    let Ok(iter) = table.iter() else {
        return results;
    };

    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(ts) = serde_json::from_slice::<ToolSequence>(value.value()) {
            results.push(ts);
        }
    }

    results
}

// ── Repo Config ──────────────────────────────────────────────

pub fn put_repo_config(
    db: &Database,
    repo_id: &RawId,
    config: &serde_json::Value,
) -> Result<(), StoreError> {
    put_by_id(db, tables::REPO_CONFIG, repo_id, config)
}

pub fn get_repo_config(
    db: &Database,
    repo_id: &RawId,
) -> Result<serde_json::Value, StoreError> {
    get_by_id(db, tables::REPO_CONFIG, repo_id)
}

// ── Retrieval Stats ──────────────────────────────────────────

pub fn put_retrieval_stats(
    db: &Database,
    key: &str,
    stats: &serde_json::Value,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(stats)?;
    let write_txn = db.begin_write().map_err(StoreError::redb)?;
    {
        let mut table = write_txn
            .open_table(tables::RETRIEVAL_STATS)
            .map_err(StoreError::redb)?;
        table
            .insert(key, bytes.as_slice())
            .map_err(StoreError::redb)?;
    }
    write_txn.commit().map_err(StoreError::redb)?;
    Ok(())
}

// ── Projection Metadata ──────────────────────────────────────

pub fn put_projection_metadata(
    db: &Database,
    episode_id: &RawId,
    metadata: &serde_json::Value,
) -> Result<(), StoreError> {
    put_by_id(db, tables::PROJECTION_METADATA, episode_id, metadata)
}

// ── Processing Jobs ──────────────────────────────────────────

pub fn put_processing_job(
    db: &Database,
    seq: u64,
    job: &serde_json::Value,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(job)?;
    let write_txn = db.begin_write().map_err(StoreError::redb)?;
    {
        let mut table = write_txn
            .open_table(tables::PROCESSING_JOBS)
            .map_err(StoreError::redb)?;
        table
            .insert(seq, bytes.as_slice())
            .map_err(StoreError::redb)?;
    }
    write_txn.commit().map_err(StoreError::redb)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        db,
        ids::{ArtifactId, DecisionId, EntityId, EpisodeId, RepoId, TaskId},
        schema::{
            Confidence,
            EmbeddingBackend,
            EntityKind,
            EventKind,
            EvidenceRef,
            ProcessingState,
            TaskStatus,
        },
    };

    fn test_db() -> Database {
        db::open_in_memory().expect("in-memory db")
    }

    // ── RawEvent round-trip property ─────────────────────────

    #[hegel::composite]
    fn gen_raw_event(tc: hegel::TestCase) -> RawEvent {
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        RawEvent {
            seq: tc
                .draw(gs::integers::<u64>().min_value(0).max_value(1_000_000)),
            repo_id: RepoId::derive(&repo_input),
            ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            event_kind: tc.draw(gs::sampled_from(vec![
                EventKind::UserPromptSubmit,
                EventKind::AssistantResponse,
                EventKind::ToolUse,
                EventKind::FileRead,
            ])),
            payload_hash: tc.draw(gs::arrays(gs::integers::<u8>())),
            payload_bytes: tc
                .draw(gs::vecs(gs::integers::<u8>()).max_size(128)),
        }
    }

    // -- Property: write then read RawEvent = identity --
    #[hegel::test(test_cases = 50)]
    fn prop_raw_event_roundtrip(tc: TestCase) {
        let event = tc.draw(gen_raw_event());
        let db = test_db();
        append_raw_event(&db, &event).expect("append");
        let loaded = get_raw_event(&db, event.seq).expect("get");
        assert_eq!(event, loaded);
    }

    // ── Episode round-trip property ──────────────────────────

    #[hegel::test(test_cases = 50)]
    fn prop_episode_roundtrip(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let ep = Episode {
            episode_id: EpisodeId::derive(&input),
            repo_id: RepoId::derive(&repo_input),
            start_seq: 0,
            end_seq: 10,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: 1_700_000_000_000,
            retry_count: 0,
            is_noisy: false,
        };
        let db = test_db();
        put_episode(&db, &ep).expect("put");
        let loaded = get_episode(&db, &ep.episode_id.raw()).expect("get");
        assert_eq!(ep, loaded);
    }

    // ── Decision round-trip ──────────────────────────────────

    #[test]
    fn test_decision_roundtrip() {
        let db = test_db();
        let dec = Decision {
            decision_id: DecisionId::derive(b"test-dec"),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(b"ep"),
            task_id: None,
            statement: "Use redb".into(),
            rationale: "Embedded, ACID, Rust-native".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1_700_000_000_000,
            valid_to_ts_utc_ms: None,
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(b"ep"),
                span_summary: "discussed storage options".into(),
            }],
        };
        put_decision(&db, &dec).expect("put");
        let loaded = get_decision(&db, &dec.decision_id.raw()).expect("get");
        assert_eq!(dec, loaded);
    }

    // ── Task round-trip ──────────────────────────────────────

    #[test]
    fn test_task_roundtrip() {
        let db = test_db();
        let task = Task {
            task_id: TaskId::derive(b"build-memory"),
            repo_id: RepoId::derive(b"repo"),
            title: "Build memory search".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep1"),
        };
        put_task(&db, &task).expect("put");
        let loaded = get_task(&db, &task.task_id.raw()).expect("get");
        assert_eq!(task, loaded);
    }

    // ── Entity round-trip ────────────────────────────────────

    #[test]
    fn test_entity_roundtrip() {
        let db = test_db();
        let entity = Entity {
            entity_id: EntityId::derive(b"grafeo"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
        };
        put_entity(&db, &entity).expect("put");
        let loaded = get_entity(&db, &entity.entity_id.raw()).expect("get");
        assert_eq!(entity, loaded);
    }

    // ── Artifact round-trips ─────────────────────────────────

    #[test]
    fn test_summary_artifact_roundtrip() {
        let db = test_db();
        let art = SummaryArtifact {
            episode_id: EpisodeId::derive(b"ep"),
            revision: "v1".into(),
            summary_text: "User debugged a test failure".into(),
            payload_checksum: [0xAA; 32],
        };
        put_summary_artifact(&db, &art).expect("put");
        let loaded =
            get_summary_artifact(&db, &art.episode_id.raw()).expect("get");
        assert_eq!(art, loaded);
    }

    #[test]
    fn test_extraction_artifact_roundtrip() {
        let db = test_db();
        let art = ExtractionArtifact {
            episode_id: EpisodeId::derive(b"ep"),
            revision: "v1".into(),
            output_json: b"{}".to_vec(),
            payload_checksum: [0xBB; 32],
        };
        put_extraction_artifact(&db, &art).expect("put");
        let loaded =
            get_extraction_artifact(&db, &art.episode_id.raw()).expect("get");
        assert_eq!(art, loaded);
    }

    #[test]
    fn test_embedding_artifact_roundtrip() {
        let db = test_db();
        let art = EmbeddingArtifact {
            artifact_id: ArtifactId::derive(b"emb"),
            revision: "v1".into(),
            backend: EmbeddingBackend::Cpu,
            quantization: None,
            pooled_vector_bytes: vec![0u8; 64],
            late_interaction_bytes: None,
            payload_checksum: [0xCC; 32],
        };
        put_embedding_artifact(&db, &art).expect("put");
        let loaded =
            get_embedding_artifact(&db, &art.artifact_id.raw()).expect("get");
        assert_eq!(art, loaded);
    }

    // ── ToolSequence round-trip ────────────────────────────────

    #[hegel::composite]
    fn gen_tool_sequence(tc: hegel::TestCase) -> ToolSequence {
        use crate::store::ids::WorkflowId;

        let wf_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));

        let kinds = vec![
            EventKind::UserPromptSubmit,
            EventKind::ToolUse,
            EventKind::FileRead,
            EventKind::FileWrite,
            EventKind::TestRun,
        ];
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
        let mut pattern = Vec::with_capacity(n);
        for _ in 0..n {
            pattern.push(tc.draw(gs::sampled_from(kinds.clone())));
        }

        let n_sources: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let mut sources = Vec::with_capacity(n_sources);
        for i in 0..n_sources {
            let mut ep_in = wf_input.clone();
            #[allow(clippy::cast_possible_truncation)]
            ep_in.extend_from_slice(&(i as u32).to_le_bytes());
            sources.push(EpisodeId::derive(&ep_in));
        }

        ToolSequence {
            workflow_id: WorkflowId::derive(&wf_input),
            repo_id: RepoId::derive(&repo_input),
            pattern,
            label: tc.draw(gs::text().min_size(1).max_size(80)),
            frequency: tc
                .draw(gs::integers::<u32>().min_value(2).max_value(50)),
            source_episodes: sources,
            detected_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
        }
    }

    // -- Property: put then get ToolSequence = identity --
    #[hegel::test(test_cases = 50)]
    fn prop_tool_sequence_roundtrip(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        let db = test_db();
        put_tool_sequence(&db, &ts).expect("put");
        let loaded =
            get_tool_sequence(&db, &ts.workflow_id.raw()).expect("get");
        assert_eq!(ts, loaded);
    }

    // -- Property: duplicate puts are idempotent --
    #[hegel::test(test_cases = 30)]
    fn prop_tool_sequence_put_idempotent(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        let db = test_db();
        put_tool_sequence(&db, &ts).expect("put 1");
        put_tool_sequence(&db, &ts).expect("put 2");
        let loaded =
            get_tool_sequence(&db, &ts.workflow_id.raw()).expect("get");
        assert_eq!(ts, loaded);
    }

    // -- Property: list returns all stored sequences --
    #[hegel::test(test_cases = 30)]
    fn prop_tool_sequence_list_complete(tc: TestCase) {
        let db = test_db();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));

        let mut stored = Vec::new();
        for i in 0..n {
            let mut wf_in = vec![0u8; 4];
            #[allow(clippy::cast_possible_truncation)]
            wf_in.extend_from_slice(&(i as u32).to_le_bytes());
            let ts = ToolSequence {
                workflow_id: crate::store::ids::WorkflowId::derive(&wf_in),
                repo_id: RepoId::derive(b"repo"),
                pattern: vec![EventKind::ToolUse, EventKind::FileWrite],
                label: format!("workflow-{i}"),
                frequency: 3,
                source_episodes: vec![
                    EpisodeId::derive(b"ep1"),
                    EpisodeId::derive(b"ep2"),
                ],
                detected_ts_utc_ms: 1_700_000_000_000,
            };
            put_tool_sequence(&db, &ts).expect("put");
            stored.push(ts);
        }

        let listed = list_tool_sequences(&db);
        assert_eq!(
            listed.len(),
            stored.len(),
            "list should return all stored sequences"
        );
        for ts in &stored {
            assert!(
                listed.iter().any(|l| l.workflow_id == ts.workflow_id),
                "missing workflow {}",
                ts.workflow_id
            );
        }
    }

    // ── Not found ────────────────────────────────────────────

    #[test]
    fn test_get_missing_returns_not_found() {
        let db = test_db();
        let id = EpisodeId::derive(b"nonexistent");
        let result = get_episode(&db, &id.raw());
        assert!(matches!(result, Err(StoreError::NotFound)));
    }

    #[test]
    fn test_get_missing_tool_sequence() {
        let db = test_db();
        // Write one sequence so the table exists
        let ts = ToolSequence {
            workflow_id: crate::store::ids::WorkflowId::derive(b"exists"),
            repo_id: RepoId::derive(b"repo"),
            pattern: vec![EventKind::ToolUse, EventKind::FileWrite],
            label: "test".into(),
            frequency: 2,
            source_episodes: vec![EpisodeId::derive(b"ep1")],
            detected_ts_utc_ms: 1_000,
        };
        put_tool_sequence(&db, &ts).expect("put");

        let id = crate::store::ids::WorkflowId::derive(b"nonexistent");
        let result = get_tool_sequence(&db, &id.raw());
        assert!(matches!(result, Err(StoreError::NotFound)));
    }
}
