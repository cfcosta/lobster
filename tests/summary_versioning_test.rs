//! Tests for summary artifact versioning and checksumming.
//!
//! Verifies that summary artifacts have proper version tracking
//! and deterministic checksums as required by the architecture.

use lobster::{
    episodes::{
        heuristic_summarizer::HeuristicSummarizer,
        summarizer::{Summarizer, SummaryInput},
    },
    store::ids::EpisodeId,
};

/// Summary artifacts must have a non-empty revision string.
#[tokio::test]
async fn test_summary_has_revision() {
    let summarizer = HeuristicSummarizer::default();
    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: None,
    };

    let artifact = summarizer.summarize(input).await.unwrap();
    assert!(!artifact.revision.is_empty(), "revision must not be empty");
    assert!(
        artifact.revision.contains("heuristic"),
        "heuristic summarizer should identify itself"
    );
}

/// Summary checksums must be deterministic: same input → same
/// checksum.
#[tokio::test]
async fn test_summary_checksum_deterministic() {
    let summarizer = HeuristicSummarizer::default();
    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: Some("Build memory".into()),
    };

    let a1 = summarizer.summarize(input.clone()).await.unwrap();
    let a2 = summarizer.summarize(input).await.unwrap();

    assert_eq!(
        a1.payload_checksum, a2.payload_checksum,
        "same input must produce same checksum"
    );
    assert_ne!(a1.payload_checksum, [0; 32], "checksum must not be zeros");
}

/// Different inputs must produce different checksums.
#[tokio::test]
async fn test_summary_checksum_varies_with_input() {
    let summarizer = HeuristicSummarizer::default();

    let a1 = summarizer
        .summarize(SummaryInput {
            episode_events_json: b"[]".to_vec(),
            repo_path: "/repo-a".into(),
            task_title: None,
        })
        .await
        .unwrap();

    let a2 = summarizer
        .summarize(SummaryInput {
            episode_events_json: b"[]".to_vec(),
            repo_path: "/repo-b".into(),
            task_title: None,
        })
        .await
        .unwrap();

    assert_ne!(
        a1.payload_checksum, a2.payload_checksum,
        "different inputs should produce different checksums"
    );
}

/// Summary artifact persists and round-trips through redb.
#[tokio::test]
async fn test_summary_persistence_roundtrip() {
    let database = lobster::store::db::open_in_memory().unwrap();
    let summarizer = HeuristicSummarizer::default();
    let episode_id = EpisodeId::derive(b"test-ep");

    let input = SummaryInput {
        episode_events_json: b"[]".to_vec(),
        repo_path: "/test".into(),
        task_title: None,
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
