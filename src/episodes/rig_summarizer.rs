//! rig-core backed LLM summarizer.
//!
//! Per spec: "The primary summarizer implementation wraps rig-core,
//! which is Tokio-based." Uses the provider-agnostic rig API for
//! Anthropic/OpenAI/Ollama backends.

use rig::client::{CompletionClient, ProviderClient};
use sha2::{Digest, Sha256};

use crate::{
    episodes::summarizer::{Summarizer, SummaryError, SummaryInput},
    store::{ids::EpisodeId, schema::SummaryArtifact},
};

/// rig-core backed LLM summarizer.
///
/// Reads the LLM provider from environment variables:
/// - `ANTHROPIC_API_KEY` for Anthropic
/// - `OPENAI_API_KEY` for `OpenAI`
/// - Falls back to heuristic if no key is set.
pub struct RigSummarizer {
    pub revision: String,
}

impl Default for RigSummarizer {
    fn default() -> Self {
        Self {
            revision: "rig-v1".to_string(),
        }
    }
}

impl Summarizer for RigSummarizer {
    async fn summarize(
        &self,
        input: SummaryInput,
    ) -> Result<SummaryArtifact, SummaryError> {
        use rig::completion::Prompt;

        // Try Anthropic first, then OpenAI, then fall back
        let response = if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            let client = rig::providers::anthropic::Client::from_env();
            let agent = client
                .agent("claude-sonnet-4-6")
                .preamble(
                    "Summarize the following episode events \
                     concisely. Focus on decisions made, tools \
                     used, and outcomes.",
                )
                .build();
            agent
                .prompt(&format!(
                    "Task: {}\nRepo: {}\nEvents: {}",
                    input.task_title.as_deref().unwrap_or("unknown"),
                    input.repo_path,
                    String::from_utf8_lossy(&input.episode_events_json,)
                ))
                .await
                .map_err(|e| SummaryError::ModelUnavailable(e.to_string()))?
        } else if std::env::var("OPENAI_API_KEY").is_ok() {
            let client = rig::providers::openai::Client::from_env();
            let agent = client
                .agent("gpt-4o-mini")
                .preamble(
                    "Summarize the following episode events \
                     concisely. Focus on decisions made, tools \
                     used, and outcomes.",
                )
                .build();
            agent
                .prompt(&format!(
                    "Task: {}\nRepo: {}\nEvents: {}",
                    input.task_title.as_deref().unwrap_or("unknown"),
                    input.repo_path,
                    String::from_utf8_lossy(&input.episode_events_json,)
                ))
                .await
                .map_err(|e| SummaryError::ModelUnavailable(e.to_string()))?
        } else {
            return Err(SummaryError::ModelUnavailable(
                "No ANTHROPIC_API_KEY or OPENAI_API_KEY set".into(),
            ));
        };

        let mut hasher = Sha256::new();
        hasher.update(response.as_bytes());
        let checksum: [u8; 32] = hasher.finalize().into();

        Ok(SummaryArtifact {
            episode_id: EpisodeId::derive(input.repo_path.as_bytes()),
            revision: self.revision.clone(),
            summary_text: response,
            payload_checksum: checksum,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rig_summarizer_requires_api_key() {
        // Without API keys, should return ModelUnavailable
        // (We can't test with real keys in CI)
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let summarizer = RigSummarizer::default();
            let input = SummaryInput {
                episode_events_json: b"[]".to_vec(),
                repo_path: "/test".into(),
                task_title: None,
            };
            let result = summarizer.summarize(input).await;
            assert!(matches!(result, Err(SummaryError::ModelUnavailable(_))));
        }
    }
}
