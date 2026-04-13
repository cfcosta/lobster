//! Deterministic convention detection for repo identity profiles.
//!
//! Scans raw events in an episode range to detect stable repo
//! conventions: build tools, test frameworks, VCS, languages, etc.
//! No LLM needed — pure heuristic pattern matching.

use crate::store::{
    db::LobsterDb,
    ids::{EpisodeId, RepoId},
    schema::{Confidence, EvidenceRef, ProfileFact, RepoProfile},
};

/// Minimum number of episodes supporting a fact before it's promoted.
const MIN_SUPPORT: u32 = 2;

/// A convention signal extracted from a single event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Signal {
    /// Convention statement (e.g., "uses nix flakes").
    statement: String,
}

/// Scan raw events to extract convention signals.
///
/// Looks at tool names and file paths in hook event payloads to
/// detect recurring patterns: build tools, test frameworks, VCS
/// commands, and languages.
#[must_use]
fn extract_signals(payload: &str) -> Vec<Signal> {
    let mut signals = Vec::new();

    // Parse as JSON to extract tool_name and file paths
    let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) else {
        return signals;
    };

    // Tool name signals
    if let Some(tool) = val.get("tool_name").and_then(serde_json::Value::as_str)
    {
        detect_tool_signals(tool, &mut signals);
    }

    // File path signals from tool_input
    if let Some(input) = val.get("tool_input") {
        if let Some(path) = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(serde_json::Value::as_str)
        {
            detect_path_signals(path, &mut signals);
        }
        if let Some(cmd) =
            input.get("command").and_then(serde_json::Value::as_str)
        {
            detect_command_signals(cmd, &mut signals);
        }
    }

    signals
}

fn detect_tool_signals(_tool: &str, _signals: &mut Vec<Signal>) {
    // Tool-name-only signals are intentionally sparse. Most
    // convention detection comes from file paths and commands.
}

fn detect_path_signals(path: &str, signals: &mut Vec<Signal>) {
    // Build system files
    if path.ends_with("flake.nix") || path.ends_with("flake.lock") {
        signals.push(Signal {
            statement: "uses nix flakes for builds".into(),
        });
    }
    if path.ends_with("Cargo.toml") || path.ends_with("Cargo.lock") {
        signals.push(Signal {
            statement: "Rust project using Cargo".into(),
        });
    }
    if path.ends_with("package.json") || path.ends_with("package-lock.json") {
        signals.push(Signal {
            statement: "JavaScript/TypeScript project using npm".into(),
        });
    }
    if path.ends_with("pyproject.toml") || path.ends_with("setup.py") {
        signals.push(Signal {
            statement: "Python project".into(),
        });
    }
    if path.ends_with("go.mod") || path.ends_with("go.sum") {
        signals.push(Signal {
            statement: "Go project using modules".into(),
        });
    }

    // Test frameworks
    if path.contains("hegel") || path.contains("hegeltest") {
        signals.push(Signal {
            statement: "uses hegel property-based testing".into(),
        });
    }

    // Config files
    if path.ends_with(".claude/settings.json") {
        signals.push(Signal {
            statement: "uses Claude Code".into(),
        });
    }
    if path.ends_with(".mcp.json") {
        signals.push(Signal {
            statement: "uses MCP servers".into(),
        });
    }
}

fn detect_command_signals(cmd: &str, signals: &mut Vec<Signal>) {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("");

    match first {
        "jj" => signals.push(Signal {
            statement: "uses jujutsu (jj) for version control".into(),
        }),
        "git" => signals.push(Signal {
            statement: "uses git for version control".into(),
        }),
        "nix" => signals.push(Signal {
            statement: "uses nix flakes for builds".into(),
        }),
        "cargo" => {
            signals.push(Signal {
                statement: "Rust project using Cargo".into(),
            });
            if words.get(1) == Some(&"test") {
                signals.push(Signal {
                    statement: "uses cargo test".into(),
                });
            }
            if words.get(1) == Some(&"clippy") {
                signals.push(Signal {
                    statement: "uses cargo clippy for linting".into(),
                });
            }
        }
        "npm" | "yarn" | "pnpm" | "bun" => signals.push(Signal {
            statement: "JavaScript/TypeScript project using npm".into(),
        }),
        "pytest" | "python" | "uv" => signals.push(Signal {
            statement: "Python project".into(),
        }),
        _ => {}
    }
}

/// Aggregate signals across episodes into convention facts.
///
/// Groups signals by statement, counts episode support, and
/// promotes signals that appear in at least `MIN_SUPPORT` episodes.
/// The `now_ms` parameter is the timestamp to use for new facts
/// (avoids non-determinism from system clock).
#[must_use]
pub(crate) fn aggregate_conventions(
    episode_signals: &[(EpisodeId, Vec<Signal>)],
    now_ms: i64,
) -> Vec<ProfileFact> {
    use std::collections::HashMap;

    let mut counts: HashMap<String, Vec<EpisodeId>> = HashMap::new();

    for (ep_id, signals) in episode_signals {
        // Deduplicate within the same episode
        let mut seen = std::collections::HashSet::new();
        for sig in signals {
            if seen.insert(&sig.statement) {
                counts
                    .entry(sig.statement.clone())
                    .or_default()
                    .push(*ep_id);
            }
        }
    }

    let mut facts: Vec<ProfileFact> = counts
        .into_iter()
        .filter(|(_, eps)| eps.len() >= MIN_SUPPORT as usize)
        .map(|(statement, eps)| {
            let evidence: Vec<EvidenceRef> = eps
                .iter()
                .take(3) // Limit evidence refs
                .map(|ep_id| EvidenceRef {
                    episode_id: *ep_id,
                    span_summary: format!("detected convention: {statement}"),
                })
                .collect();
            #[allow(clippy::cast_possible_truncation)]
            ProfileFact {
                statement,
                evidence,
                first_seen_ts_utc_ms: now_ms,
                last_confirmed_ts_utc_ms: now_ms,
                support_count: eps.len() as u32,
                confidence: match eps.len() {
                    0..=2 => Confidence::Low,
                    3..=5 => Confidence::Medium,
                    _ => Confidence::High,
                },
            }
        })
        .collect();

    // Sort deterministically: by support count (desc), then statement
    facts.sort_by(|a, b| {
        b.support_count
            .cmp(&a.support_count)
            .then_with(|| a.statement.cmp(&b.statement))
    });

    facts
}

/// Build or update a repo profile from raw events in the database.
///
/// Scans all raw events, extracts convention signals per episode,
/// aggregates them, and merges with the existing profile (preserving
/// timestamps from previously seen facts).
#[must_use]
pub fn build_profile(db: &LobsterDb, repo_id: &RepoId) -> RepoProfile {
    let episode_signals = scan_events_for_signals(db);

    let now_ms = chrono::Utc::now().timestamp_millis();
    let new_conventions = aggregate_conventions(&episode_signals, now_ms);

    // Load existing profile to preserve timestamps
    let existing =
        crate::store::crud::get_repo_profile(db, &repo_id.raw()).ok();

    let conventions = merge_facts(
        existing.as_ref().map_or(&[][..], |p| &p.conventions),
        &new_conventions,
    );

    let now_ms = chrono::Utc::now().timestamp_millis();

    RepoProfile {
        repo_id: *repo_id,
        conventions,
        preferences: existing.map_or_else(Vec::new, |p| p.preferences),
        updated_ts_utc_ms: now_ms,
        revision: "v1".into(),
    }
}

/// Scan raw events and group signals by episode.
fn scan_events_for_signals(db: &LobsterDb) -> Vec<(EpisodeId, Vec<Signal>)> {
    use crate::store::schema::RawEvent;

    let rtxn = match db.env.read_txn() {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    // Collect all raw events, keyed by their repo-derived episode proxy.
    // For simplicity, each raw event's seq is grouped into an episode
    // by scanning the episodes table.
    let mut all_events: Vec<RawEvent> = Vec::new();
    let iter = match db.raw_events.iter(&rtxn) {
        Ok(i) => i,
        Err(_) => return vec![],
    };
    for entry in iter.flatten() {
        let (_, bytes) = entry;
        if let Ok(event) = serde_json::from_slice::<RawEvent>(bytes) {
            all_events.push(event);
        }
    }

    // Group events by episode
    let mut episode_map: std::collections::HashMap<EpisodeId, Vec<Signal>> =
        std::collections::HashMap::new();

    // Read episode boundaries
    let episode_iter = match db.episodes.iter(&rtxn) {
        Ok(i) => i,
        Err(_) => return vec![],
    };
    let mut episodes = Vec::new();
    for entry in episode_iter.flatten() {
        let (_, bytes) = entry;
        if let Ok(ep) =
            serde_json::from_slice::<crate::store::schema::Episode>(bytes)
        {
            episodes.push(ep);
        }
    }

    for ep in &episodes {
        let mut signals = Vec::new();
        for event in &all_events {
            if event.seq >= ep.start_seq
                && event.seq <= ep.end_seq
                && event.repo_id == ep.repo_id
            {
                let payload = String::from_utf8_lossy(&event.payload_bytes);
                signals.extend(extract_signals(&payload));
            }
        }
        episode_map
            .entry(ep.episode_id)
            .or_default()
            .extend(signals);
    }

    episode_map.into_iter().collect()
}

/// Merge new facts with existing ones, preserving first_seen timestamps.
fn merge_facts(
    existing: &[ProfileFact],
    new_facts: &[ProfileFact],
) -> Vec<ProfileFact> {
    let mut merged: Vec<ProfileFact> = Vec::new();

    for new in new_facts {
        if let Some(old) =
            existing.iter().find(|f| f.statement == new.statement)
        {
            // Preserve first_seen from the older record
            merged.push(ProfileFact {
                first_seen_ts_utc_ms: old.first_seen_ts_utc_ms,
                last_confirmed_ts_utc_ms: new.last_confirmed_ts_utc_ms,
                support_count: new.support_count,
                confidence: new.confidence,
                statement: new.statement.clone(),
                evidence: new.evidence.clone(),
            });
        } else {
            merged.push(new.clone());
        }
    }

    // Cap at MAX_PROFILE_FACTS
    merged.truncate(crate::store::schema::MAX_PROFILE_FACTS);
    merged
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::ids::EpisodeId;

    // ── extract_signals tests ────────────────────────────────

    #[test]
    fn test_extract_nix_signal_from_path() {
        let payload =
            r#"{"tool_name":"Read","tool_input":{"file_path":"flake.nix"}}"#;
        let signals = extract_signals(payload);
        assert!(
            signals.iter().any(|s| s.statement.contains("nix")),
            "should detect nix flakes from flake.nix path"
        );
    }

    #[test]
    fn test_extract_cargo_signal_from_path() {
        let payload =
            r#"{"tool_name":"Read","tool_input":{"file_path":"Cargo.toml"}}"#;
        let signals = extract_signals(payload);
        assert!(
            signals.iter().any(|s| s.statement.contains("Cargo")),
            "should detect Cargo from Cargo.toml path"
        );
    }

    #[test]
    fn test_extract_jj_signal_from_command() {
        let payload = r#"{"tool_name":"Bash","tool_input":{"command":"jj commit -m test"}}"#;
        let signals = extract_signals(payload);
        assert!(
            signals.iter().any(|s| s.statement.contains("jujutsu")),
            "should detect jj from command"
        );
    }

    #[test]
    fn test_extract_git_signal_from_command() {
        let payload =
            r#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
        let signals = extract_signals(payload);
        assert!(
            signals.iter().any(|s| s.statement.contains("git")),
            "should detect git from command"
        );
    }

    #[test]
    fn test_extract_no_signals_from_empty() {
        let signals = extract_signals("{}");
        assert!(signals.is_empty());
    }

    #[test]
    fn test_extract_no_signals_from_invalid_json() {
        let signals = extract_signals("not json");
        assert!(signals.is_empty());
    }

    // ── aggregate_conventions tests ──────────────────────────

    #[test]
    fn test_aggregate_empty() {
        let facts = aggregate_conventions(&[], 1000);
        assert!(facts.is_empty());
    }

    #[test]
    fn test_aggregate_below_threshold() {
        let ep1 = EpisodeId::derive(b"ep1");
        let signals = vec![(
            ep1,
            vec![Signal {
                statement: "uses nix".into(),
            }],
        )];
        let facts = aggregate_conventions(&signals, 1000);
        assert!(facts.is_empty(), "single episode should not promote");
    }

    #[test]
    fn test_aggregate_above_threshold() {
        let ep1 = EpisodeId::derive(b"ep1");
        let ep2 = EpisodeId::derive(b"ep2");
        let signals = vec![
            (
                ep1,
                vec![Signal {
                    statement: "uses nix".into(),
                }],
            ),
            (
                ep2,
                vec![Signal {
                    statement: "uses nix".into(),
                }],
            ),
        ];
        let facts = aggregate_conventions(&signals, 1000);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].support_count, 2);
    }

    #[test]
    fn test_aggregate_deduplicates_within_episode() {
        let ep1 = EpisodeId::derive(b"ep1");
        let ep2 = EpisodeId::derive(b"ep2");
        let signals = vec![
            (
                ep1,
                vec![
                    Signal {
                        statement: "uses nix".into(),
                    },
                    Signal {
                        statement: "uses nix".into(),
                    },
                ],
            ),
            (
                ep2,
                vec![Signal {
                    statement: "uses nix".into(),
                }],
            ),
        ];
        let facts = aggregate_conventions(&signals, 1000);
        assert_eq!(facts[0].support_count, 2, "should count 2 episodes, not 3");
    }

    // ── Property: aggregate is deterministic ─────────────────

    #[hegel::test(test_cases = 50)]
    fn prop_aggregate_deterministic(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let statements = vec![
            "uses nix",
            "Rust project using Cargo",
            "uses jujutsu (jj) for version control",
        ];

        let mut episode_signals = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            let n_sigs: usize =
                tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
            let mut sigs = Vec::new();
            for _ in 0..n_sigs {
                let stmt = tc.draw(gs::sampled_from(statements.clone()));
                sigs.push(Signal {
                    statement: stmt.to_string(),
                });
            }
            episode_signals.push((ep, sigs));
        }

        let result1 = aggregate_conventions(&episode_signals, 1000);
        let result2 = aggregate_conventions(&episode_signals, 1000);
        assert_eq!(result1, result2, "aggregation must be deterministic");
    }

    // ── Property: support_count <= number of episodes ────────

    #[hegel::test(test_cases = 50)]
    fn prop_support_count_bounded(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));

        let mut episode_signals = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            let sigs = vec![Signal {
                statement: "uses nix".into(),
            }];
            episode_signals.push((ep, sigs));
        }

        let facts = aggregate_conventions(&episode_signals, 1000);
        for fact in &facts {
            assert!(
                fact.support_count as usize <= n_episodes,
                "support {} exceeds episodes {}",
                fact.support_count,
                n_episodes
            );
        }
    }

    // ── Property: all promoted facts have evidence ───────────

    #[hegel::test(test_cases = 50)]
    fn prop_promoted_facts_have_evidence(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(6));

        let mut episode_signals = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            episode_signals.push((
                ep,
                vec![Signal {
                    statement: "test convention".into(),
                }],
            ));
        }

        let facts = aggregate_conventions(&episode_signals, 1000);
        for fact in &facts {
            assert!(
                !fact.evidence.is_empty(),
                "promoted facts must have evidence"
            );
        }
    }

    // ── merge_facts tests ────────────────────────────────────

    #[test]
    fn test_merge_preserves_first_seen() {
        let old = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 1000,
            last_confirmed_ts_utc_ms: 1000,
            support_count: 2,
            confidence: Confidence::Low,
        }];
        let new_facts = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 5000,
            last_confirmed_ts_utc_ms: 5000,
            support_count: 4,
            confidence: Confidence::Medium,
        }];

        let merged = merge_facts(&old, &new_facts);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].first_seen_ts_utc_ms, 1000,
            "should preserve original first_seen"
        );
        assert_eq!(merged[0].support_count, 4);
    }

    #[test]
    fn test_merge_adds_new_facts() {
        let old = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 1000,
            last_confirmed_ts_utc_ms: 1000,
            support_count: 2,
            confidence: Confidence::Low,
        }];
        let new_facts = vec![
            ProfileFact {
                statement: "uses nix".into(),
                evidence: vec![],
                first_seen_ts_utc_ms: 5000,
                last_confirmed_ts_utc_ms: 5000,
                support_count: 4,
                confidence: Confidence::Medium,
            },
            ProfileFact {
                statement: "Rust project".into(),
                evidence: vec![],
                first_seen_ts_utc_ms: 5000,
                last_confirmed_ts_utc_ms: 5000,
                support_count: 3,
                confidence: Confidence::Medium,
            },
        ];

        let merged = merge_facts(&old, &new_facts);
        assert_eq!(merged.len(), 2);
    }

    // ── Property: merge never exceeds MAX_PROFILE_FACTS ──────

    #[hegel::test(test_cases = 30)]
    fn prop_merge_bounded(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(30));
        let mut new_facts = Vec::new();
        for i in 0..n {
            new_facts.push(ProfileFact {
                statement: format!("fact-{i}"),
                evidence: vec![],
                first_seen_ts_utc_ms: 1000,
                last_confirmed_ts_utc_ms: 1000,
                support_count: 2,
                confidence: Confidence::Low,
            });
        }

        let merged = merge_facts(&[], &new_facts);
        assert!(
            merged.len() <= crate::store::schema::MAX_PROFILE_FACTS,
            "got {} facts, max is {}",
            merged.len(),
            crate::store::schema::MAX_PROFILE_FACTS,
        );
    }
}
