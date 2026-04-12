//! rig-core backed LLM summarizer.
//!
//! Uses `app::llm::call` which reads provider and model from env:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: claude-sonnet-4-6)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: gpt-4o-mini)

use sha2::{Digest, Sha256};

use crate::{
    episodes::summarizer::{Summarizer, SummaryError, SummaryInput},
    store::{ids::EpisodeId, schema::SummaryArtifact},
};

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
        let prompt = format!(
            "Task: {}\nRepo: {}\nEvents:\n{}",
            input.task_title.as_deref().unwrap_or("unknown"),
            input.repo_path,
            String::from_utf8_lossy(&input.episode_events_json),
        );

        let response = crate::app::llm::call(
            "Summarize the following episode events concisely. \
             Focus on decisions made, tools used, and outcomes.",
            &prompt,
        )
        .await
        .map_err(SummaryError::ModelUnavailable)?;

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
