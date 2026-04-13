//! rig-core backed LLM summarizer.
//!
//! Uses `app::llm::call` which reads provider and model from env:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: claude-sonnet-4-6)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: gpt-5.4-mini)

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
        let events_text = strip_tool_markup(&String::from_utf8_lossy(
            &input.episode_events_json,
        ));

        let mut file_context = String::new();
        if !input.file_reads.is_empty() {
            file_context.push_str("\nFiles read during this session:\n");
            for (path, content) in &input.file_reads {
                use std::fmt::Write;
                let _ = writeln!(file_context, "\n--- {path} ---");
                let _ = writeln!(file_context, "{content}");
            }
        }

        let prompt = format!(
            "Repository: {repo}\n\
             Task: {task}\n\
             \n\
             Events from this work session:\n\
             {events}\n\
             {files}",
            repo = input.repo_path,
            task = input.task_title.as_deref().unwrap_or("(none)"),
            events = events_text,
            files = file_context,
        );

        let response = crate::app::llm::call(
            "You produce concise third-person summaries of developer work sessions.\n\
             \n\
             Rules:\n\
             - Write in third person past tense (\"The developer added...\", not \"I will...\").\n\
             - Focus on: what changed, why, what was decided, what files were touched.\n\
             - Omit tool call syntax, JSON payloads, and raw command output.\n\
             - If the session contains no meaningful work, write \"No significant changes.\"\n\
             - Keep the summary under 300 words.\n\
             - Do NOT use markdown headers, bullet points, or formatting.\n\
             - Write plain prose paragraphs only.",
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

/// Strip tool call/response markup and large JSON blobs from event
/// text before sending to the summarizer. This prevents the LLM from
/// parroting raw XML and command output back in the summary.
fn strip_tool_markup(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut skip_depth: u32 = 0;

    for line in text.lines() {
        let trimmed = line.trim();

        // Track tool_call / tool_response blocks
        if trimmed.starts_with("<tool_call>") {
            skip_depth += 1;
            continue;
        }
        if trimmed.starts_with("<tool_response>") {
            skip_depth += 1;
            continue;
        }
        if trimmed.starts_with("</tool_call>")
            || trimmed.starts_with("</tool_response>")
        {
            skip_depth = skip_depth.saturating_sub(1);
            continue;
        }

        if skip_depth > 0 {
            continue;
        }

        // Skip lines that look like large JSON blobs
        if trimmed.len() > 500
            && (trimmed.starts_with('{') || trimmed.starts_with('['))
        {
            result.push_str("[large JSON payload omitted]\n");
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // If the entire input was a JSON array/object (common for raw
    // events), extract just the meaningful fields
    if result.trim().starts_with('[') || result.trim().starts_with('{') {
        if let Ok(events) =
            serde_json::from_str::<Vec<serde_json::Value>>(result.trim())
        {
            use std::fmt::Write;
            let mut extracted = String::new();
            for event in &events {
                if let Some(name) = event.get("hook_event_name") {
                    let _ = writeln!(extracted, "Event: {name}");
                }
                if let Some(tool) = event.get("tool_name") {
                    let _ = writeln!(extracted, "  Tool: {tool}");
                }
            }
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rig_summarizer_requires_api_key() {
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let summarizer = RigSummarizer::default();
            let input = SummaryInput {
                episode_events_json: b"[]".to_vec(),
                repo_path: "/test".into(),
                task_title: None,
                file_reads: vec![],
            };
            let result = summarizer.summarize(input).await;
            assert!(matches!(result, Err(SummaryError::ModelUnavailable(_))));
        }
    }

    // ── strip_tool_markup ───────────────────────────────────

    #[test]
    fn test_strip_removes_tool_call_blocks() {
        let input =
            "Before\n<tool_call>\n{\"name\":\"bash\"}\n</tool_call>\nAfter";
        let out = strip_tool_markup(input);
        assert!(!out.contains("tool_call"));
        assert!(!out.contains("bash"));
        assert!(out.contains("Before"));
        assert!(out.contains("After"));
    }

    #[test]
    fn test_strip_removes_tool_response_blocks() {
        let input =
            "Start\n<tool_response>\nlots of output\n</tool_response>\nEnd";
        let out = strip_tool_markup(input);
        assert!(!out.contains("tool_response"));
        assert!(!out.contains("lots of output"));
        assert!(out.contains("Start"));
        assert!(out.contains("End"));
    }

    #[test]
    fn test_strip_plain_text_unchanged() {
        let input = "The developer fixed a bug in main.rs.\nTests pass.";
        let out = strip_tool_markup(input);
        assert_eq!(out.trim(), input);
    }

    #[test]
    fn test_strip_extracts_event_names_from_json() {
        let input = r#"[{"hook_event_name":"UserPromptSubmit","tool_name":null},{"hook_event_name":"PostToolUse","tool_name":"Write"}]"#;
        let out = strip_tool_markup(input);
        assert!(out.contains("UserPromptSubmit"));
        assert!(out.contains("Write"));
        assert!(!out.contains("hook_event_name"));
    }

    use hegel::{TestCase, generators as gs};

    /// `strip_tool_markup` never produces output containing
    /// `<tool_call>` or `<tool_response>` tags.
    #[hegel::test(test_cases = 200)]
    fn prop_strip_removes_all_tags(tc: TestCase) {
        let prefix: String = tc.draw(gs::text().max_size(50));
        let inner: String = tc.draw(gs::text().max_size(100));
        let suffix: String = tc.draw(gs::text().max_size(50));
        let input =
            format!("{prefix}\n<tool_call>\n{inner}\n</tool_call>\n{suffix}");
        let out = strip_tool_markup(&input);
        assert!(
            !out.contains("<tool_call>"),
            "output still contains <tool_call>"
        );
        assert!(
            !out.contains("</tool_call>"),
            "output still contains </tool_call>"
        );
    }

    /// `strip_tool_markup` on text without tags is idempotent.
    #[hegel::test(test_cases = 200)]
    fn prop_strip_idempotent_on_plain_text(tc: TestCase) {
        let text: String = tc.draw(
            gs::text()
                .max_size(200)
                .alphabet("abcdefghijklmnopqrstuvwxyz \n.!?"),
        );
        let once = strip_tool_markup(&text);
        let twice = strip_tool_markup(&once);
        assert_eq!(once, twice);
    }
}
