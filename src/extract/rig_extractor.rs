//! rig-core backed unified episode analyzer.
//!
//! Merges summarization and extraction into a single LLM call using
//! rig's structured extraction (tool-call based). The model returns
//! a typed `EpisodeAnalysis` matching the `JsonSchema` derive.
//!
//! Uses `app::llm::extract` which reads provider and model from env:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: claude-sonnet-4-6)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: gpt-5.4-mini)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::extract::traits::{
    ExtractedDecision,
    ExtractedEntity,
    ExtractedRelation,
    ExtractionError,
    ExtractionOutput,
    RelationType,
};

/// Unified LLM output: summary + structured extraction in one call.
///
/// Every field is annotated with `schemars` doc attributes so the
/// JSON schema sent to the model describes what each field expects.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EpisodeAnalysis {
    /// A concise third-person past-tense summary of the work session
    /// (plain prose, no markdown, under 300 words). Focus on what
    /// changed, why, what was decided, and what files were touched.
    /// If the session contains no meaningful work, write
    /// "No significant changes."
    pub summary: String,

    /// Technical decisions made during the session. Only include
    /// genuine decisions (architecture, design, tool choice,
    /// approach), not routine actions. Empty array if none.
    #[serde(default)]
    pub decisions: Vec<AnalysisDecision>,

    /// Notable entities mentioned that would be useful to recall in
    /// future sessions. Prefer components and constraints over
    /// generic concepts.
    #[serde(default)]
    pub entities: Vec<AnalysisEntity>,

    /// Relationships between entities, decisions, and tasks.
    #[serde(default)]
    pub relations: Vec<AnalysisRelation>,

    /// Task title strings referenced in the session.
    #[serde(default)]
    pub task_refs: Vec<String>,

    /// Decision statement strings referenced in the session.
    #[serde(default)]
    pub decision_refs: Vec<String>,

    /// Repository conventions detected from the session: build tools,
    /// test frameworks, VCS, languages, etc. Each string is a short
    /// factual statement like "Rust project using Cargo" or "uses
    /// jujutsu (jj) for version control". Only include conventions
    /// that are clearly evidenced by the session events. Empty array
    /// if none detected.
    #[serde(default)]
    pub conventions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisDecision {
    /// One clear sentence stating what was decided.
    pub statement: String,
    /// Why it was decided (one sentence).
    pub rationale: String,
    /// "high" (explicit choice), "medium" (implied), or "low" (uncertain).
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisEntity {
    /// One of: "component", "constraint", "file-lite", "concept", "repo".
    pub kind: String,
    /// Canonical name of the entity.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisRelation {
    /// One of: `EntityEntity`, `DecisionEntity`, `TaskEntity`, `TaskDecision`.
    pub relation_type: String,
    /// Name of the source entity.
    pub from: String,
    /// Name of the target entity.
    pub to: String,
}

const PREAMBLE: &str = "\
You analyze developer work sessions and produce both a summary and structured facts.\n\
\n\
Summary rules:\n\
- Write in third person past tense (\"The developer added...\", not \"I will...\").\n\
- Focus on: what changed, why, what was decided, what files were touched.\n\
- Omit tool call syntax, JSON payloads, and raw command output.\n\
- If the session contains no meaningful work, write \"No significant changes.\"\n\
- Keep the summary under 300 words.\n\
- Do NOT use markdown headers, bullet points, or formatting.\n\
- Write plain prose paragraphs only.\n\
\n\
Extraction rules:\n\
- Only extract what is explicitly stated. Do not invent.\n\
- Only include genuine technical decisions (architecture, design, tool choice, approach).\n\
  Do NOT include routine actions like \"ran tests\" or \"read a file\".\n\
- Only extract entities that belong to or are directly relevant to the repository.\n\
  Do NOT extract entities from other projects, test fixtures, or example code.\n\
- Prefer fewer, higher-quality items over exhaustive extraction.\n\
- If nothing meaningful was decided, return an empty decisions array.\n\
\n\
Convention detection rules:\n\
- Detect repository conventions from the session events: build tools, test\n\
  frameworks, VCS, languages, CI systems, linters, formatters, etc.\n\
- Each convention should be a short factual statement (e.g., \"Rust project\n\
  using Cargo\", \"uses nix flakes for builds\", \"uses jujutsu for version\n\
  control\", \"uses hegel property-based testing\").\n\
- Only include conventions clearly evidenced in the events (file paths,\n\
  commands, tool usage). Do not guess or infer from entity names alone.\n\
- Conventions should be stable facts about the repository, not session-specific.";

/// Analyze an episode in a single LLM call: summarize + extract.
///
/// # Errors
///
/// Returns `ExtractionError` if the API key is missing or the call fails.
pub async fn analyze(prompt: &str) -> Result<EpisodeAnalysis, ExtractionError> {
    crate::app::llm::extract::<EpisodeAnalysis>(PREAMBLE, prompt)
        .await
        .map_err(ExtractionError::ModelUnavailable)
}

/// Convert an `EpisodeAnalysis` into the `ExtractionOutput` used by
/// downstream pipeline stages.
impl From<&EpisodeAnalysis> for ExtractionOutput {
    fn from(a: &EpisodeAnalysis) -> Self {
        Self {
            task_refs: a.task_refs.clone(),
            decision_refs: a.decision_refs.clone(),
            entities: a
                .entities
                .iter()
                .map(|e| ExtractedEntity {
                    kind: e.kind.clone(),
                    name: e.name.clone(),
                })
                .collect(),
            relations: a
                .relations
                .iter()
                .filter_map(|r| {
                    let relation_type = match r.relation_type.as_str() {
                        "TaskDecision" => RelationType::TaskDecision,
                        "TaskEntity" => RelationType::TaskEntity,
                        "DecisionEntity" => RelationType::DecisionEntity,
                        "EntityEntity" => RelationType::EntityEntity,
                        _ => return None,
                    };
                    Some(ExtractedRelation {
                        relation_type,
                        from: r.from.clone(),
                        to: r.to.clone(),
                    })
                })
                .collect(),
            decisions: a
                .decisions
                .iter()
                .map(|d| ExtractedDecision {
                    statement: d.statement.clone(),
                    rationale: d.rationale.clone(),
                    confidence: d.confidence.clone(),
                })
                .collect(),
            conventions: a.conventions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_to_extraction_output() {
        let analysis = EpisodeAnalysis {
            summary: "The developer fixed a bug.".into(),
            decisions: vec![AnalysisDecision {
                statement: "Use redb for storage".into(),
                rationale: "ACID compliance".into(),
                confidence: "high".into(),
            }],
            entities: vec![AnalysisEntity {
                kind: "component".into(),
                name: "redb".into(),
            }],
            relations: vec![AnalysisRelation {
                relation_type: "DecisionEntity".into(),
                from: "Use redb for storage".into(),
                to: "redb".into(),
            }],
            task_refs: vec![],
            decision_refs: vec![],
            conventions: vec!["Rust project using Cargo".into()],
        };

        let output = ExtractionOutput::from(&analysis);
        assert_eq!(output.decisions.len(), 1);
        assert_eq!(output.entities.len(), 1);
        assert_eq!(output.relations.len(), 1);
        assert_eq!(output.decisions[0].statement, "Use redb for storage");
        assert_eq!(analysis.conventions, vec!["Rust project using Cargo"]);
    }

    #[test]
    fn test_unknown_relation_type_filtered() {
        let analysis = EpisodeAnalysis {
            summary: String::new(),
            decisions: vec![],
            entities: vec![],
            relations: vec![AnalysisRelation {
                relation_type: "Unknown".into(),
                from: "a".into(),
                to: "b".into(),
            }],
            task_refs: vec![],
            decision_refs: vec![],
            conventions: vec![],
        };

        let output = ExtractionOutput::from(&analysis);
        assert!(output.relations.is_empty());
    }

    #[test]
    fn test_empty_analysis() {
        let analysis = EpisodeAnalysis {
            summary: "No significant changes.".into(),
            decisions: vec![],
            entities: vec![],
            relations: vec![],
            task_refs: vec![],
            decision_refs: vec![],
            conventions: vec![],
        };

        let output = ExtractionOutput::from(&analysis);
        assert!(output.decisions.is_empty());
        assert!(output.entities.is_empty());
        assert!(output.relations.is_empty());
    }

    #[tokio::test]
    async fn test_analyze_requires_api_key() {
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let result = analyze("test").await;
            assert!(matches!(
                result,
                Err(ExtractionError::ModelUnavailable(_))
            ));
        }
    }

    #[test]
    fn test_episode_analysis_json_schema() {
        // Verify the schema generates without panic
        let schema = schemars::schema_for!(EpisodeAnalysis);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("summary"));
        assert!(json.contains("decisions"));
        assert!(json.contains("entities"));
        assert!(json.contains("conventions"));
    }

    use hegel::{TestCase, generators as gs};

    #[hegel::test(test_cases = 200)]
    fn prop_analysis_serde_roundtrip(tc: TestCase) {
        let n_decisions: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let n_entities: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));

        let decisions: Vec<AnalysisDecision> = (0..n_decisions)
            .map(|_| AnalysisDecision {
                statement: tc.draw(
                    gs::text()
                        .min_size(1)
                        .max_size(50)
                        .alphabet("abcdefghijklmnopqrstuvwxyz "),
                ),
                rationale: tc.draw(
                    gs::text()
                        .min_size(1)
                        .max_size(50)
                        .alphabet("abcdefghijklmnopqrstuvwxyz "),
                ),
                confidence: tc.draw(gs::sampled_from(vec![
                    "high".to_string(),
                    "medium".to_string(),
                    "low".to_string(),
                ])),
            })
            .collect();

        let entities: Vec<AnalysisEntity> = (0..n_entities)
            .map(|_| AnalysisEntity {
                kind: tc.draw(gs::sampled_from(vec![
                    "component".to_string(),
                    "concept".to_string(),
                    "constraint".to_string(),
                ])),
                name: tc.draw(
                    gs::text()
                        .min_size(1)
                        .max_size(30)
                        .alphabet("abcdefghijklmnopqrstuvwxyz"),
                ),
            })
            .collect();

        let analysis = EpisodeAnalysis {
            summary: tc.draw(
                gs::text()
                    .min_size(1)
                    .max_size(200)
                    .alphabet("abcdefghijklmnopqrstuvwxyz ."),
            ),
            decisions,
            entities,
            relations: vec![],
            task_refs: vec![],
            decision_refs: vec![],
            conventions: vec![],
        };

        let json = serde_json::to_string(&analysis).unwrap();
        let parsed: EpisodeAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(analysis.summary, parsed.summary);
        assert_eq!(analysis.decisions.len(), parsed.decisions.len());
        assert_eq!(analysis.entities.len(), parsed.entities.len());
    }

    #[hegel::test(test_cases = 200)]
    fn prop_conversion_preserves_decision_count(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));

        let decisions: Vec<AnalysisDecision> = (0..n)
            .map(|_| AnalysisDecision {
                statement: tc.draw(
                    gs::text()
                        .min_size(1)
                        .max_size(50)
                        .alphabet("abcdefghijklmnopqrstuvwxyz "),
                ),
                rationale: tc.draw(
                    gs::text()
                        .min_size(1)
                        .max_size(50)
                        .alphabet("abcdefghijklmnopqrstuvwxyz "),
                ),
                confidence: tc.draw(gs::sampled_from(vec![
                    "high".to_string(),
                    "medium".to_string(),
                    "low".to_string(),
                ])),
            })
            .collect();

        let analysis = EpisodeAnalysis {
            summary: "test".into(),
            decisions,
            entities: vec![],
            relations: vec![],
            task_refs: vec![],
            decision_refs: vec![],
            conventions: vec![],
        };

        let output = ExtractionOutput::from(&analysis);
        assert_eq!(output.decisions.len(), n);
    }
}
