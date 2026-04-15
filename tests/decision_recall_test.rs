//! End-to-end test: decisions recalled through the retrieval pipeline.

use lobster::{
    graph::db as grafeo_db,
    hooks::{events::HookEvent, recall},
    store::db,
};

fn make_prompt_event(prompt: &str) -> HookEvent {
    serde_json::from_value(serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "tool_input": {"prompt": prompt},
        "cwd": "/test/repo",
    }))
    .unwrap()
}

#[test]
fn test_recall_pipeline_runs_on_empty_db() {
    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    let event = make_prompt_event("What did we decide about storage?");

    let payload = recall::run_recall(&event, &database, &grafeo);
    assert!(payload.items.is_empty());
    assert!(payload.truncated.is_none());
    assert!(payload.latency_ms < 1000);
}

#[test]
fn test_recall_query_construction() {
    let event = make_prompt_event("Why did we use redb?");
    let query = recall::construct_query(&event);
    assert_eq!(query.as_deref(), Some("Why did we use redb?"));
}
