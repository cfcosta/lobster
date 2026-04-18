//! Pre-processing utilities for episode event text.
//!
//! Strips tool call markup and large JSON blobs from raw event text
//! before it is sent to the LLM for analysis.

/// Maximum length of a user prompt included in the extracted digest.
const MAX_PROMPT_CHARS: usize = 500;

/// Maximum length of a tool input snippet (file path, command, etc.)
/// included in the extracted digest.
const MAX_TOOL_INPUT_CHARS: usize = 200;

/// Strip tool call/response markup and large JSON blobs from event
/// text before sending to the summarizer.
///
/// Raw episode events arrive as a single-line JSON array of event
/// objects (often well over 500 chars). We must try to parse that
/// shape *before* falling back to line-based stripping, otherwise
/// the "drop long lines starting with `{` / `[`" heuristic wipes
/// the whole payload and leaves the LLM with no content to
/// summarize — which is how earlier revisions ended up fabricating
/// fixes from thin air.
#[must_use]
pub fn strip_tool_markup(text: &str) -> String {
    let trimmed = text.trim();

    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(events) =
            serde_json::from_str::<Vec<serde_json::Value>>(trimmed)
        {
            let mut extracted = String::new();
            for event in &events {
                append_event_digest(event, &mut extracted);
            }
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }

    // Fallback: treat input as a free-form transcript and strip
    // tool_call / tool_response blocks plus any single lines that
    // look like large JSON blobs.
    let mut result = String::with_capacity(text.len());
    let mut skip_depth: u32 = 0;

    for line in text.lines() {
        let trimmed = line.trim();

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

        if trimmed.len() > 500
            && (trimmed.starts_with('{') || trimmed.starts_with('['))
        {
            result.push_str("[large JSON payload omitted]\n");
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result
}

/// Append a human-readable digest of a single raw event to `out`.
///
/// Extracts only fields a summarizer can faithfully report on
/// (event kind, tool name, user prompt, file path, command). Tool
/// outputs and full JSON payloads are intentionally omitted — they
/// are both noisy and a common source of summarizer confabulation.
fn append_event_digest(event: &serde_json::Value, out: &mut String) {
    use std::fmt::Write;

    let name = event
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    if name.is_empty() {
        return;
    }

    let _ = writeln!(out, "Event: {name}");

    if let Some(tool) =
        event.get("tool_name").and_then(serde_json::Value::as_str)
        && !tool.is_empty()
    {
        let _ = writeln!(out, "  Tool: {tool}");
    }

    let input = event.get("tool_input");

    if let Some(prompt) = input
        .and_then(|v| v.get("prompt"))
        .and_then(serde_json::Value::as_str)
        && !prompt.trim().is_empty()
    {
        let _ =
            writeln!(out, "  Prompt: {}", truncate(prompt, MAX_PROMPT_CHARS));
    }

    // file_path covers Read/Write/Edit; command covers Bash; pattern
    // covers Grep.
    for key in ["file_path", "path", "command", "pattern"] {
        if let Some(value) = input
            .and_then(|v| v.get(key))
            .and_then(serde_json::Value::as_str)
            && !value.trim().is_empty()
        {
            let _ = writeln!(
                out,
                "  {}: {}",
                key,
                truncate(value, MAX_TOOL_INPUT_CHARS)
            );
            break;
        }
    }
}

/// Truncate `s` to at most `max` chars, appending an ellipsis marker
/// when a cut is made. Operates on char boundaries, not bytes.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("… [truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_strip_extracts_user_prompt_text() {
        let input = r#"[{"hook_event_name":"UserPromptSubmit","tool_input":{"prompt":"Please fix the login bug in auth.rs"}}]"#;
        let out = strip_tool_markup(input);
        assert!(
            out.contains("Please fix the login bug in auth.rs"),
            "prompt text must appear in extracted digest: {out:?}"
        );
    }

    #[test]
    fn test_strip_extracts_tool_file_path() {
        let input = r#"[{"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"src/auth.rs"}}]"#;
        let out = strip_tool_markup(input);
        assert!(out.contains("Edit"), "tool name missing: {out:?}");
        assert!(out.contains("src/auth.rs"), "file_path missing: {out:?}");
    }

    #[test]
    fn test_strip_extracts_bash_command() {
        let input = r#"[{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}}]"#;
        let out = strip_tool_markup(input);
        assert!(out.contains("Bash"));
        assert!(out.contains("cargo test"));
    }

    #[test]
    fn test_strip_truncates_long_prompt() {
        let long = "x".repeat(5_000);
        let input = format!(
            "[{{\"hook_event_name\":\"UserPromptSubmit\",\"tool_input\":{{\"prompt\":\"{long}\"}}}}]"
        );
        let out = strip_tool_markup(&input);
        assert!(
            out.contains("truncated"),
            "long prompt must be truncated: {out:?}"
        );
        assert!(out.len() < 2_000, "digest too large: {} chars", out.len());
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
