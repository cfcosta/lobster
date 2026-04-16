//! Generic CRUD operations for all Lobster record types stored in
//! LMDB via heed.
//!
//! All records are serialized to JSON bytes for storage. Keys are
//! either `u64` sequence numbers, `[u8; 16]` raw ID bytes, or
//! `&str` strings.
#![allow(clippy::missing_errors_doc)]

use serde::{Deserialize, Serialize};

use crate::store::{
    db::LobsterDb,
    ids::RawId,
    schema::{
        Decision,
        EmbeddingArtifact,
        Entity,
        Episode,
        ExtractionArtifact,
        RawEvent,
        RecallEngagement,
        RepoProfile,
        SummaryArtifact,
        Task,
        ToolSequence,
    },
};

/// Error type for CRUD operations.
#[derive(Debug)]
pub enum StoreError {
    Db(String),
    Serde(serde_json::Error),
    NotFound,
}

impl StoreError {
    fn db(e: impl std::fmt::Display) -> Self {
        Self::Db(e.to_string())
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
            Self::Db(e) => write!(f, "db: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::NotFound => write!(f, "not found"),
        }
    }
}

impl std::error::Error for StoreError {}

// ── Helpers ──────────────────────────────────────────────────

fn put_by_id<V: Serialize>(
    db: &LobsterDb,
    table: crate::store::db::IdDb,
    id: &RawId,
    value: &V,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(value)?;
    let mut wtxn = db.env.write_txn().map_err(StoreError::db)?;
    table
        .put(&mut wtxn, id.as_bytes(), &bytes)
        .map_err(StoreError::db)?;
    wtxn.commit().map_err(StoreError::db)?;
    Ok(())
}

fn get_by_id<V: for<'de> Deserialize<'de>>(
    db: &LobsterDb,
    table: crate::store::db::IdDb,
    id: &RawId,
) -> Result<V, StoreError> {
    let rtxn = db.env.read_txn().map_err(StoreError::db)?;
    let bytes = table
        .get(&rtxn, id.as_bytes())
        .map_err(StoreError::db)?
        .ok_or(StoreError::NotFound)?;
    let value: V = serde_json::from_slice(bytes)?;
    Ok(value)
}

// ── RawEvent (seq-keyed) ─────────────────────────────────────

/// Append a raw event.
pub fn append_raw_event(
    db: &LobsterDb,
    event: &RawEvent,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(event)?;
    let mut wtxn = db.env.write_txn().map_err(StoreError::db)?;
    db.raw_events
        .put(&mut wtxn, &event.seq, &bytes)
        .map_err(StoreError::db)?;
    wtxn.commit().map_err(StoreError::db)?;
    Ok(())
}

/// Read a raw event by sequence number.
pub fn get_raw_event(db: &LobsterDb, seq: u64) -> Result<RawEvent, StoreError> {
    let rtxn = db.env.read_txn().map_err(StoreError::db)?;
    let bytes = db
        .raw_events
        .get(&rtxn, &seq)
        .map_err(StoreError::db)?
        .ok_or(StoreError::NotFound)?;
    let event: RawEvent = serde_json::from_slice(bytes)?;
    Ok(event)
}

/// Load all raw events in a sequence range (inclusive).
pub fn get_raw_events_range(
    db: &LobsterDb,
    start_seq: u64,
    end_seq: u64,
) -> Result<Vec<RawEvent>, StoreError> {
    let rtxn = db.env.read_txn().map_err(StoreError::db)?;
    let mut events = Vec::new();
    let range = start_seq..=end_seq;
    let iter = db
        .raw_events
        .range(&rtxn, &range)
        .map_err(StoreError::db)?;
    for entry in iter {
        let (_, value) = entry.map_err(StoreError::db)?;
        let event: RawEvent = serde_json::from_slice(value)?;
        events.push(event);
    }
    Ok(events)
}

// ── Episode ──────────────────────────────────────────────────

pub fn put_episode(db: &LobsterDb, ep: &Episode) -> Result<(), StoreError> {
    put_by_id(db, db.episodes, &ep.episode_id.raw(), ep)
}

pub fn get_episode(db: &LobsterDb, id: &RawId) -> Result<Episode, StoreError> {
    get_by_id(db, db.episodes, id)
}

// ── Decision ─────────────────────────────────────────────────

pub fn put_decision(db: &LobsterDb, dec: &Decision) -> Result<(), StoreError> {
    put_by_id(db, db.decisions, &dec.decision_id.raw(), dec)
}

pub fn get_decision(
    db: &LobsterDb,
    id: &RawId,
) -> Result<Decision, StoreError> {
    get_by_id(db, db.decisions, id)
}

// ── Task ─────────────────────────────────────────────────────

pub fn put_task(db: &LobsterDb, task: &Task) -> Result<(), StoreError> {
    put_by_id(db, db.tasks, &task.task_id.raw(), task)
}

pub fn get_task(db: &LobsterDb, id: &RawId) -> Result<Task, StoreError> {
    get_by_id(db, db.tasks, id)
}

// ── Entity ───────────────────────────────────────────────────

pub fn put_entity(db: &LobsterDb, entity: &Entity) -> Result<(), StoreError> {
    put_by_id(db, db.entities, &entity.entity_id.raw(), entity)
}

pub fn get_entity(db: &LobsterDb, id: &RawId) -> Result<Entity, StoreError> {
    get_by_id(db, db.entities, id)
}

/// Count all entities in the store.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn count_entities(db: &LobsterDb) -> usize {
    let Ok(rtxn) = db.env.read_txn() else {
        return 0;
    };
    db.entities.len(&rtxn).unwrap_or(0) as usize
}

// ── Summary Artifact ─────────────────────────────────────────

pub fn put_summary_artifact(
    db: &LobsterDb,
    art: &SummaryArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, db.summary_artifacts, &art.episode_id.raw(), art)
}

pub fn get_summary_artifact(
    db: &LobsterDb,
    episode_id: &RawId,
) -> Result<SummaryArtifact, StoreError> {
    get_by_id(db, db.summary_artifacts, episode_id)
}

// ── Extraction Artifact ──────────────────────────────────────

pub fn put_extraction_artifact(
    db: &LobsterDb,
    art: &ExtractionArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, db.extraction_artifacts, &art.episode_id.raw(), art)
}

pub fn get_extraction_artifact(
    db: &LobsterDb,
    episode_id: &RawId,
) -> Result<ExtractionArtifact, StoreError> {
    get_by_id(db, db.extraction_artifacts, episode_id)
}

// ── Embedding Artifact ───────────────────────────────────────

pub fn put_embedding_artifact(
    db: &LobsterDb,
    art: &EmbeddingArtifact,
) -> Result<(), StoreError> {
    put_by_id(db, db.embedding_artifacts, &art.artifact_id.raw(), art)
}

pub fn get_embedding_artifact(
    db: &LobsterDb,
    artifact_id: &RawId,
) -> Result<EmbeddingArtifact, StoreError> {
    get_by_id(db, db.embedding_artifacts, artifact_id)
}

// ── Tool Sequence (procedural memory) ────────────────────────

pub fn put_tool_sequence(
    db: &LobsterDb,
    ts: &ToolSequence,
) -> Result<(), StoreError> {
    put_by_id(db, db.tool_sequences, &ts.workflow_id.raw(), ts)
}

pub fn get_tool_sequence(
    db: &LobsterDb,
    workflow_id: &RawId,
) -> Result<ToolSequence, StoreError> {
    get_by_id(db, db.tool_sequences, workflow_id)
}

/// List all stored tool sequences.
#[must_use]
pub fn list_tool_sequences(db: &LobsterDb) -> Vec<ToolSequence> {
    let mut results = Vec::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return results;
    };
    let Ok(iter) = db.tool_sequences.iter(&rtxn) else {
        return results;
    };

    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(ts) = serde_json::from_slice::<ToolSequence>(value) {
            results.push(ts);
        }
    }

    results
}

// ── Recall Engagement ────────────────────────────────────────

pub fn put_recall_engagement(
    db: &LobsterDb,
    eng: &RecallEngagement,
) -> Result<(), StoreError> {
    put_by_id(db, db.recall_engagements, &eng.surfaced_id, eng)
}

pub fn get_recall_engagement(
    db: &LobsterDb,
    surfaced_id: &RawId,
) -> Result<RecallEngagement, StoreError> {
    get_by_id(db, db.recall_engagements, surfaced_id)
}

/// Record that an artifact was surfaced in recall. Creates or
/// increments the `surface_count`.
pub fn record_surface(
    db: &LobsterDb,
    surfaced_id: &RawId,
    artifact_type: &str,
) -> Result<(), StoreError> {
    match get_recall_engagement(db, surfaced_id) {
        Ok(mut eng) => {
            eng.surface_count += 1;
            eng.surfaced_ts_utc_ms = chrono::Utc::now().timestamp_millis();
            put_recall_engagement(db, &eng)
        }
        Err(StoreError::NotFound | StoreError::Db(_)) => {
            let eng = RecallEngagement {
                surfaced_id: *surfaced_id,
                artifact_type: artifact_type.into(),
                surfaced_ts_utc_ms: chrono::Utc::now().timestamp_millis(),
                surface_count: 1,
                engagement_count: 0,
            };
            put_recall_engagement(db, &eng)
        }
        Err(e) => Err(e),
    }
}

/// Record that the user engaged with a previously surfaced artifact.
pub fn record_engagement(
    db: &LobsterDb,
    surfaced_id: &RawId,
) -> Result<(), StoreError> {
    let mut eng = get_recall_engagement(db, surfaced_id)?;
    eng.engagement_count += 1;
    put_recall_engagement(db, &eng)
}

// ── Repo Profile ────────────────────────────────────────────

pub fn put_repo_profile(
    db: &LobsterDb,
    profile: &RepoProfile,
) -> Result<(), StoreError> {
    put_by_id(db, db.repo_profiles, &profile.repo_id.raw(), profile)
}

pub fn get_repo_profile(
    db: &LobsterDb,
    repo_id: &RawId,
) -> Result<RepoProfile, StoreError> {
    get_by_id(db, db.repo_profiles, repo_id)
}

// ── Repo Config ──────────────────────────────────────────────

pub fn put_repo_config(
    db: &LobsterDb,
    repo_id: &RawId,
    config: &serde_json::Value,
) -> Result<(), StoreError> {
    put_by_id(db, db.repo_config, repo_id, config)
}

pub fn get_repo_config(
    db: &LobsterDb,
    repo_id: &RawId,
) -> Result<serde_json::Value, StoreError> {
    get_by_id(db, db.repo_config, repo_id)
}

// ── Retrieval Stats ──────────────────────────────────────────

pub fn put_retrieval_stats(
    db: &LobsterDb,
    key: &str,
    stats: &serde_json::Value,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(stats)?;
    let mut wtxn = db.env.write_txn().map_err(StoreError::db)?;
    db.retrieval_stats
        .put(&mut wtxn, key, &bytes)
        .map_err(StoreError::db)?;
    wtxn.commit().map_err(StoreError::db)?;
    Ok(())
}

// ── Projection Metadata ──────────────────────────────────────

pub fn put_projection_metadata(
    db: &LobsterDb,
    episode_id: &RawId,
    metadata: &serde_json::Value,
) -> Result<(), StoreError> {
    put_by_id(db, db.projection_metadata, episode_id, metadata)
}

// ── Processing Jobs ──────────────────────────────────────────

pub fn put_processing_job(
    db: &LobsterDb,
    seq: u64,
    job: &serde_json::Value,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(job)?;
    let mut wtxn = db.env.write_txn().map_err(StoreError::db)?;
    db.processing_jobs
        .put(&mut wtxn, &seq, &bytes)
        .map_err(StoreError::db)?;
    wtxn.commit().map_err(StoreError::db)?;
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
            ProfileFact,
            RepoProfile,
            TaskStatus,
        },
    };

    fn test_db() -> (LobsterDb, tempfile::TempDir) {
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
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
        put_episode(&db, &ep).expect("put");
        let loaded = get_episode(&db, &ep.episode_id.raw()).expect("get");
        assert_eq!(ep, loaded);
    }

    // ── Decision round-trip ──────────────────────────────────

    #[test]
    fn test_decision_roundtrip() {
        let (db, _dir) = test_db();
        let dec = Decision {
            decision_id: DecisionId::derive(b"test-dec"),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(b"ep"),
            task_id: None,
            statement: "Use LMDB".into(),
            rationale: "Embedded, MVCC, Rust-native via heed".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1_700_000_000_000,
            valid_to_ts_utc_ms: None,
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(b"ep"),
                span_summary: "discussed storage options".into(),
            }],
            premises: vec![],
        };
        put_decision(&db, &dec).expect("put");
        let loaded = get_decision(&db, &dec.decision_id.raw()).expect("get");
        assert_eq!(dec, loaded);
    }

    // ── Task round-trip ──────────────────────────────────────

    #[test]
    fn test_task_roundtrip() {
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
        let entity = Entity {
            entity_id: EntityId::derive(b"grafeo"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
            first_seen_episode: None,
            last_seen_ts_utc_ms: None,
            mention_count: 0,
        };
        put_entity(&db, &entity).expect("put");
        let loaded = get_entity(&db, &entity.entity_id.raw()).expect("get");
        assert_eq!(entity, loaded);
    }

    // ── Artifact round-trips ─────────────────────────────────

    #[test]
    fn test_summary_artifact_roundtrip() {
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
        put_tool_sequence(&db, &ts).expect("put");
        let loaded =
            get_tool_sequence(&db, &ts.workflow_id.raw()).expect("get");
        assert_eq!(ts, loaded);
    }

    // -- Property: duplicate puts are idempotent --
    #[hegel::test(test_cases = 30)]
    fn prop_tool_sequence_put_idempotent(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        let (db, _dir) = test_db();
        put_tool_sequence(&db, &ts).expect("put 1");
        put_tool_sequence(&db, &ts).expect("put 2");
        let loaded =
            get_tool_sequence(&db, &ts.workflow_id.raw()).expect("get");
        assert_eq!(ts, loaded);
    }

    // -- Property: list returns all stored sequences --
    #[hegel::test(test_cases = 30)]
    fn prop_tool_sequence_list_complete(tc: TestCase) {
        let (db, _dir) = test_db();
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
        let (db, _dir) = test_db();
        let id = EpisodeId::derive(b"nonexistent");
        let result = get_episode(&db, &id.raw());
        assert!(matches!(result, Err(StoreError::NotFound)));
    }

    #[test]
    fn test_get_missing_tool_sequence() {
        let (db, _dir) = test_db();
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

    // ── Recall engagement tests ─────────────────────────────

    #[test]
    fn test_record_surface_creates() {
        let (db, _dir) = test_db();
        let id = RawId::derive("test", b"artifact1");
        record_surface(&db, &id, "decision").unwrap();

        let eng = get_recall_engagement(&db, &id).unwrap();
        assert_eq!(eng.surface_count, 1);
        assert_eq!(eng.engagement_count, 0);
        assert_eq!(eng.artifact_type, "decision");
    }

    #[test]
    fn test_record_surface_increments() {
        let (db, _dir) = test_db();
        let id = RawId::derive("test", b"artifact1");
        record_surface(&db, &id, "decision").unwrap();
        record_surface(&db, &id, "decision").unwrap();
        record_surface(&db, &id, "decision").unwrap();

        let eng = get_recall_engagement(&db, &id).unwrap();
        assert_eq!(eng.surface_count, 3);
    }

    #[test]
    fn test_record_engagement() {
        let (db, _dir) = test_db();
        let id = RawId::derive("test", b"artifact1");
        record_surface(&db, &id, "summary").unwrap();
        record_engagement(&db, &id).unwrap();

        let eng = get_recall_engagement(&db, &id).unwrap();
        assert_eq!(eng.surface_count, 1);
        assert_eq!(eng.engagement_count, 1);
    }

    // -- Property: engagement_ratio is in [0, 1] --
    #[hegel::test(test_cases = 50)]
    fn prop_engagement_ratio_bounded(tc: TestCase) {
        let surface: u32 =
            tc.draw(gs::integers::<u32>().min_value(0).max_value(100));
        let engage: u32 =
            tc.draw(gs::integers::<u32>().min_value(0).max_value(surface));

        let eng = RecallEngagement {
            surfaced_id: RawId::derive("test", b"x"),
            artifact_type: "decision".into(),
            surfaced_ts_utc_ms: 1000,
            surface_count: surface,
            engagement_count: engage,
        };
        let ratio = eng.engagement_ratio();
        assert!(
            (0.0..=1.0).contains(&ratio),
            "ratio must be in [0,1]: {ratio}"
        );
    }

    // -- Property: is_ignored detects low engagement --
    #[hegel::test(test_cases = 50)]
    fn prop_ignored_threshold(tc: TestCase) {
        let surface: u32 =
            tc.draw(gs::integers::<u32>().min_value(3).max_value(20));

        let eng = RecallEngagement {
            surfaced_id: RawId::derive("test", b"x"),
            artifact_type: "decision".into(),
            surfaced_ts_utc_ms: 1000,
            surface_count: surface,
            engagement_count: 0,
        };
        // With 0 engagements, ratio is 0.0, which is < any positive threshold
        assert!(
            eng.is_ignored(3, 0.1),
            "zero engagement with {surface} surfaces should be ignored"
        );
    }

    // ── RepoProfile round-trip ─────────────────────────────────

    #[hegel::composite]
    fn gen_profile_fact(tc: hegel::TestCase) -> ProfileFact {
        let ep_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let first: i64 = tc.draw(
            gs::integers::<i64>()
                .min_value(1000)
                .max_value(i64::MAX / 2),
        );
        ProfileFact {
            statement: tc.draw(gs::text().min_size(1).max_size(100)),
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(&ep_input),
                span_summary: "test evidence".into(),
            }],
            first_seen_ts_utc_ms: first,
            last_confirmed_ts_utc_ms: first,
            support_count: tc
                .draw(gs::integers::<u32>().min_value(1).max_value(50)),
            confidence: tc.draw(gs::sampled_from(vec![
                Confidence::Low,
                Confidence::Medium,
                Confidence::High,
            ])),
        }
    }

    #[hegel::composite]
    fn gen_repo_profile(tc: hegel::TestCase) -> RepoProfile {
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let n_conv: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let n_pref: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let mut conventions = Vec::with_capacity(n_conv);
        for _ in 0..n_conv {
            conventions.push(tc.draw(gen_profile_fact()));
        }
        let mut preferences = Vec::with_capacity(n_pref);
        for _ in 0..n_pref {
            preferences.push(tc.draw(gen_profile_fact()));
        }
        RepoProfile {
            repo_id: RepoId::derive(&repo_input),
            conventions,
            preferences,
            updated_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            revision: "v1".into(),
        }
    }

    // -- Property: put then get RepoProfile = identity --
    #[hegel::test(test_cases = 50)]
    fn prop_repo_profile_roundtrip(tc: TestCase) {
        let profile = tc.draw(gen_repo_profile());
        let (db, _dir) = test_db();
        put_repo_profile(&db, &profile).expect("put");
        let loaded =
            get_repo_profile(&db, &profile.repo_id.raw()).expect("get");
        assert_eq!(profile, loaded);
    }

    // -- Property: put is idempotent --
    #[hegel::test(test_cases = 30)]
    fn prop_repo_profile_put_idempotent(tc: TestCase) {
        let profile = tc.draw(gen_repo_profile());
        let (db, _dir) = test_db();
        put_repo_profile(&db, &profile).expect("put 1");
        put_repo_profile(&db, &profile).expect("put 2");
        let loaded =
            get_repo_profile(&db, &profile.repo_id.raw()).expect("get");
        assert_eq!(profile, loaded);
    }

    // -- Unit: get missing profile returns NotFound --
    #[test]
    fn test_get_missing_repo_profile() {
        let (db, _dir) = test_db();
        let id = RepoId::derive(b"nonexistent");
        let result = get_repo_profile(&db, &id.raw());
        assert!(matches!(result, Err(StoreError::NotFound)));
    }

    // -- Property: update overwrites previous profile --
    #[hegel::test(test_cases = 30)]
    fn prop_repo_profile_update_overwrites(tc: TestCase) {
        let mut profile = tc.draw(gen_repo_profile());
        let (db, _dir) = test_db();
        put_repo_profile(&db, &profile).expect("put 1");

        // Mutate and re-put
        profile.revision = "v2".into();
        profile.conventions.push(ProfileFact {
            statement: "added later".into(),
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(b"new-ep"),
                span_summary: "new".into(),
            }],
            first_seen_ts_utc_ms: 5000,
            last_confirmed_ts_utc_ms: 5000,
            support_count: 1,
            confidence: Confidence::Low,
        });
        put_repo_profile(&db, &profile).expect("put 2");

        let loaded =
            get_repo_profile(&db, &profile.repo_id.raw()).expect("get");
        assert_eq!(loaded.revision, "v2");
        assert_eq!(loaded.conventions.len(), profile.conventions.len());
    }

    // -- Property: RecallEngagement serde round-trip --
    #[hegel::test(test_cases = 50)]
    fn prop_recall_engagement_roundtrip(tc: TestCase) {
        let eng = RecallEngagement {
            surfaced_id: RawId::derive(
                "test",
                &tc.draw(
                    gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16),
                ),
            ),
            artifact_type: tc.draw(gs::sampled_from(vec![
                "decision".to_string(),
                "summary".to_string(),
                "entity".to_string(),
            ])),
            surfaced_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            surface_count: tc
                .draw(gs::integers::<u32>().min_value(0).max_value(100)),
            engagement_count: tc
                .draw(gs::integers::<u32>().min_value(0).max_value(100)),
        };
        let json = serde_json::to_string(&eng).unwrap();
        let parsed: RecallEngagement = serde_json::from_str(&json).unwrap();
        assert_eq!(eng, parsed);
    }
}
