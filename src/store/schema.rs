//! Core data model types for Lobster's canonical storage.
//!
//! Every type here maps directly to the architecture spec data model.
//! These are the durable records stored in redb.

use serde::{Deserialize, Serialize};

use crate::store::ids::{
    ArtifactId,
    DecisionId,
    EntityId,
    EpisodeId,
    RepoId,
    TaskId,
};

// ── EvidenceRef (d49.10) ─────────────────────────────────────

/// Links a decision or graph edge back to its source evidence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub episode_id: EpisodeId,
    pub span_summary: String,
}

// ── EventKind + RawEvent (d49.2) ─────────────────────────────

/// Classification of hook/tool/conversation events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    UserPromptSubmit,
    AssistantResponse,
    ToolUse,
    ToolResult,
    ToolUseFailure,
    FileRead,
    FileWrite,
    FileEdit,
    TestRun,
    TestResult,
    PlanTransition,
}

/// A raw hook event as captured from Claude Code.
///
/// Raw events are the canonical truth layer — they are appended
/// to redb immediately and never modified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEvent {
    pub seq: u64,
    pub repo_id: RepoId,
    pub ts_utc_ms: i64,
    pub event_kind: EventKind,
    pub payload_hash: [u8; 32],
    pub payload_bytes: Vec<u8>,
}

// ── ProcessingState + Episode (d49.3) ────────────────────────

/// Lifecycle state of an episode through the finalization pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessingState {
    Pending,
    Ready,
    RetryQueued,
    FailedFinal,
}

/// A coherent work segment derived from raw events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub episode_id: EpisodeId,
    pub repo_id: RepoId,
    pub start_seq: u64,
    pub end_seq: u64,
    pub task_id: Option<TaskId>,
    pub processing_state: ProcessingState,
    pub finalized_ts_utc_ms: i64,
    /// Number of extraction retry attempts. Spec says mark
    /// `RetryQueued` after first failure, `FailedFinal` after second.
    #[serde(default)]
    pub retry_count: u32,
    /// Flagged as low-signal during dreaming. Reduces ranking score.
    #[serde(default)]
    pub is_noisy: bool,
}

// ── Confidence + Decision (d49.4) ────────────────────────────

/// Discrete confidence level for detected decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// A detected decision with rationale and evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub decision_id: DecisionId,
    pub repo_id: RepoId,
    pub episode_id: EpisodeId,
    pub task_id: Option<TaskId>,
    pub statement: String,
    pub rationale: String,
    pub confidence: Confidence,
    pub valid_from_ts_utc_ms: i64,
    pub valid_to_ts_utc_ms: Option<i64>,
    pub evidence: Vec<EvidenceRef>,
}

// ── TaskStatus + Task (d49.5) ────────────────────────────────

/// Lifecycle status of a persistent work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Abandoned,
}

/// A persistent work item that spans episodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub title: String,
    pub status: TaskStatus,
    pub opened_in: EpisodeId,
    pub last_seen_in: EpisodeId,
}

// ── EntityKind + Entity (d49.6) ──────────────────────────────

/// Kind of semantic entity in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind {
    Concept,
    Constraint,
    Component,
    FileLite,
    Repo,
}

/// A semantic entity (concept, constraint, component, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: EntityId,
    pub repo_id: RepoId,
    pub kind: EntityKind,
    pub canonical_name: String,
}

// ── SummaryArtifact (d49.7) ──────────────────────────────────

/// A versioned summary produced by the summarization pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryArtifact {
    pub episode_id: EpisodeId,
    pub revision: String,
    pub summary_text: String,
    pub payload_checksum: [u8; 32],
}

// ── ExtractionArtifact (d49.8) ───────────────────────────────

/// A versioned graph extraction output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionArtifact {
    pub episode_id: EpisodeId,
    pub revision: String,
    pub output_json: Vec<u8>,
    pub payload_checksum: [u8; 32],
}

// ── EmbeddingBackend + EmbeddingArtifact (d49.9) ─────────────

/// Backend used for embedding inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmbeddingBackend {
    Cpu,
    Cuda,
    Metal,
    Mkl,
}

/// A versioned embedding artifact with pooled and optional
/// late-interaction representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingArtifact {
    pub artifact_id: ArtifactId,
    pub revision: String,
    pub backend: EmbeddingBackend,
    pub quantization: Option<String>,
    pub pooled_vector_bytes: Vec<u8>,
    pub late_interaction_bytes: Option<Vec<u8>>,
    pub payload_checksum: [u8; 32],
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Helpers to generate valid instances by construction --

    #[hegel::composite]
    fn gen_evidence_ref(tc: hegel::TestCase) -> EvidenceRef {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(32));
        EvidenceRef {
            episode_id: EpisodeId::derive(&input),
            span_summary: tc.draw(gs::text().min_size(1).max_size(100)),
        }
    }

    #[hegel::composite]
    fn gen_raw_event(tc: hegel::TestCase) -> RawEvent {
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let kind: EventKind = tc.draw(gs::sampled_from(vec![
            EventKind::UserPromptSubmit,
            EventKind::AssistantResponse,
            EventKind::ToolUse,
            EventKind::ToolResult,
            EventKind::FileRead,
            EventKind::FileWrite,
            EventKind::TestRun,
        ]));
        RawEvent {
            seq: tc.draw(gs::integers::<u64>()),
            repo_id: RepoId::derive(&repo_input),
            ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            event_kind: kind,
            payload_hash: tc.draw(gs::arrays(gs::integers::<u8>())),
            payload_bytes: tc
                .draw(gs::vecs(gs::integers::<u8>()).max_size(256)),
        }
    }

    #[hegel::composite]
    fn gen_episode(tc: hegel::TestCase) -> Episode {
        let ep_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let start: u64 =
            tc.draw(gs::integers::<u64>().min_value(0).max_value(1_000_000));
        let end: u64 = tc.draw(
            gs::integers::<u64>()
                .min_value(start)
                .max_value(start + 10_000),
        );
        let state: ProcessingState = tc.draw(gs::sampled_from(vec![
            ProcessingState::Pending,
            ProcessingState::Ready,
            ProcessingState::RetryQueued,
            ProcessingState::FailedFinal,
        ]));
        Episode {
            episode_id: EpisodeId::derive(&ep_input),
            repo_id: RepoId::derive(&repo_input),
            start_seq: start,
            end_seq: end,
            task_id: None,
            processing_state: state,
            finalized_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            retry_count: 0,
            is_noisy: false,
        }
    }

    #[hegel::composite]
    fn gen_decision(tc: hegel::TestCase) -> Decision {
        let dec_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let ep_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let conf: Confidence = tc.draw(gs::sampled_from(vec![
            Confidence::Low,
            Confidence::Medium,
            Confidence::High,
        ]));
        let n_evidence: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
        let mut evidence = Vec::with_capacity(n_evidence);
        for _ in 0..n_evidence {
            evidence.push(tc.draw(gen_evidence_ref()));
        }
        Decision {
            decision_id: DecisionId::derive(&dec_input),
            repo_id: RepoId::derive(&repo_input),
            episode_id: EpisodeId::derive(&ep_input),
            task_id: None,
            statement: tc.draw(gs::text().min_size(1).max_size(200)),
            rationale: tc.draw(gs::text().min_size(1).max_size(200)),
            confidence: conf,
            valid_from_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
            valid_to_ts_utc_ms: None,
            evidence,
        }
    }

    // -- Property: serde JSON round-trip for every type --
    // Oracle: round-trip (serialize then deserialize = identity).

    #[hegel::test(test_cases = 200)]
    fn prop_raw_event_serde_roundtrip(tc: TestCase) {
        let event = tc.draw(gen_raw_event());
        let json = serde_json::to_string(&event).unwrap();
        let parsed: RawEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[hegel::test(test_cases = 200)]
    fn prop_episode_serde_roundtrip(tc: TestCase) {
        let episode = tc.draw(gen_episode());
        let json = serde_json::to_string(&episode).unwrap();
        let parsed: Episode = serde_json::from_str(&json).unwrap();
        assert_eq!(episode, parsed);
    }

    #[hegel::test(test_cases = 200)]
    fn prop_decision_serde_roundtrip(tc: TestCase) {
        let decision = tc.draw(gen_decision());
        let json = serde_json::to_string(&decision).unwrap();
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(decision, parsed);
    }

    #[hegel::test(test_cases = 100)]
    fn prop_evidence_ref_serde_roundtrip(tc: TestCase) {
        let er = tc.draw(gen_evidence_ref());
        let json = serde_json::to_string(&er).unwrap();
        let parsed: EvidenceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(er, parsed);
    }

    // -- Property: Episode invariant start_seq <= end_seq --
    // Generated by construction but verified as defense.
    #[hegel::test(test_cases = 200)]
    fn prop_episode_seq_ordering(tc: TestCase) {
        let episode = tc.draw(gen_episode());
        assert!(
            episode.start_seq <= episode.end_seq,
            "start must not exceed end"
        );
    }

    // -- Property: Decision always has at least one evidence ref --
    // Generated by construction (min 1) but verified.
    #[hegel::test(test_cases = 200)]
    fn prop_decision_has_evidence(tc: TestCase) {
        let decision = tc.draw(gen_decision());
        assert!(
            !decision.evidence.is_empty(),
            "promoted decisions must have evidence"
        );
    }

    // -- Unit tests for enum variant coverage --

    #[test]
    fn test_processing_state_variants() {
        let states = [
            ProcessingState::Pending,
            ProcessingState::Ready,
            ProcessingState::RetryQueued,
            ProcessingState::FailedFinal,
        ];
        for s in &states {
            let json = serde_json::to_string(s).unwrap();
            let parsed: ProcessingState = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, parsed);
        }
    }

    #[test]
    fn test_entity_kind_variants() {
        let kinds = [
            EntityKind::Concept,
            EntityKind::Constraint,
            EntityKind::Component,
            EntityKind::FileLite,
            EntityKind::Repo,
        ];
        for k in &kinds {
            let json = serde_json::to_string(k).unwrap();
            let parsed: EntityKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*k, parsed);
        }
    }

    #[test]
    fn test_embedding_backend_variants() {
        let backends = [
            EmbeddingBackend::Cpu,
            EmbeddingBackend::Cuda,
            EmbeddingBackend::Metal,
            EmbeddingBackend::Mkl,
        ];
        for b in &backends {
            let json = serde_json::to_string(b).unwrap();
            let parsed: EmbeddingBackend = serde_json::from_str(&json).unwrap();
            assert_eq!(*b, parsed);
        }
    }

    #[test]
    fn test_task_status_variants() {
        let statuses = [
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Completed,
            TaskStatus::Abandoned,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, parsed);
        }
    }

    #[test]
    fn test_summary_artifact_roundtrip() {
        let input = b"test-episode";
        let sa = SummaryArtifact {
            episode_id: EpisodeId::derive(input),
            revision: "v1.0".into(),
            summary_text: "User fixed a bug".into(),
            payload_checksum: [0xAB; 32],
        };
        let json = serde_json::to_string(&sa).unwrap();
        let parsed: SummaryArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(sa, parsed);
    }

    #[test]
    fn test_extraction_artifact_roundtrip() {
        let ea = ExtractionArtifact {
            episode_id: EpisodeId::derive(b"ep"),
            revision: "v0.1".into(),
            output_json: b"{\"entities\":[]}".to_vec(),
            payload_checksum: [0xCD; 32],
        };
        let json = serde_json::to_string(&ea).unwrap();
        let parsed: ExtractionArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(ea, parsed);
    }

    #[test]
    fn test_embedding_artifact_roundtrip() {
        let ea = EmbeddingArtifact {
            artifact_id: ArtifactId::derive(b"emb"),
            revision: "v1.0".into(),
            backend: EmbeddingBackend::Cpu,
            quantization: None,
            pooled_vector_bytes: vec![0.0_f32; 128]
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect(),
            late_interaction_bytes: None,
            payload_checksum: [0xEF; 32],
        };
        let json = serde_json::to_string(&ea).unwrap();
        let parsed: EmbeddingArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(ea, parsed);
    }
}
