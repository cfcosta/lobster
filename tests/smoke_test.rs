//! Full pipeline integration smoke test.

use lobster::{
    episodes::finalize::{FinalizeResult, finalize_episode},
    graph::{db as grafeo_db, rebuild::rebuild_from_redb},
    hooks::{events::HookEvent, recall},
    rank::routes::execute_query,
    store::{crud, db, schema::ProcessingState, visibility},
};

fn has_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

fn make_prompt_event(prompt: &str) -> HookEvent {
    serde_json::from_value(serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "tool_input": {"prompt": prompt},
        "cwd": "/home/user/project",
    }))
    .unwrap()
}

#[tokio::test]
async fn test_full_pipeline_smoke() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    let result = finalize_episode(
        &database,
        &grafeo,
        "/home/user/project",
        b"[]",
        0,
        10,
        Some("Build memory search".into()),
    )
    .await;

    let episode_id = match result {
        FinalizeResult::Ready { episode_id, .. } => episode_id,
        other => panic!("finalization failed: {other:?}"),
    };

    let episode = crud::get_episode(&database, &episode_id.raw()).unwrap();
    assert_eq!(episode.processing_state, ProcessingState::Ready);
    assert!(grafeo.node_count() >= 1);
    assert!(visibility::is_episode_visible(&database, &episode_id.raw()));

    let results = execute_query("memory search", &database, &grafeo, false);
    let _ = results;

    let hook_event = make_prompt_event("What was the task?");
    let payload = recall::run_recall(&hook_event, &database, &grafeo);
    assert!(payload.latency_ms < 5000);

    let grafeo_fresh = grafeo_db::new_in_memory();
    let rebuild_stats = rebuild_from_redb(&database, &grafeo_fresh).unwrap();
    assert_eq!(rebuild_stats.episodes_projected, 1);
}

#[tokio::test]
async fn test_multiple_episodes_accumulate() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    for i in 0..3 {
        let result = finalize_episode(
            &database,
            &grafeo,
            "/repo",
            b"[]",
            i * 10,
            i * 10 + 9,
            Some(format!("Task {i}")),
        )
        .await;
        assert!(
            matches!(result, FinalizeResult::Ready { .. }),
            "episode {i} should finalize"
        );
    }

    let grafeo_rebuilt = grafeo_db::new_in_memory();
    let stats = rebuild_from_redb(&database, &grafeo_rebuilt).unwrap();
    assert_eq!(stats.episodes_projected, 3);
}
