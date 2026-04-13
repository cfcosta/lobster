//! rig-core backed LLM extractor.
//!
//! Uses `app::llm::call` which reads provider and model from env:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: claude-sonnet-4-6)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: gpt-4o-mini)

use crate::extract::traits::{
    ExtractedDecision,
    ExtractedEntity,
    ExtractedRelation,
    ExtractionError,
    ExtractionInput,
    ExtractionOutput,
    Extractor,
    RelationType,
};

pub struct RigExtractor;

impl Extractor for RigExtractor {
    async fn extract(
        &self,
        input: ExtractionInput,
    ) -> Result<ExtractionOutput, ExtractionError> {
        let prompt = format!(
            "Repository: {repo}\n\
             \n\
             Summary of work session:\n\
             {summary}\n",
            repo = input.repo_path,
            summary = input.summary_text,
        );

        let response = crate::app::llm::call(
            "You extract structured facts from developer work session summaries.\n\
             Output a single JSON object with these fields:\n\
             \n\
             \"decisions\": array of decisions made during the session. Each has:\n\
             - \"statement\": one clear sentence stating what was decided\n\
             - \"rationale\": why it was decided (one sentence)\n\
             - \"confidence\": \"high\" (explicit choice), \"medium\" (implied), or \"low\" (uncertain)\n\
             Only include genuine technical decisions (architecture, design, tool choice, approach).\n\
             Do NOT include routine actions like \"ran tests\" or \"read a file\".\n\
             \n\
             \"entities\": array of notable things mentioned. Each has:\n\
             - \"kind\": one of \"component\", \"constraint\", \"file-lite\", \"concept\", \"repo\"\n\
             - \"name\": canonical name\n\
             Only include entities that would be useful to recall in future sessions.\n\
             Prefer components and constraints over generic concepts.\n\
             \n\
             \"relations\": array of relationships. Each has:\n\
             - \"relation_type\": \"EntityEntity\", \"DecisionEntity\", \"TaskEntity\", or \"TaskDecision\"\n\
             - \"from\": name of source entity\n\
             - \"to\": name of target entity\n\
             \n\
             \"task_refs\": array of task title strings (empty if none)\n\
             \"decision_refs\": array of decision statement strings (empty if none)\n\
             \n\
             Rules:\n\
             - Output ONLY valid JSON, no markdown fences or prose.\n\
             - Only extract what is explicitly stated. Do not invent.\n\
             - If nothing meaningful was decided, return an empty \"decisions\" array.\n\
             - Prefer fewer, higher-quality items over exhaustive extraction.",
            &prompt,
        )
        .await
        .map_err(ExtractionError::ModelUnavailable)?;

        parse_extraction_response(&response)
    }
}

#[allow(clippy::too_many_lines)]
fn parse_extraction_response(
    response: &str,
) -> Result<ExtractionOutput, ExtractionError> {
    // Try to find JSON in the response (LLMs sometimes add prose)
    let json_str = extract_json_from_response(response);

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| {
            ExtractionError::InvalidOutput(format!(
                "failed to parse LLM JSON: {e}\nresponse: {response}"
            ))
        })?;

    let entities: Vec<ExtractedEntity> = parsed
        .get("entities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some(ExtractedEntity {
                        kind: e.get("kind")?.as_str()?.to_string(),
                        name: e.get("name")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let relations: Vec<ExtractedRelation> = parsed
        .get("relations")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let rt_str = r.get("relation_type")?.as_str()?;
                    let relation_type = match rt_str {
                        "TaskDecision" => RelationType::TaskDecision,
                        "TaskEntity" => RelationType::TaskEntity,
                        "DecisionEntity" => RelationType::DecisionEntity,
                        "EntityEntity" => RelationType::EntityEntity,
                        _ => return None,
                    };
                    Some(ExtractedRelation {
                        relation_type,
                        from: r.get("from")?.as_str()?.to_string(),
                        to: r.get("to")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let task_refs: Vec<String> = parsed
        .get("task_refs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let decision_refs: Vec<String> = parsed
        .get("decision_refs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let decisions: Vec<ExtractedDecision> = parsed
        .get("decisions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    Some(ExtractedDecision {
                        statement: d.get("statement")?.as_str()?.to_string(),
                        rationale: d.get("rationale")?.as_str()?.to_string(),
                        confidence: d
                            .get("confidence")
                            .and_then(|v| v.as_str())
                            .unwrap_or("low")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ExtractionOutput {
        task_refs,
        decision_refs,
        entities,
        relations,
        decisions,
    })
}

fn extract_json_from_response(response: &str) -> &str {
    // Try to find JSON object in the response
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            return &response[start..=end];
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_extraction() {
        let json = r#"{
            "entities": [
                {"kind": "component", "name": "redb"},
                {"kind": "constraint", "name": "offline-first"}
            ],
            "relations": [
                {"relation_type": "DecisionEntity", "from": "dec-1", "to": "redb"}
            ],
            "task_refs": [],
            "decision_refs": ["dec-1"]
        }"#;

        let result = parse_extraction_response(json).unwrap();
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.relations.len(), 1);
        assert_eq!(result.decision_refs, vec!["dec-1"]);
    }

    #[test]
    fn test_parse_empty_extraction_is_valid() {
        let json = r#"{
            "entities": [],
            "relations": [],
            "task_refs": [],
            "decision_refs": []
        }"#;

        let result = parse_extraction_response(json).unwrap();
        // Empty extraction is valid — no fake entities injected
        assert!(result.entities.is_empty());
    }

    #[test]
    fn test_parse_with_markdown_fences() {
        let response = "Here's the extraction:\n```json\n{\"entities\": [{\"kind\": \"component\", \"name\": \"grafeo\"}], \"relations\": [], \"task_refs\": [], \"decision_refs\": []}\n```";

        let result = parse_extraction_response(response).unwrap();
        assert_eq!(result.entities[0].name, "grafeo");
    }

    #[tokio::test]
    async fn test_rig_extractor_requires_api_key() {
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let extractor = RigExtractor;
            let input = ExtractionInput {
                summary_text: "test".into(),
                decisions_json: b"[]".to_vec(),
                tool_outcomes_json: b"[]".to_vec(),
                conversation_spans_json: b"[]".to_vec(),
                repo_path: "/test".into(),
            };
            let result = extractor.extract(input).await;
            assert!(matches!(
                result,
                Err(ExtractionError::ModelUnavailable(_))
            ));
        }
    }

    // ── Decision extraction parsing ─────────────────────────

    #[test]
    fn test_parse_with_decisions() {
        let json = r#"{
            "entities": [{"kind": "component", "name": "redb"}],
            "relations": [],
            "task_refs": [],
            "decision_refs": [],
            "decisions": [
                {
                    "statement": "Use redb for storage",
                    "rationale": "Embedded, ACID, Rust-native",
                    "confidence": "high"
                }
            ]
        }"#;

        let result = parse_extraction_response(json).unwrap();
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0].statement, "Use redb for storage");
        assert_eq!(result.decisions[0].confidence, "high");
    }

    #[test]
    fn test_parse_without_decisions_defaults_empty() {
        let json = r#"{
            "entities": [{"kind": "component", "name": "grafeo"}],
            "relations": [],
            "task_refs": [],
            "decision_refs": []
        }"#;

        let result = parse_extraction_response(json).unwrap();
        assert!(result.decisions.is_empty());
    }

    #[test]
    fn test_parse_decisions_missing_fields_skipped() {
        let json = r#"{
            "entities": [],
            "relations": [],
            "task_refs": [],
            "decision_refs": [],
            "decisions": [
                {"statement": "valid", "rationale": "reason", "confidence": "high"},
                {"statement": "no rationale"},
                {"rationale": "no statement"}
            ]
        }"#;

        let result = parse_extraction_response(json).unwrap();
        // Only the first decision has both required fields
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0].statement, "valid");
    }

    use hegel::{TestCase, generators as gs};

    /// `ExtractedDecision` serde round-trip.
    #[hegel::test(test_cases = 200)]
    fn prop_extracted_decision_roundtrip(tc: TestCase) {
        let dec = ExtractedDecision {
            statement: tc.draw(gs::text().min_size(1).max_size(100)),
            rationale: tc.draw(gs::text().min_size(1).max_size(100)),
            confidence: tc.draw(gs::sampled_from(vec![
                "high".to_string(),
                "medium".to_string(),
                "low".to_string(),
            ])),
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: ExtractedDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(dec, parsed);
    }

    /// `parse_extraction_response` with decisions round-trips
    /// the decision count.
    #[hegel::test(test_cases = 100)]
    fn prop_parse_preserves_decision_count(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut decisions = Vec::new();
        for _ in 0..n {
            let stmt: String = tc.draw(
                gs::text()
                    .min_size(1)
                    .max_size(50)
                    .alphabet("abcdefghijklmnopqrstuvwxyz "),
            );
            let rationale: String = tc.draw(
                gs::text()
                    .min_size(1)
                    .max_size(50)
                    .alphabet("abcdefghijklmnopqrstuvwxyz "),
            );
            let conf: String = tc.draw(gs::sampled_from(vec![
                "high".to_string(),
                "medium".to_string(),
                "low".to_string(),
            ]));
            decisions.push(serde_json::json!({
                "statement": stmt,
                "rationale": rationale,
                "confidence": conf,
            }));
        }

        let json = serde_json::json!({
            "entities": [{"kind": "repo", "name": "test"}],
            "relations": [],
            "task_refs": [],
            "decision_refs": [],
            "decisions": decisions,
        });

        let result = parse_extraction_response(&json.to_string()).unwrap();
        assert_eq!(result.decisions.len(), n);
    }
}
