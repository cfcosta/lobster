//! Async Extractor trait interface and typed output schema.
//!
//! The extractor emits typed structured facts, not freeform graph
//! queries. Lobster's deterministic compiler converts these facts
//! into Grafeo CRUD operations.

use serde::{Deserialize, Serialize};

/// Input for graph extraction: the full episode bundle.
#[derive(Debug, Clone)]
pub struct ExtractionInput {
    pub summary_text: String,
    pub decisions_json: Vec<u8>,
    pub tool_outcomes_json: Vec<u8>,
    pub conversation_spans_json: Vec<u8>,
    pub repo_path: String,
}

/// A typed entity reference in the extraction output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub kind: String,
    pub name: String,
}

/// A typed relation between entities/tasks/decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedRelation {
    pub relation_type: RelationType,
    pub from: String,
    pub to: String,
}

/// The fixed set of relation types for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationType {
    TaskDecision,
    TaskEntity,
    DecisionEntity,
    EntityEntity,
}

/// A decision detected by the LLM extractor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedDecision {
    /// What was decided (one clear sentence).
    pub statement: String,
    /// Why it was decided.
    pub rationale: String,
    /// LLM's assessment: "high", "medium", or "low".
    pub confidence: String,
}

/// Typed structured facts emitted by the extractor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionOutput {
    pub task_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub entities: Vec<ExtractedEntity>,
    pub relations: Vec<ExtractedRelation>,
    #[serde(default)]
    pub decisions: Vec<ExtractedDecision>,
}

/// Error from the extraction pipeline.
#[derive(Debug)]
pub enum ExtractionError {
    ModelUnavailable(String),
    Timeout,
    InvalidOutput(String),
    ValidationFailed(String),
}

impl std::fmt::Display for ExtractionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelUnavailable(msg) => {
                write!(f, "model unavailable: {msg}")
            }
            Self::Timeout => write!(f, "extraction timed out"),
            Self::InvalidOutput(msg) => {
                write!(f, "invalid output: {msg}")
            }
            Self::ValidationFailed(msg) => {
                write!(f, "validation failed: {msg}")
            }
        }
    }
}

impl std::error::Error for ExtractionError {}

/// Async graph extraction interface.
///
/// Implementations:
/// - `rig-core`-backed LLM extractor (structured output)
/// - Heuristic deterministic extractor (offline fallback)
pub trait Extractor: Send + Sync {
    fn extract(
        &self,
        input: ExtractionInput,
    ) -> impl Future<Output = Result<ExtractionOutput, ExtractionError>> + Send;
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: ExtractionOutput serde round-trip --
    #[hegel::test(test_cases = 100)]
    fn prop_extraction_output_roundtrip(tc: TestCase) {
        let n_entities: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut entities = Vec::with_capacity(n_entities);
        for _ in 0..n_entities {
            entities.push(ExtractedEntity {
                kind: tc.draw(gs::sampled_from(vec![
                    "concept".to_string(),
                    "constraint".to_string(),
                    "component".to_string(),
                ])),
                name: tc.draw(gs::text().min_size(1).max_size(50)),
            });
        }
        let n_relations: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut relations = Vec::with_capacity(n_relations);
        for _ in 0..n_relations {
            relations.push(ExtractedRelation {
                relation_type: tc.draw(gs::sampled_from(vec![
                    RelationType::TaskDecision,
                    RelationType::TaskEntity,
                    RelationType::DecisionEntity,
                    RelationType::EntityEntity,
                ])),
                from: tc.draw(gs::text().min_size(1).max_size(30)),
                to: tc.draw(gs::text().min_size(1).max_size(30)),
            });
        }
        let output = ExtractionOutput {
            task_refs: vec![],
            decision_refs: vec![],
            entities,
            relations,
            decisions: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        let parsed: ExtractionOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, parsed);
    }

    // -- Property: RelationType serde round-trip --
    #[hegel::test(test_cases = 100)]
    fn prop_relation_type_roundtrip(tc: TestCase) {
        let rt: RelationType = tc.draw(gs::sampled_from(vec![
            RelationType::TaskDecision,
            RelationType::TaskEntity,
            RelationType::DecisionEntity,
            RelationType::EntityEntity,
        ]));
        let json = serde_json::to_string(&rt).unwrap();
        let parsed: RelationType = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, parsed);
    }

    struct MockExtractor;

    impl Extractor for MockExtractor {
        async fn extract(
            &self,
            _input: ExtractionInput,
        ) -> Result<ExtractionOutput, ExtractionError> {
            Ok(ExtractionOutput {
                task_refs: vec![],
                decision_refs: vec![],
                entities: vec![ExtractedEntity {
                    kind: "component".into(),
                    name: "Grafeo".into(),
                }],
                relations: vec![],
                decisions: vec![],
            })
        }
    }

    #[tokio::test]
    async fn test_mock_extractor() {
        let extractor = MockExtractor;
        let input = ExtractionInput {
            summary_text: "test".into(),
            decisions_json: b"[]".to_vec(),
            tool_outcomes_json: b"[]".to_vec(),
            conversation_spans_json: b"[]".to_vec(),
            repo_path: "/test".into(),
        };
        let result = extractor.extract(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().entities[0].name, "Grafeo");
    }
}
