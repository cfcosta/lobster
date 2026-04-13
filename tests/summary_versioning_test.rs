//! Tests for summary artifact versioning and checksumming.
//!
//! These tests require `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` to be
//! set. They are skipped when no API key is available.

use lobster::{
    episodes::{
        rig_summarizer::RigSummarizer,
        summarizer::{Summarizer, SummaryInput},
    },
    store::ids::EpisodeId,
};

fn has_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
}

/// Summary artifacts must have a non-empty revision string.
#[tokio::test]
async fn test_summary_has_revision() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let summarizer = RigSummarizer::default();
    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: Some("test task".into()),
        file_reads: vec![],
    };

    let artifact = summarizer.summarize(input).await.unwrap();
    assert!(!artifact.revision.is_empty(), "revision must not be empty");
}

/// Summary artifact persists and round-trips through redb.
#[tokio::test]
async fn test_summary_persistence_roundtrip() {
    if !has_api_key() {
        eprintln!("skipping: no API key");
        return;
    }
    let (database, _dir) = lobster::store::db::open_in_memory().unwrap();
    let summarizer = RigSummarizer::default();
    let episode_id = EpisodeId::derive(b"test-ep");

    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: Some("test task".into()),
        file_reads: vec![],
    };

    let mut artifact = summarizer.summarize(input).await.unwrap();
    artifact.episode_id = episode_id;

    lobster::store::crud::put_summary_artifact(&database, &artifact).unwrap();

    let loaded = lobster::store::crud::get_summary_artifact(
        &database,
        &episode_id.raw(),
    )
    .unwrap();

    assert_eq!(loaded.summary_text, artifact.summary_text);
    assert_eq!(loaded.revision, artifact.revision);
    assert_eq!(loaded.payload_checksum, artifact.payload_checksum);
}

/// Without API keys, summarizer returns `ModelUnavailable`.
#[tokio::test]
async fn test_summarizer_requires_api_key() {
    if has_api_key() {
        eprintln!("skipping: API key is set");
        return;
    }
    let summarizer = RigSummarizer::default();
    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: None,
        file_reads: vec![],
    };
    let result = summarizer.summarize(input).await;
    assert!(result.is_err(), "should fail without API key");
}
