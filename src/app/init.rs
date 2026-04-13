//! Initialization logic for `lobster init`.
//!
//! Handles home-directory guard and config merging so that
//! existing `.claude/settings.json` and `.mcp.json` are never
//! clobbered.

use std::path::Path;

/// Check whether a repo root is the user's home directory.
///
/// Lobster must never write to `~/.claude/settings.json` (the
/// global Claude config).
///
/// # Errors
///
/// Returns an error message if `repo_root` resolves to `$HOME`.
pub fn reject_home_directory(repo_root: &Path) -> Result<(), String> {
    let Some(home) = home_dir() else {
        return Ok(()); // can't determine home, allow
    };

    // Canonicalize both to handle symlinks / trailing slashes
    let canon_repo = std::fs::canonicalize(repo_root)
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let canon_home =
        std::fs::canonicalize(&home).unwrap_or_else(|_| home.clone());

    if canon_repo == canon_home {
        return Err(format!(
            "refusing to initialize in home directory ({}).\n\
             Lobster writes .claude/settings.json and .mcp.json, which \
             would overwrite your global Claude Code configuration.\n\
             Run `lobster init` inside a project repository instead.",
            canon_home.display()
        ));
    }
    Ok(())
}

/// Merge lobster hooks into an existing Claude settings object.
///
/// If `existing` is `None`, returns a fresh settings object.
/// Otherwise, merges lobster's hook entries into the existing
/// `hooks` map without touching other keys or other hooks.
///
/// Lobster entries are identified by a command containing
/// `bin_path`. Duplicates are skipped.
#[must_use]
#[allow(clippy::missing_panics_doc)] // parse_or_empty always returns an object
pub fn merge_claude_settings(
    existing: Option<&str>,
    bin_path: &str,
) -> serde_json::Value {
    let mut root = parse_or_empty(existing);

    let hooks = root
        .as_object_mut()
        .expect("parse_or_empty always returns an object")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    for hook_type in ["UserPromptSubmit", "PostToolUse"] {
        let lobster_entry = serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": format!("{bin_path} hook {hook_type}"),
                "timeout": 10
            }]
        });

        let arr = hooks
            .as_object_mut()
            .expect("hooks is always an object")
            .entry(hook_type)
            .or_insert_with(|| serde_json::json!([]));

        let Some(entries) = arr.as_array_mut() else {
            // Malformed — replace with array containing our entry
            *arr = serde_json::json!([lobster_entry]);
            continue;
        };

        let already_present = entries
            .iter()
            .any(|entry| has_command_containing(entry, bin_path));

        if !already_present {
            entries.push(lobster_entry);
        }
    }

    root
}

/// Merge lobster's MCP server into an existing `.mcp.json` object.
///
/// If `existing` is `None`, returns a fresh MCP config.
/// Otherwise, adds/updates the `lobster` key under `mcpServers`
/// without touching other servers.
#[must_use]
#[allow(clippy::missing_panics_doc)] // parse_or_empty always returns an object
pub fn merge_mcp_config(
    existing: Option<&str>,
    bin_path: &str,
) -> serde_json::Value {
    let mut root = parse_or_empty(existing);

    let servers = root
        .as_object_mut()
        .expect("parse_or_empty always returns an object")
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(obj) = servers.as_object_mut() {
        obj.insert(
            "lobster".to_string(),
            serde_json::json!({
                "command": bin_path,
                "args": ["mcp"]
            }),
        );
    }

    root
}

/// Parse JSON or return an empty object.
fn parse_or_empty(json: Option<&str>) -> serde_json::Value {
    json.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Check if a hook entry contains a command string that includes `needle`.
fn has_command_containing(entry: &serde_json::Value, needle: &str) -> bool {
    entry
        .get("hooks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|hooks_arr| {
            hooks_arr.iter().any(|h| {
                h.get("command")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|c| c.contains(needle))
            })
        })
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // ── reject_home_directory ──────────────────────────────

    #[test]
    fn test_reject_home_directory() {
        if let Some(home) = home_dir() {
            let result = reject_home_directory(&home);
            assert!(result.is_err(), "home dir must be rejected");
            assert!(
                result.unwrap_err().contains("refusing"),
                "error message should explain the refusal"
            );
        }
    }

    #[test]
    fn test_accept_non_home_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let result = reject_home_directory(tmp.path());
        assert!(result.is_ok(), "non-home dir should be accepted");
    }

    // ── merge_claude_settings ──────────────────────────────

    #[test]
    fn test_merge_into_empty() {
        let result = merge_claude_settings(None, "/usr/bin/lobster");
        let hooks = result.get("hooks").unwrap().as_object().unwrap();
        assert!(hooks.contains_key("UserPromptSubmit"));
        assert!(hooks.contains_key("PostToolUse"));
    }

    #[test]
    fn test_merge_preserves_existing_hooks() {
        let existing = r#"{
            "hooks": {
                "UserPromptSubmit": [{
                    "hooks": [{
                        "type": "command",
                        "command": "other-tool hook submit",
                        "timeout": 5
                    }]
                }]
            },
            "someOtherKey": true
        }"#;

        let result = merge_claude_settings(Some(existing), "/usr/bin/lobster");

        // Existing hook preserved
        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(submit.len(), 2, "should have original + lobster");
        assert_eq!(
            submit[0]["hooks"][0]["command"].as_str().unwrap(),
            "other-tool hook submit"
        );

        // Other keys preserved
        assert_eq!(result["someOtherKey"], true);
    }

    #[test]
    fn test_merge_is_idempotent() {
        let bin = "/usr/bin/lobster";
        let first = merge_claude_settings(None, bin);
        let first_str = serde_json::to_string(&first).unwrap();
        let second = merge_claude_settings(Some(&first_str), bin);

        let submit = second["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(submit.len(), 1, "must not duplicate lobster hooks");
    }

    /// Merge is idempotent for any number of rounds.
    #[hegel::test(test_cases = 50)]
    fn prop_merge_settings_idempotent(tc: TestCase) {
        let bin: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(50)
                .alphabet("abcdefghijklmnopqrstuvwxyz/-_"),
        );
        let rounds: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(5));

        let mut current = merge_claude_settings(None, &bin);
        for _ in 0..rounds {
            let json = serde_json::to_string(&current).unwrap();
            current = merge_claude_settings(Some(&json), &bin);
        }

        let submit = current["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(
            submit.len(),
            1,
            "lobster hook must appear exactly once after {rounds} merges"
        );
    }

    /// Merge never removes existing hook entries.
    #[hegel::test(test_cases = 50)]
    fn prop_merge_settings_preserves_others(tc: TestCase) {
        let n_existing: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut hooks = Vec::new();
        for i in 0..n_existing {
            hooks.push(serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!("tool-{i} hook"),
                    "timeout": 5
                }]
            }));
        }

        let existing = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": hooks
            }
        });

        let result = merge_claude_settings(
            Some(&serde_json::to_string(&existing).unwrap()),
            "/bin/lobster",
        );

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        // All original entries + 1 lobster entry
        assert_eq!(submit.len(), n_existing + 1);
    }

    // ── merge_mcp_config ───────────────────────────────────

    #[test]
    fn test_mcp_merge_into_empty() {
        let result = merge_mcp_config(None, "/usr/bin/lobster");
        assert_eq!(
            result["mcpServers"]["lobster"]["command"].as_str().unwrap(),
            "/usr/bin/lobster"
        );
    }

    #[test]
    fn test_mcp_merge_preserves_other_servers() {
        let existing = r#"{
            "mcpServers": {
                "other-tool": {
                    "command": "other-bin",
                    "args": ["serve"]
                }
            }
        }"#;

        let result = merge_mcp_config(Some(existing), "/usr/bin/lobster");

        let servers = result["mcpServers"].as_object().unwrap();
        assert!(
            servers.contains_key("other-tool"),
            "must keep other servers"
        );
        assert!(servers.contains_key("lobster"), "must add lobster");
        assert_eq!(
            servers["other-tool"]["command"].as_str().unwrap(),
            "other-bin"
        );
    }

    #[test]
    fn test_mcp_merge_is_idempotent() {
        let bin = "/usr/bin/lobster";
        let first = merge_mcp_config(None, bin);
        let first_str = serde_json::to_string(&first).unwrap();
        let second = merge_mcp_config(Some(&first_str), bin);

        let servers = second["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1, "must not duplicate lobster");
    }

    /// MCP merge never removes existing servers.
    #[hegel::test(test_cases = 50)]
    fn prop_mcp_merge_preserves_servers(tc: TestCase) {
        let n_servers: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
        let mut servers = serde_json::Map::new();
        for i in 0..n_servers {
            servers.insert(
                format!("server-{i}"),
                serde_json::json!({"command": format!("bin-{i}")}),
            );
        }

        let existing = serde_json::json!({ "mcpServers": servers });
        let result = merge_mcp_config(
            Some(&serde_json::to_string(&existing).unwrap()),
            "/bin/lobster",
        );

        let merged = result["mcpServers"].as_object().unwrap();
        // All original servers + lobster
        assert_eq!(merged.len(), n_servers + 1);
        for i in 0..n_servers {
            assert!(
                merged.contains_key(&format!("server-{i}")),
                "server-{i} must be preserved"
            );
        }
    }
}
