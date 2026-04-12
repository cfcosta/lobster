//! Hook recall: query construction, retrieval, and output assembly.
//!
//! When a hook fires (e.g., `UserPromptSubmit`), this module:
//! 1. Constructs a recall query from the hook context
//! 2. Runs retrieval via the route execution pipeline
//! 3. Expands evidence windows on results
//! 4. Assembles a tiny output payload (1-3 items)

use std::time::Instant;

use grafeo::GrafeoDB;
use redb::Database;
use serde::Serialize;

use crate::{
    hooks::events::HookEvent,
    rank::{
        evidence::{
            DecisionEvidence,
            SummaryEvidence,
            expand_decision,
            expand_summary,
        },
        routes::RetrievalResult,
    },
};

/// Maximum items in automatic recall output.
const MAX_RECALL_ITEMS: usize = 3;

/// Maximum time budget for hook recall in milliseconds.
const RECALL_BUDGET_MS: u128 = 500;

/// The recall output payload returned to Claude Code.
#[derive(Debug, Clone, Serialize)]
pub struct RecallPayload {
    pub items: Vec<RecallItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    pub latency_ms: u64,
}

/// A single recall item.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum RecallItem {
    Decision(DecisionEvidence),
    Summary(SummaryEvidence),
    Hint { text: String },
}

/// Run the recall pipeline for a hook event.
///
/// Returns a JSON-serializable payload with 0-3 high-confidence
/// recall items. Returns empty payload if no relevant memories
/// found or if the latency budget is exceeded.
#[must_use]
pub fn run_recall(
    event: &HookEvent,
    db: &Database,
    grafeo: &GrafeoDB,
) -> RecallPayload {
    let start = Instant::now();

    // Construct query from hook context
    let Some(query) = construct_query(event) else {
        return RecallPayload {
            items: vec![],
            truncated: None,
            latency_ms: 0,
        };
    };

    // Execute retrieval
    // Find current task for task_overlap scoring
    let repo_id = event
        .working_directory()
        .map(|d| crate::store::ids::RepoId::derive(d.as_bytes()));
    let current_task = repo_id
        .as_ref()
        .and_then(|r| crate::rank::context::find_current_task(db, r));

    let results = crate::rank::routes::execute_query_with_context(
        &query,
        db,
        grafeo,
        false,
        current_task.as_ref(),
    );

    // Check latency budget
    let elapsed = start.elapsed().as_millis();
    if elapsed > RECALL_BUDGET_MS {
        tracing::warn!(
            elapsed_ms = elapsed,
            budget_ms = RECALL_BUDGET_MS,
            "recall exceeded latency budget"
        );
        return RecallPayload {
            items: vec![],
            truncated: Some(true),
            latency_ms: u64::try_from(elapsed).unwrap_or(u64::MAX),
        };
    }

    // Assemble output payload — load actual content from redb
    let items: Vec<RecallItem> = results
        .iter()
        .take(MAX_RECALL_ITEMS)
        .map(|r| result_to_item(r, db))
        .collect();

    let truncated = if results.len() > MAX_RECALL_ITEMS {
        Some(true)
    } else {
        None
    };

    let latency_ms =
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    RecallPayload {
        items,
        truncated,
        latency_ms,
    }
}

/// Construct a recall query from hook context.
///
/// Returns `None` for hook types that don't need recall.
#[must_use]
pub fn construct_query(event: &HookEvent) -> Option<String> {
    if event.is_prompt_submit() {
        event.user_prompt()
    } else if event.is_tool_use() || event.is_tool_failure() {
        let tool = event.tool_name.as_deref().unwrap_or("");
        let input_context = event
            .tool_input
            .as_ref()
            .and_then(|v| {
                v.get("file_path")
                    .or_else(|| v.get("path"))
                    .or_else(|| v.get("command"))
            })
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if tool.is_empty() && input_context.is_empty() {
            return None;
        }
        Some(format!("{tool} {input_context}").trim().to_string())
    } else {
        None
    }
}

fn result_to_item(result: &RetrievalResult, db: &Database) -> RecallItem {
    use crate::store::crud;

    match result.artifact_type.as_str() {
        "decision" => {
            // Load actual decision from redb
            if let Ok(dec) = crud::get_decision(db, &result.episode_id) {
                RecallItem::Decision(expand_decision(
                    &dec.statement,
                    &dec.rationale,
                    &format!("{:?}", dec.confidence),
                    &dec.evidence
                        .iter()
                        .map(|e| {
                            (e.episode_id.to_string(), e.span_summary.clone())
                        })
                        .collect::<Vec<_>>(),
                    None,
                ))
            } else {
                RecallItem::Hint {
                    text: format!("Decision (score: {:.2})", result.score),
                }
            }
        }
        "summary" => {
            // Load actual summary from redb
            if let Ok(summary) =
                crud::get_summary_artifact(db, &result.episode_id)
            {
                RecallItem::Summary(expand_summary(
                    &summary.summary_text,
                    &result.episode_id.to_string(),
                    &[],
                ))
            } else {
                RecallItem::Hint {
                    text: format!("Summary (score: {:.2})", result.score),
                }
            }
        }
        "entity" => {
            if let Ok(entity) = crud::get_entity(db, &result.episode_id) {
                RecallItem::Hint {
                    text: format!(
                        "{:?}: {}",
                        entity.kind, entity.canonical_name
                    ),
                }
            } else {
                RecallItem::Hint {
                    text: format!("Entity (score: {:.2})", result.score),
                }
            }
        }
        _ => RecallItem::Hint {
            text: format!(
                "Related {} (score: {:.2})",
                result.artifact_type, result.score
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{graph::db as grafeo_db, store::db};

    fn make_prompt_event(prompt: &str) -> HookEvent {
        serde_json::from_value(serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "tool_input": {"prompt": prompt},
            "cwd": "/test",
        }))
        .unwrap()
    }

    fn make_tool_event(tool: &str) -> HookEvent {
        serde_json::from_value(serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "cwd": "/test",
        }))
        .unwrap()
    }

    #[test]
    fn test_construct_query_from_prompt() {
        let event = make_prompt_event("fix the auth bug");
        let query = construct_query(&event);
        assert_eq!(query.as_deref(), Some("fix the auth bug"));
    }

    #[test]
    fn test_construct_query_from_tool() {
        let event = make_tool_event("Write");
        let query = construct_query(&event);
        assert_eq!(query.as_deref(), Some("Write"));
    }

    #[test]
    fn test_construct_query_stop_returns_none() {
        let event: HookEvent = serde_json::from_value(serde_json::json!({
            "hook_event_name": "Stop",
            "reason": "done",
        }))
        .unwrap();
        assert!(construct_query(&event).is_none());
    }

    #[test]
    fn test_run_recall_empty_db() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();
        let event = make_prompt_event("test query");

        let payload = run_recall(&event, &database, &grafeo);
        assert!(payload.items.is_empty());
        assert!(payload.latency_ms < 1000);
    }

    #[test]
    fn test_run_recall_stop_noop() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();
        let event: HookEvent = serde_json::from_value(serde_json::json!({
            "hook_event_name": "Stop",
        }))
        .unwrap();

        let payload = run_recall(&event, &database, &grafeo);
        assert!(payload.items.is_empty());
        assert_eq!(payload.latency_ms, 0);
    }

    #[test]
    fn test_recall_payload_serializes_to_json() {
        let payload = RecallPayload {
            items: vec![RecallItem::Hint {
                text: "test hint".into(),
            }],
            truncated: None,
            latency_ms: 42,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("test hint"));
        assert!(json.contains("42"));
        // truncated should not appear when None
        assert!(!json.contains("truncated"));
    }
}
