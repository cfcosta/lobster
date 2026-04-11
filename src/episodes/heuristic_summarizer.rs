//! Heuristic fallback summarizer for offline use.
//!
//! Produces summaries without LLM access by extracting key
//! information from episode events using deterministic rules.

use sha2::{Digest, Sha256};

use crate::{
    episodes::summarizer::{Summarizer, SummaryError, SummaryInput},
    store::{ids::EpisodeId, schema::SummaryArtifact},
};

/// Deterministic heuristic summarizer.
///
/// Extracts key events and tool outcomes into a structured summary.
/// Works offline with no LLM dependency.
pub struct HeuristicSummarizer {
    pub revision: String,
}

impl Default for HeuristicSummarizer {
    fn default() -> Self {
        Self {
            revision: "heuristic-v1".to_string(),
        }
    }
}

impl Summarizer for HeuristicSummarizer {
    async fn summarize(
        &self,
        input: SummaryInput,
    ) -> Result<SummaryArtifact, SummaryError> {
        let events: Vec<serde_json::Value> =
            serde_json::from_slice(&input.episode_events_json)
                .map_err(|e| SummaryError::InvalidOutput(e.to_string()))?;

        let mut summary_parts = Vec::new();

        if let Some(title) = &input.task_title {
            summary_parts.push(format!("Task: {title}"));
        }

        summary_parts.push(format!("Repo: {}", input.repo_path));
        summary_parts.push(format!("Events: {} total", events.len()));

        // Extract tool names and outcomes
        let tool_events: Vec<&str> = events
            .iter()
            .filter_map(|e| {
                e.get("event_kind").and_then(serde_json::Value::as_str)
            })
            .collect();

        if !tool_events.is_empty() {
            let tool_count = tool_events
                .iter()
                .filter(|&&k| k == "ToolUse" || k == "ToolResult")
                .count();
            if tool_count > 0 {
                summary_parts.push(format!("Tool interactions: {tool_count}"));
            }
        }

        let summary_text = summary_parts.join(". ");

        let mut hasher = Sha256::new();
        hasher.update(summary_text.as_bytes());
        let checksum: [u8; 32] = hasher.finalize().into();

        let episode_id = EpisodeId::derive(input.repo_path.as_bytes());

        Ok(SummaryArtifact {
            episode_id,
            revision: self.revision.clone(),
            summary_text,
            payload_checksum: checksum,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_heuristic_summarizer_basic() {
        let summarizer = HeuristicSummarizer::default();
        let input = SummaryInput {
            episode_events_json: b"[]".to_vec(),
            repo_path: "/home/user/project".into(),
            task_title: Some("Fix the bug".into()),
        };

        let result = summarizer.summarize(input).await;
        assert!(result.is_ok());
        let artifact = result.unwrap();
        assert!(artifact.summary_text.contains("Fix the bug"));
        assert!(artifact.summary_text.contains("0 total"));
        assert_eq!(artifact.revision, "heuristic-v1");
    }

    #[tokio::test]
    async fn test_heuristic_summarizer_with_events() {
        let summarizer = HeuristicSummarizer::default();
        let events = serde_json::json!([
            {"event_kind": "UserPromptSubmit"},
            {"event_kind": "ToolUse"},
            {"event_kind": "ToolResult"},
        ]);

        let input = SummaryInput {
            episode_events_json: serde_json::to_vec(&events).unwrap(),
            repo_path: "/test".into(),
            task_title: None,
        };

        let result = summarizer.summarize(input).await;
        assert!(result.is_ok());
        let artifact = result.unwrap();
        assert!(artifact.summary_text.contains("3 total"));
        assert!(artifact.summary_text.contains("Tool interactions: 2"));
    }

    #[tokio::test]
    async fn test_heuristic_summarizer_deterministic() {
        let summarizer = HeuristicSummarizer::default();
        let input = SummaryInput {
            episode_events_json: b"[]".to_vec(),
            repo_path: "/test".into(),
            task_title: None,
        };

        let r1 = summarizer.summarize(input.clone()).await.unwrap();
        let r2 = summarizer.summarize(input).await.unwrap();
        assert_eq!(r1.summary_text, r2.summary_text);
        assert_eq!(r1.payload_checksum, r2.payload_checksum);
    }

    #[tokio::test]
    async fn test_heuristic_summarizer_invalid_json() {
        let summarizer = HeuristicSummarizer::default();
        let input = SummaryInput {
            episode_events_json: b"not json".to_vec(),
            repo_path: "/test".into(),
            task_title: None,
        };

        let result = summarizer.summarize(input).await;
        assert!(matches!(result, Err(SummaryError::InvalidOutput(_))));
    }
}
