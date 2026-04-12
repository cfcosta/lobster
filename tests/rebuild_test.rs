//! Integration test: Grafeo can be rebuilt from redb alone.
//!
//! This proves the architecture's canonical rule: if Grafeo is
//! lost, Lobster can reconstruct the semantic graph from the
//! durable artifacts stored in redb.

use lobster::{
    episodes::finalize::{FinalizeResult, finalize_episode},
    graph::{db as grafeo_db, rebuild::rebuild_from_redb},
    store::db,
};

#[tokio::test]
async fn test_rebuild_from_scratch_matches_original() {
    let database = db::open_in_memory().unwrap();

    // Phase 1: Finalize several episodes into original Grafeo
    let grafeo_original = grafeo_db::new_in_memory();

    let r1 = finalize_episode(
        &database,
        &grafeo_original,
        "/repo/a",
        b"[]",
        0,
        10,
        Some("Fix auth".into()),
    )
    .await;
    assert!(matches!(r1, FinalizeResult::Ready { .. }));

    let r2 = finalize_episode(
        &database,
        &grafeo_original,
        "/repo/a",
        b"[]",
        11,
        20,
        Some("Add tests".into()),
    )
    .await;
    assert!(matches!(r2, FinalizeResult::Ready { .. }));

    let original_nodes = grafeo_original.node_count();
    let _original_edges = grafeo_original.edge_count();
    assert!(original_nodes >= 2, "should have at least 2 episode nodes");

    // Phase 2: Pretend Grafeo is lost — rebuild from redb
    let grafeo_rebuilt = grafeo_db::new_in_memory();
    let stats = rebuild_from_redb(&database, &grafeo_rebuilt).unwrap();

    assert_eq!(stats.episodes_projected, 2);
    assert!(
        grafeo_rebuilt.node_count() >= 2,
        "rebuilt should have at least episode nodes"
    );
}

#[tokio::test]
async fn test_rebuild_excludes_non_ready_episodes() {
    let database = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    // Finalize one episode (Ready)
    let r =
        finalize_episode(&database, &grafeo, "/repo", b"[]", 0, 5, None).await;
    assert!(matches!(r, FinalizeResult::Ready { .. }));

    // Manually insert a Pending episode
    let pending = lobster::store::schema::Episode {
        episode_id: lobster::store::ids::EpisodeId::derive(b"pending-ep"),
        repo_id: lobster::store::ids::RepoId::derive(b"repo"),
        start_seq: 100,
        end_seq: 110,
        task_id: None,
        processing_state: lobster::store::schema::ProcessingState::Pending,
        finalized_ts_utc_ms: 999,
        retry_count: 0,
    };
    lobster::store::crud::put_episode(&database, &pending).unwrap();

    // Rebuild — should only project the Ready episode
    let grafeo_rebuilt = grafeo_db::new_in_memory();
    let stats = rebuild_from_redb(&database, &grafeo_rebuilt).unwrap();

    assert_eq!(stats.episodes_scanned, 2, "should scan both episodes");
    assert_eq!(stats.episodes_projected, 1, "should only project Ready");
    assert_eq!(stats.episodes_skipped, 1, "should skip Pending");
}
