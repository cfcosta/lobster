//! Heuristic deterministic extractor for offline use.
//!
//! Extracts entities and relations from structured episode data
//! using pattern matching. No LLM required.

use crate::extract::traits::{
    ExtractedEntity,
    ExtractedRelation,
    ExtractionError,
    ExtractionInput,
    ExtractionOutput,
    Extractor,
    RelationType,
};

/// Deterministic rule-based extractor.
pub struct HeuristicExtractor;

impl Extractor for HeuristicExtractor {
    async fn extract(
        &self,
        input: ExtractionInput,
    ) -> Result<ExtractionOutput, ExtractionError> {
        let mut entities = Vec::new();
        let mut relations = Vec::new();
        let task_refs = Vec::new();
        let mut decision_refs = Vec::new();

        // Extract entities from summary text using simple
        // heuristics
        extract_entities_from_text(&input.summary_text, &mut entities);

        // Parse decisions JSON to find decision refs
        if let Ok(decisions) = serde_json::from_slice::<Vec<serde_json::Value>>(
            &input.decisions_json,
        ) {
            for dec in &decisions {
                if let Some(id) =
                    dec.get("decision_id").and_then(serde_json::Value::as_str)
                {
                    decision_refs.push(id.to_string());

                    // Create decision->entity relations only when
                    // we have a valid decision ref
                    for entity in &entities {
                        relations.push(ExtractedRelation {
                            relation_type: RelationType::DecisionEntity,
                            from: id.to_string(),
                            to: entity.name.clone(),
                        });
                    }
                }
            }
        }

        // If no entities or refs found, extract from repo path
        if entities.is_empty()
            && task_refs.is_empty()
            && decision_refs.is_empty()
        {
            entities.push(ExtractedEntity {
                kind: "repo".to_string(),
                name: input.repo_path.clone(),
            });
        }

        Ok(ExtractionOutput {
            task_refs,
            decision_refs,
            entities,
            relations,
        })
    }
}

fn extract_entities_from_text(text: &str, entities: &mut Vec<ExtractedEntity>) {
    let lower = text.to_lowercase();

    // Look for component mentions
    let component_keywords =
        ["grafeo", "redb", "pylate", "tantivy", "rig-core"];
    for kw in &component_keywords {
        if lower.contains(kw) {
            entities.push(ExtractedEntity {
                kind: "component".to_string(),
                name: (*kw).to_string(),
            });
        }
    }

    // Look for constraint language
    if lower.contains("must not")
        || lower.contains("never")
        || lower.contains("forbidden")
    {
        // Extract the constraint sentence
        if let Some(sentence) = extract_constraint_sentence(&lower) {
            entities.push(ExtractedEntity {
                kind: "constraint".to_string(),
                name: sentence,
            });
        }
    }
}

fn extract_constraint_sentence(text: &str) -> Option<String> {
    for pattern in &["must not", "never", "forbidden"] {
        if let Some(pos) = text.find(pattern) {
            let start = text[..pos]
                .rfind(['.', '!', '?', '\n'])
                .map_or(0, |p| p + 1);
            let end = text[pos..]
                .find(['.', '!', '?', '\n'])
                .map_or(text.len(), |p| pos + p);
            let sentence = text[start..end].trim().to_string();
            if !sentence.is_empty() {
                return Some(sentence);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extracts_component_entities() {
        let extractor = HeuristicExtractor;
        let input = ExtractionInput {
            summary_text:
                "Used Grafeo for graph storage and redb for persistence".into(),
            decisions_json: b"[]".to_vec(),
            tool_outcomes_json: b"[]".to_vec(),
            conversation_spans_json: b"[]".to_vec(),
            repo_path: "/test".into(),
        };

        let output = extractor.extract(input).await.unwrap();
        let names: Vec<&str> =
            output.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"grafeo"));
        assert!(names.contains(&"redb"));
    }

    #[tokio::test]
    async fn test_extracts_constraint_entities() {
        let extractor = HeuristicExtractor;
        let input = ExtractionInput {
            summary_text:
                "The system must not download models during hook execution."
                    .into(),
            decisions_json: b"[]".to_vec(),
            tool_outcomes_json: b"[]".to_vec(),
            conversation_spans_json: b"[]".to_vec(),
            repo_path: "/test".into(),
        };

        let output = extractor.extract(input).await.unwrap();
        assert!(output.entities.iter().any(|e| e.kind == "constraint"));
    }

    #[tokio::test]
    async fn test_empty_text_falls_back_to_repo() {
        let extractor = HeuristicExtractor;
        let input = ExtractionInput {
            summary_text: String::new(),
            decisions_json: b"[]".to_vec(),
            tool_outcomes_json: b"[]".to_vec(),
            conversation_spans_json: b"[]".to_vec(),
            repo_path: "/home/user/project".into(),
        };

        let output = extractor.extract(input).await.unwrap();
        assert_eq!(output.entities.len(), 1);
        assert_eq!(output.entities[0].kind, "repo");
        assert_eq!(output.entities[0].name, "/home/user/project");
    }

    #[tokio::test]
    async fn test_links_decisions_to_entities() {
        let extractor = HeuristicExtractor;
        let decisions = serde_json::json!([
            {"decision_id": "dec-123", "statement": "use redb"}
        ]);
        let input = ExtractionInput {
            summary_text: "Chose redb for storage".into(),
            decisions_json: serde_json::to_vec(&decisions).unwrap(),
            tool_outcomes_json: b"[]".to_vec(),
            conversation_spans_json: b"[]".to_vec(),
            repo_path: "/test".into(),
        };

        let output = extractor.extract(input).await.unwrap();
        assert_eq!(output.decision_refs, vec!["dec-123"]);
        assert!(!output.relations.is_empty());
        assert_eq!(
            output.relations[0].relation_type,
            RelationType::DecisionEntity
        );
    }
}
