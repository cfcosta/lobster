//! Core memory: always-injected identity and high-value decisions.
//!
//! Injects the repo profile (conventions + preferences) and the
//! top-N highest-confidence decisions on every `UserPromptSubmit`
//! regardless of query match. Inspired by Hermes's `MEMORY.md` +
//! `USER.md` frozen snapshot pattern.

use crate::store::{
    db::LobsterDb,
    schema::{Confidence, Decision, RepoProfile},
};

/// Maximum number of core memory items to inject.
const MAX_CORE_ITEMS: usize = 3;

/// A core memory item ready for injection into recall output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreMemoryItem {
    pub statement: String,
    pub confidence: Confidence,
    pub valid_from_ts_utc_ms: i64,
}

/// Load the top-N highest-confidence, still-valid decisions as
/// core memory items.
///
/// Selection criteria (in priority order):
/// 1. Only decisions with `valid_to` = None (not superseded)
/// 2. Highest confidence first (High > Medium > Low)
/// 3. Most recent `valid_from` as tiebreaker
///
/// Returns at most `MAX_CORE_ITEMS` items.
#[must_use]
pub fn load_core_memory(db: &LobsterDb) -> Vec<CoreMemoryItem> {
    load_core_memory_n(db, MAX_CORE_ITEMS)
}

/// Load core memory with a configurable limit.
#[must_use]
pub fn load_core_memory_n(db: &LobsterDb, limit: usize) -> Vec<CoreMemoryItem> {
    let Ok(rtxn) = db.env.read_txn() else {
        return vec![];
    };
    let Ok(iter) = db.decisions.iter(&rtxn) else {
        return vec![];
    };

    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut candidates: Vec<Decision> = Vec::new();
    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(dec) = serde_json::from_slice::<Decision>(value) {
            // Only include still-valid decisions
            if dec.valid_to_ts_utc_ms.is_some_and(|vt| vt <= now_ms) {
                continue;
            }
            candidates.push(dec);
        }
    }

    // Sort: highest confidence first, then most recent
    candidates.sort_by(|a, b| {
        confidence_rank(b.confidence)
            .cmp(&confidence_rank(a.confidence))
            .then_with(|| b.valid_from_ts_utc_ms.cmp(&a.valid_from_ts_utc_ms))
    });

    candidates
        .into_iter()
        .take(limit)
        .map(|d| CoreMemoryItem {
            statement: d.statement,
            confidence: d.confidence,
            valid_from_ts_utc_ms: d.valid_from_ts_utc_ms,
        })
        .collect()
}

/// Numeric rank for confidence levels (higher = more important).
const fn confidence_rank(c: Confidence) -> u8 {
    match c {
        Confidence::High => 3,
        Confidence::Medium => 2,
        Confidence::Low => 1,
    }
}

/// Format core memory items as a prefix string for hook output.
///
/// Produces a compact block like:
/// ```text
/// [Core Memory]
/// • Use redb for storage (High confidence)
/// • Deploy with zero downtime (Medium confidence)
/// ```
///
/// Returns empty string if no core items.
#[must_use]
pub fn format_core_memory(items: &[CoreMemoryItem]) -> String {
    if items.is_empty() {
        return String::new();
    }

    let mut lines = Vec::with_capacity(items.len() + 1);
    lines.push("[Core Memory]".to_string());
    for item in items {
        lines.push(format!(
            "• {} ({:?} confidence)",
            item.statement, item.confidence
        ));
    }
    lines.join("\n")
}

/// Load the repo profile for a given repo path.
///
/// Returns `None` if no profile exists or the DB is unavailable.
#[must_use]
pub fn load_repo_profile(
    db: &LobsterDb,
    repo_path: &str,
) -> Option<RepoProfile> {
    let repo_id = crate::store::ids::RepoId::derive(repo_path.as_bytes());
    crate::store::crud::get_repo_profile(db, &repo_id.raw()).ok()
}

/// Format a repo profile as a prefix string for hook output.
///
/// Produces a compact block like:
/// ```text
/// [Repo Profile]
/// • uses nix flakes for builds
/// • Rust project using Cargo
/// • uses jujutsu (jj) for version control
/// ```
///
/// Returns empty string if the profile has no facts.
#[must_use]
pub fn format_repo_profile(profile: &RepoProfile) -> String {
    if profile.conventions.is_empty() && profile.preferences.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    lines.push("[Repo Profile]".to_string());

    for fact in &profile.conventions {
        lines.push(format!("• {}", fact.statement));
    }
    for fact in &profile.preferences {
        lines.push(format!("• {}", fact.statement));
    }

    lines.join("\n")
}

/// Assemble the full always-injected memory block.
///
/// Combines the repo profile and core decisions into a single
/// string. The profile comes first (identity context), followed
/// by decisions (architectural choices).
#[must_use]
pub fn format_full_core_block(
    profile: Option<&RepoProfile>,
    decisions: &[CoreMemoryItem],
) -> String {
    let profile_text = profile.map_or_else(String::new, format_repo_profile);
    let decision_text = format_core_memory(decisions);

    match (profile_text.is_empty(), decision_text.is_empty()) {
        (true, true) => String::new(),
        (true, false) => decision_text,
        (false, true) => profile_text,
        (false, false) => format!("{profile_text}\n\n{decision_text}"),
    }
}

/// Filter core memory items that already appear in recall results.
///
/// Removes items whose statement matches a decision in the recall
/// payload to avoid showing the same decision twice.
#[must_use]
pub fn dedup_against_recall(
    core_items: &[CoreMemoryItem],
    recall: &crate::hooks::recall::RecallPayload,
) -> Vec<CoreMemoryItem> {
    use crate::hooks::recall::RecallItem;

    // Collect all decision statements from recall
    let recall_statements: std::collections::HashSet<&str> = recall
        .items
        .iter()
        .filter_map(|item| match item {
            RecallItem::Decision(d) => Some(d.statement.as_str()),
            _ => None,
        })
        .collect();

    core_items
        .iter()
        .filter(|item| !recall_statements.contains(item.statement.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        crud,
        db,
        ids::{DecisionId, EpisodeId, RepoId},
        schema::EvidenceRef,
    };

    fn make_decision(
        suffix: &[u8],
        statement: &str,
        confidence: Confidence,
        valid_from: i64,
    ) -> Decision {
        Decision {
            decision_id: DecisionId::derive(suffix),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(suffix),
            task_id: None,
            statement: statement.into(),
            rationale: "test".into(),
            confidence,
            valid_from_ts_utc_ms: valid_from,
            valid_to_ts_utc_ms: None,
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(suffix),
                span_summary: "test".into(),
            }],
            premises: vec![],
        }
    }

    // -- Unit: empty DB returns empty core memory --
    #[test]
    fn test_empty_db() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let items = load_core_memory(&database);
        assert!(items.is_empty());
    }

    // -- Unit: high confidence decisions come first --
    #[test]
    fn test_confidence_ordering() {
        let (database, _dir) = db::open_in_memory().unwrap();

        crud::put_decision(
            &database,
            &make_decision(b"low", "Low thing", Confidence::Low, 1000),
        )
        .unwrap();
        crud::put_decision(
            &database,
            &make_decision(b"high", "High thing", Confidence::High, 1000),
        )
        .unwrap();
        crud::put_decision(
            &database,
            &make_decision(b"med", "Med thing", Confidence::Medium, 1000),
        )
        .unwrap();

        let items = load_core_memory(&database);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].confidence, Confidence::High);
        assert_eq!(items[1].confidence, Confidence::Medium);
        assert_eq!(items[2].confidence, Confidence::Low);
    }

    // -- Unit: superseded decisions are excluded --
    #[test]
    fn test_superseded_excluded() {
        let (database, _dir) = db::open_in_memory().unwrap();

        let mut old =
            make_decision(b"old", "Old decision", Confidence::High, 1000);
        old.valid_to_ts_utc_ms = Some(2000);
        crud::put_decision(&database, &old).unwrap();

        let current =
            make_decision(b"new", "Current decision", Confidence::High, 3000);
        crud::put_decision(&database, &current).unwrap();

        let items = load_core_memory(&database);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].statement, "Current decision");
    }

    // -- Unit: limit is respected --
    #[test]
    fn test_limit() {
        let (database, _dir) = db::open_in_memory().unwrap();
        for i in 0..10u32 {
            crud::put_decision(
                &database,
                &make_decision(
                    &i.to_le_bytes(),
                    &format!("Decision {i}"),
                    Confidence::High,
                    i64::from(i) * 1000,
                ),
            )
            .unwrap();
        }

        let items = load_core_memory_n(&database, 2);
        assert_eq!(items.len(), 2);
    }

    // -- Property: result count <= limit --
    #[hegel::test(test_cases = 30)]
    fn prop_result_bounded(tc: TestCase) {
        let (database, _dir) = db::open_in_memory().unwrap();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(8));
        let limit: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(5));

        for i in 0..n {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
            crud::put_decision(
                &database,
                &make_decision(
                    &(i as u32).to_le_bytes(),
                    &format!("Decision {i}"),
                    Confidence::High,
                    (i * 1000) as i64,
                ),
            )
            .unwrap();
        }

        let items = load_core_memory_n(&database, limit);
        assert!(
            items.len() <= limit,
            "got {} items with limit {}",
            items.len(),
            limit
        );
        assert!(items.len() <= n);
    }

    // -- Property: results are sorted by confidence then recency --
    #[hegel::test(test_cases = 30)]
    fn prop_sorted_by_confidence(tc: TestCase) {
        let (database, _dir) = db::open_in_memory().unwrap();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(6));

        let confidences =
            vec![Confidence::Low, Confidence::Medium, Confidence::High];
        for i in 0..n {
            let conf = tc.draw(gs::sampled_from(confidences.clone()));
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
            crud::put_decision(
                &database,
                &make_decision(
                    &(i as u32).to_le_bytes(),
                    &format!("Decision {i}"),
                    conf,
                    (i * 1000) as i64,
                ),
            )
            .unwrap();
        }

        let items = load_core_memory(&database);
        for window in items.windows(2) {
            assert!(
                confidence_rank(window[0].confidence)
                    >= confidence_rank(window[1].confidence),
                "not sorted by confidence"
            );
        }
    }

    // -- Unit: format produces expected output --
    #[test]
    fn test_format_core_memory() {
        let items = vec![
            CoreMemoryItem {
                statement: "Use redb for storage".into(),
                confidence: Confidence::High,
                valid_from_ts_utc_ms: 1000,
            },
            CoreMemoryItem {
                statement: "Deploy with zero downtime".into(),
                confidence: Confidence::Medium,
                valid_from_ts_utc_ms: 2000,
            },
        ];

        let output = format_core_memory(&items);
        assert!(output.contains("[Core Memory]"));
        assert!(output.contains("• Use redb for storage (High confidence)"));
        assert!(output.contains("• Deploy with zero downtime"));
    }

    // -- Unit: format empty returns empty --
    #[test]
    fn test_format_empty() {
        assert!(format_core_memory(&[]).is_empty());
    }

    // -- Unit: dedup removes items present in recall --
    #[test]
    fn test_dedup_against_recall() {
        use crate::{
            hooks::recall::{RecallItem, RecallPayload},
            rank::evidence::DecisionEvidence,
        };

        let core_items = vec![
            CoreMemoryItem {
                statement: "Use redb for storage".into(),
                confidence: Confidence::High,
                valid_from_ts_utc_ms: 1000,
            },
            CoreMemoryItem {
                statement: "Deploy on Fridays".into(),
                confidence: Confidence::Medium,
                valid_from_ts_utc_ms: 2000,
            },
        ];

        // Recall contains "Use redb for storage" as a decision
        let recall = RecallPayload {
            items: vec![RecallItem::Decision(DecisionEvidence {
                statement: "Use redb for storage".into(),
                rationale: "ACID".into(),
                confidence: "High".into(),
                evidence: vec![],
                task_context: None,
            })],
            truncated: None,
            latency_ms: 10,
        };

        let deduped = dedup_against_recall(&core_items, &recall);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].statement, "Deploy on Fridays");
    }

    // -- Unit: dedup keeps all when no overlap --
    #[test]
    fn test_dedup_no_overlap() {
        use crate::hooks::recall::{RecallItem, RecallPayload};

        let core_items = vec![CoreMemoryItem {
            statement: "Use redb".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1000,
        }];

        let recall = RecallPayload {
            items: vec![RecallItem::Hint {
                text: "something else".into(),
            }],
            truncated: None,
            latency_ms: 5,
        };

        let deduped = dedup_against_recall(&core_items, &recall);
        assert_eq!(deduped.len(), 1);
    }

    // -- Unit: dedup with empty recall returns all --
    #[test]
    fn test_dedup_empty_recall() {
        use crate::hooks::recall::RecallPayload;

        let core_items = vec![
            CoreMemoryItem {
                statement: "A".into(),
                confidence: Confidence::High,
                valid_from_ts_utc_ms: 1000,
            },
            CoreMemoryItem {
                statement: "B".into(),
                confidence: Confidence::Low,
                valid_from_ts_utc_ms: 2000,
            },
        ];

        let recall = RecallPayload {
            items: vec![],
            truncated: None,
            latency_ms: 0,
        };

        let deduped = dedup_against_recall(&core_items, &recall);
        assert_eq!(deduped.len(), 2);
    }

    // ── Repo Profile formatting tests ───────────────────────

    fn make_profile(
        n_conv: usize,
        n_pref: usize,
    ) -> crate::store::schema::RepoProfile {
        use crate::store::{
            ids::RepoId,
            schema::{EvidenceRef, ProfileFact},
        };

        let mut conventions = Vec::new();
        for i in 0..n_conv {
            conventions.push(ProfileFact {
                statement: format!("convention-{i}"),
                evidence: vec![EvidenceRef {
                    episode_id: crate::store::ids::EpisodeId::derive(
                        format!("ep-{i}").as_bytes(),
                    ),
                    span_summary: "test".into(),
                }],
                first_seen_ts_utc_ms: 1000,
                last_confirmed_ts_utc_ms: 1000,
                support_count: 2,
                confidence: Confidence::Medium,
            });
        }
        let mut preferences = Vec::new();
        for i in 0..n_pref {
            preferences.push(ProfileFact {
                statement: format!("preference-{i}"),
                evidence: vec![EvidenceRef {
                    episode_id: crate::store::ids::EpisodeId::derive(
                        format!("pep-{i}").as_bytes(),
                    ),
                    span_summary: "test".into(),
                }],
                first_seen_ts_utc_ms: 1000,
                last_confirmed_ts_utc_ms: 1000,
                support_count: 3,
                confidence: Confidence::High,
            });
        }

        crate::store::schema::RepoProfile {
            repo_id: RepoId::derive(b"repo"),
            conventions,
            preferences,
            updated_ts_utc_ms: 1000,
            revision: "v1".into(),
        }
    }

    // -- Unit: format_repo_profile empty --
    #[test]
    fn test_format_repo_profile_empty() {
        let profile = make_profile(0, 0);
        assert!(format_repo_profile(&profile).is_empty());
    }

    // -- Unit: format_repo_profile with conventions --
    #[test]
    fn test_format_repo_profile_with_conventions() {
        let profile = make_profile(2, 0);
        let output = format_repo_profile(&profile);
        assert!(output.contains("[Repo Profile]"));
        assert!(output.contains("• convention-0"));
        assert!(output.contains("• convention-1"));
    }

    // -- Unit: format_repo_profile with both --
    #[test]
    fn test_format_repo_profile_with_both() {
        let profile = make_profile(1, 1);
        let output = format_repo_profile(&profile);
        assert!(output.contains("convention-0"));
        assert!(output.contains("preference-0"));
    }

    // -- Unit: full core block with profile + decisions --
    #[test]
    fn test_full_core_block_both() {
        let profile = make_profile(1, 0);
        let decisions = vec![CoreMemoryItem {
            statement: "Use LMDB".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1000,
        }];

        let output = format_full_core_block(Some(&profile), &decisions);
        assert!(output.contains("[Repo Profile]"));
        assert!(output.contains("[Core Memory]"));
        // Profile should come before decisions
        let profile_pos = output.find("[Repo Profile]").unwrap();
        let decision_pos = output.find("[Core Memory]").unwrap();
        assert!(
            profile_pos < decision_pos,
            "profile must appear before decisions"
        );
    }

    // -- Unit: full core block with only profile --
    #[test]
    fn test_full_core_block_profile_only() {
        let profile = make_profile(1, 0);
        let output = format_full_core_block(Some(&profile), &[]);
        assert!(output.contains("[Repo Profile]"));
        assert!(!output.contains("[Core Memory]"));
    }

    // -- Unit: full core block with only decisions --
    #[test]
    fn test_full_core_block_decisions_only() {
        let decisions = vec![CoreMemoryItem {
            statement: "Use LMDB".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1000,
        }];
        let output = format_full_core_block(None, &decisions);
        assert!(output.contains("[Core Memory]"));
        assert!(!output.contains("[Repo Profile]"));
    }

    // -- Unit: full core block empty --
    #[test]
    fn test_full_core_block_empty() {
        let output = format_full_core_block(None, &[]);
        assert!(output.is_empty());
    }

    // -- Property: profile block never empty when facts exist --
    #[hegel::test(test_cases = 30)]
    fn prop_profile_format_nonempty_when_facts_exist(tc: hegel::TestCase) {
        let n_conv: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
        let n_pref: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let profile = make_profile(n_conv, n_pref);
        let output = format_repo_profile(&profile);
        assert!(!output.is_empty(), "profile with facts must produce output");
        assert!(output.contains("[Repo Profile]"));
    }

    // -- Property: full block ordering is always profile-first --
    #[hegel::test(test_cases = 30)]
    fn prop_full_block_profile_before_decisions(tc: hegel::TestCase) {
        let n_conv: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
        let n_dec: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(3));

        let profile = make_profile(n_conv, 0);
        let mut decisions = Vec::new();
        for i in 0..n_dec {
            decisions.push(CoreMemoryItem {
                statement: format!("decision-{i}"),
                confidence: Confidence::High,
                valid_from_ts_utc_ms: 1000,
            });
        }

        let output = format_full_core_block(Some(&profile), &decisions);
        let p = output.find("[Repo Profile]").unwrap();
        let d = output.find("[Core Memory]").unwrap();
        assert!(p < d, "profile must come before decisions");
    }
}
