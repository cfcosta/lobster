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
    WorkflowId,
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
    /// Structured premises supporting the decision (optional).
    ///
    /// When present, the rationale becomes a conclusion derived from
    /// these premises. Future retrieval can evaluate whether the
    /// reasoning still holds by checking if premises are contradicted
    /// by later episodes.
    #[serde(default)]
    pub premises: Vec<String>,
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
    /// A recurring tool-use workflow detected across episodes.
    Workflow,
}

/// A semantic entity (concept, constraint, component, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: EntityId,
    pub repo_id: RepoId,
    pub kind: EntityKind,
    pub canonical_name: String,
    /// Episode where this entity was first observed.
    #[serde(default)]
    pub first_seen_episode: Option<EpisodeId>,
    /// Timestamp of the most recent episode mentioning this entity.
    #[serde(default)]
    pub last_seen_ts_utc_ms: Option<i64>,
    /// Number of episodes that mention this entity.
    #[serde(default)]
    pub mention_count: u32,
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

// ── ToolSequence (procedural memory) ────────────────────────

/// A recurring tool-use pattern detected across multiple episodes.
///
/// Tool sequences capture procedural knowledge: *how* work gets done,
/// not just *what* happened. When the same sequence of tool uses
/// appears across multiple episodes, it's promoted into a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSequence {
    pub workflow_id: WorkflowId,
    pub repo_id: RepoId,
    /// The ordered pattern of event kinds that recurs.
    pub pattern: Vec<EventKind>,
    /// Human-readable label derived from the pattern.
    pub label: String,
    /// Number of episodes where this pattern was observed.
    pub frequency: u32,
    /// Episodes that contain this pattern.
    pub source_episodes: Vec<EpisodeId>,
    /// Timestamp when this workflow was first detected.
    pub detected_ts_utc_ms: i64,
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
            premises: vec![],
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

    // -- Property: Decision with premises round-trips --
    #[hegel::test(test_cases = 100)]
    fn prop_decision_with_premises_roundtrip(tc: TestCase) {
        let mut decision = tc.draw(gen_decision());
        let n_premises: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut premises = Vec::with_capacity(n_premises);
        for _ in 0..n_premises {
            premises.push(tc.draw(gs::text().min_size(1).max_size(100)));
        }
        decision.premises = premises;

        let json = serde_json::to_string(&decision).unwrap();
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(decision, parsed);
    }

    // -- Property: Decision without premises deserializes with empty vec --
    #[test]
    fn test_decision_backward_compat() {
        // Serialize a decision, strip the premises field, then
        // deserialize to verify backward compatibility
        let dec = Decision {
            decision_id: DecisionId::derive(b"bc"),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(b"ep"),
            task_id: None,
            statement: "test".into(),
            rationale: "reason".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1000,
            valid_to_ts_utc_ms: None,
            evidence: vec![],
            premises: vec!["premise 1".into()],
        };
        let mut json_val: serde_json::Value =
            serde_json::to_value(&dec).unwrap();
        // Remove premises to simulate old serialization format
        json_val.as_object_mut().unwrap().remove("premises");
        let json = serde_json::to_string(&json_val).unwrap();

        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert!(
            parsed.premises.is_empty(),
            "missing premises field should default to empty vec"
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

    // -- Property: Entity with evolution fields round-trips --
    #[hegel::test(test_cases = 100)]
    fn prop_entity_evolution_roundtrip(tc: TestCase) {
        let id_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));

        let entity = Entity {
            entity_id: EntityId::derive(&id_input),
            repo_id: RepoId::derive(&repo_input),
            kind: tc.draw(gs::sampled_from(vec![
                EntityKind::Concept,
                EntityKind::Component,
                EntityKind::Workflow,
            ])),
            canonical_name: tc.draw(gs::text().min_size(1).max_size(50)),
            first_seen_episode: if tc.draw(gs::booleans()) {
                Some(EpisodeId::derive(&id_input))
            } else {
                None
            },
            last_seen_ts_utc_ms: if tc.draw(gs::booleans()) {
                Some(tc.draw(
                    gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
                ))
            } else {
                None
            },
            mention_count: tc
                .draw(gs::integers::<u32>().min_value(0).max_value(1000)),
        };

        let json = serde_json::to_string(&entity).unwrap();
        let parsed: Entity = serde_json::from_str(&json).unwrap();
        assert_eq!(entity, parsed);
    }

    // -- Property: Entity backward compat (missing evolution fields) --
    #[test]
    fn test_entity_backward_compat() {
        let entity = Entity {
            entity_id: EntityId::derive(b"bc"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "test".into(),
            first_seen_episode: Some(EpisodeId::derive(b"ep1")),
            last_seen_ts_utc_ms: Some(1000),
            mention_count: 5,
        };
        let mut json_val: serde_json::Value =
            serde_json::to_value(&entity).unwrap();
        let obj = json_val.as_object_mut().unwrap();
        obj.remove("first_seen_episode");
        obj.remove("last_seen_ts_utc_ms");
        obj.remove("mention_count");
        let json = serde_json::to_string(&json_val).unwrap();

        let parsed: Entity = serde_json::from_str(&json).unwrap();
        assert!(parsed.first_seen_episode.is_none());
        assert!(parsed.last_seen_ts_utc_ms.is_none());
        assert_eq!(parsed.mention_count, 0);
    }

    #[test]
    fn test_entity_kind_variants() {
        let kinds = [
            EntityKind::Concept,
            EntityKind::Constraint,
            EntityKind::Component,
            EntityKind::FileLite,
            EntityKind::Repo,
            EntityKind::Workflow,
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

    // -- ToolSequence generators and property tests --

    /// All EventKind variants for generation.
    fn all_event_kinds() -> Vec<EventKind> {
        vec![
            EventKind::UserPromptSubmit,
            EventKind::AssistantResponse,
            EventKind::ToolUse,
            EventKind::ToolResult,
            EventKind::ToolUseFailure,
            EventKind::FileRead,
            EventKind::FileWrite,
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
            EventKind::PlanTransition,
        ]
    }

    #[hegel::composite]
    fn gen_event_kind(tc: hegel::TestCase) -> EventKind {
        tc.draw(gs::sampled_from(all_event_kinds()))
    }

    #[hegel::composite]
    fn gen_tool_sequence(tc: hegel::TestCase) -> ToolSequence {
        let wf_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let pattern_len: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(10));
        let mut pattern = Vec::with_capacity(pattern_len);
        for _ in 0..pattern_len {
            pattern.push(tc.draw(gen_event_kind()));
        }
        let freq: u32 =
            tc.draw(gs::integers::<u32>().min_value(2).max_value(100));
        let n_sources: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(10));
        let mut source_episodes = Vec::with_capacity(n_sources);
        for i in 0..n_sources {
            let mut ep_input = wf_input.clone();
            #[allow(clippy::cast_possible_truncation)]
            ep_input.extend_from_slice(&(i as u32).to_le_bytes());
            source_episodes.push(EpisodeId::derive(&ep_input));
        }
        ToolSequence {
            workflow_id: crate::store::ids::WorkflowId::derive(&wf_input),
            repo_id: RepoId::derive(&repo_input),
            pattern,
            label: tc.draw(gs::text().min_size(1).max_size(100)),
            frequency: freq,
            source_episodes,
            detected_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
        }
    }

    // -- Property: ToolSequence serde round-trip --
    #[hegel::test(test_cases = 200)]
    fn prop_tool_sequence_serde_roundtrip(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        let json = serde_json::to_string(&ts).unwrap();
        let parsed: ToolSequence = serde_json::from_str(&json).unwrap();
        assert_eq!(ts, parsed);
    }

    // -- Property: ToolSequence pattern has at least 2 elements --
    // Generated by construction (min_value 2) but verified.
    #[hegel::test(test_cases = 200)]
    fn prop_tool_sequence_pattern_min_length(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        assert!(
            ts.pattern.len() >= 2,
            "workflow patterns must have at least 2 steps"
        );
    }

    // -- Property: ToolSequence frequency >= 2 --
    // A workflow only exists if it recurs.
    #[hegel::test(test_cases = 200)]
    fn prop_tool_sequence_min_frequency(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        assert!(
            ts.frequency >= 2,
            "workflow must appear in at least 2 episodes"
        );
    }

    // -- Property: ToolSequence source_episodes is non-empty --
    #[hegel::test(test_cases = 200)]
    fn prop_tool_sequence_has_sources(tc: TestCase) {
        let ts = tc.draw(gen_tool_sequence());
        assert!(
            !ts.source_episodes.is_empty(),
            "workflow must have source episodes"
        );
    }

    // -- Property: WorkflowId round-trip --
    #[hegel::test(test_cases = 200)]
    fn prop_workflow_id_roundtrip(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));
        let id = crate::store::ids::WorkflowId::derive(&input);
        let s = id.to_string();
        let parsed: crate::store::ids::WorkflowId =
            s.parse().expect("valid hex");
        assert_eq!(id, parsed);
    }
}
