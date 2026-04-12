//! Full pipeline integration smoke test.
//!
//! Exercises the complete chain: hook event → event capture →
//! episode finalization → Grafeo projection → retrieval query →
//! recall output. Proves the system works end-to-end.

use lobster::{
    episodes::finalize::{FinalizeResult, finalize_episode},
    graph::{db as grafeo_db, rebuild::rebuild_from_redb},
    hooks::{
        events::{HookEvent, HookType},
        recall,
    },
    rank::routes::execute_query,
    store::{crud, db, schema::ProcessingState, visibility},
};

/// Complete smoke test exercising every layer of the system.
fn has_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

#[tokio::test]
async fn test_full_pipeline_smoke() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    // 1. Create storage
    let database = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    // 2. Simulate episode finalization (normally triggered by
    //    event capture + segmentation)
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

    // 3. Verify redb has the episode in Ready state
    let episode = crud::get_episode(&database, &episode_id.raw()).unwrap();
    assert_eq!(episode.processing_state, ProcessingState::Ready);

    // 4. Verify summary artifact was persisted
    let summary =
        crud::get_summary_artifact(&database, &episode_id.raw()).unwrap();
    assert!(!summary.summary_text.is_empty());

    // 5. Verify extraction artifact was persisted with real checksum
    let extraction =
        crud::get_extraction_artifact(&database, &episode_id.raw()).unwrap();
    assert_ne!(extraction.payload_checksum, [0; 32]);

    // 6. Verify Grafeo has nodes
    assert!(grafeo.node_count() >= 1);

    // 7. Verify visibility: Ready episode is visible
    assert!(visibility::is_episode_visible(&database, &episode_id.raw()));

    // 8. Run a retrieval query
    let results = execute_query("memory search", &database, &grafeo, false);
    // Results may be empty (no Grafeo text/vector indexes yet)
    // but the pipeline should not crash
    let _ = results;

    // 9. Simulate a hook recall
    let hook_event = HookEvent {
        hook_type: HookType::UserPromptSubmit,
        session_id: "smoke-test".into(),
        tool_name: None,
        tool_input: None,
        tool_output: None,
        user_prompt: Some("What was the task?".into()),
        assistant_response: None,
        working_directory: Some("/home/user/project".into()),
        timestamp_ms: 1_700_000_000_000,
    };

    let payload = recall::run_recall(&hook_event, &database, &grafeo);
    // Should not crash, should have low latency
    assert!(payload.latency_ms < 5000);

    // 10. Verify Grafeo can be rebuilt from redb
    let grafeo_fresh = grafeo_db::new_in_memory();
    let rebuild_stats = rebuild_from_redb(&database, &grafeo_fresh).unwrap();
    assert_eq!(rebuild_stats.episodes_projected, 1);
    assert!(grafeo_fresh.node_count() >= 1);
}

/// Smoke test: multiple episodes accumulate correctly.
#[tokio::test]
async fn test_multiple_episodes_accumulate() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let database = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    for i in 0..5 {
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

    // All 5 episodes should be Ready
    let grafeo_rebuilt = grafeo_db::new_in_memory();
    let stats = rebuild_from_redb(&database, &grafeo_rebuilt).unwrap();
    assert_eq!(stats.episodes_projected, 5);
}
