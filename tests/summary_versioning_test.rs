//! Tests for the unified episode analysis (summary + extraction).
//!
//! These tests require `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` to be
//! set. They are skipped when no API key is available.

use lobster::extract::rig_extractor;

fn has_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

/// The unified analyze call must produce a non-empty summary.
#[tokio::test]
async fn test_analyze_produces_summary() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }

    let analysis = rig_extractor::analyze(
        "Repository: /test\nTask: test task\n\nEvents: []",
    )
    .await
    .unwrap();

    assert!(!analysis.summary.is_empty(), "summary must not be empty");
}

/// Summary artifact persists and round-trips through redb.
#[tokio::test]
async fn test_analysis_summary_persistence_roundtrip() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let (database, _dir) = lobster::store::db::open_in_memory().unwrap();

    use lobster::store::ids::EpisodeId;
    let episode_id = EpisodeId::derive(b"test-ep");

    let analysis = rig_extractor::analyze(
        "Repository: /test\nTask: test task\n\nEvents: []",
    )
    .await
    .unwrap();

    let artifact = lobster::store::schema::SummaryArtifact {
        episode_id,
        revision: "rig-v2".into(),
        summary_text: analysis.summary.clone(),
        payload_checksum: [0; 32],
    };

    lobster::store::crud::put_summary_artifact(&database, &artifact).unwrap();

    let loaded = lobster::store::crud::get_summary_artifact(
        &database,
        &episode_id.raw(),
    )
    .unwrap();

    assert_eq!(loaded.summary_text, analysis.summary);
    assert_eq!(loaded.revision, "rig-v2");
}

/// Without API keys, analyze returns an error.
#[tokio::test]
async fn test_analyze_requires_api_key() {
    if has_api_key() {
        eprintln!("skipping: API key is set");
        return;
    }
    let result = rig_extractor::analyze("test").await;
    assert!(result.is_err(), "should fail without API key");
}
