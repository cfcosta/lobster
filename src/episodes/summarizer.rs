//! Async Summarizer trait interface.
//!
//! The primary summarizer implementation wraps rig-core, which is
//! Tokio-based. Making the trait async from the start avoids
//! `block_on` calls into hook handling and MCP request paths.

use crate::store::schema::SummaryArtifact;

/// Input for summarization: the episode's events and context.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    pub episode_events_json: Vec<u8>,
    pub repo_path: String,
    pub task_title: Option<String>,
}

/// Error from the summarization pipeline.
#[derive(Debug)]
pub enum SummaryError {
    ModelUnavailable(String),
    Timeout,
    InvalidOutput(String),
}

impl std::fmt::Display for SummaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelUnavailable(msg) => {
                write!(f, "model unavailable: {msg}")
            }
            Self::Timeout => write!(f, "summarization timed out"),
            Self::InvalidOutput(msg) => {
                write!(f, "invalid output: {msg}")
            }
        }
    }
}

impl std::error::Error for SummaryError {}

/// Async summarization interface.
///
/// Implementations:
/// - `rig-core`-backed LLM summarizer (Anthropic/OpenAI/Ollama)
/// - Heuristic fallback summarizer (offline, deterministic)
pub trait Summarizer: Send + Sync {
    fn summarize(
        &self,
        input: SummaryInput,
    ) -> impl Future<Output = Result<SummaryArtifact, SummaryError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ids::EpisodeId;

    struct MockSummarizer;

    impl Summarizer for MockSummarizer {
        async fn summarize(
            &self,
            input: SummaryInput,
        ) -> Result<SummaryArtifact, SummaryError> {
            Ok(SummaryArtifact {
                episode_id: EpisodeId::derive(input.repo_path.as_bytes()),
                revision: "mock-v1".into(),
                summary_text: format!(
                    "Summary of {} bytes",
                    input.episode_events_json.len()
                ),
                payload_checksum: [0; 32],
            })
        }
    }

    #[tokio::test]
    async fn test_mock_summarizer() {
        let summarizer = MockSummarizer;
        let input = SummaryInput {
            episode_events_json: b"[]".to_vec(),
            repo_path: "/test/repo".into(),
            task_title: None,
        };
        let result = summarizer.summarize(input).await;
        assert!(result.is_ok());
        let artifact = result.unwrap();
        assert!(artifact.summary_text.contains("2 bytes"));
    }
}
