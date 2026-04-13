//! Core memory: always-injected high-value decisions.
//!
//! Injects the top-N highest-confidence decisions on every
//! `UserPromptSubmit` regardless of query match. These are the
//! durable facts that should always be in context, inspired by
//! `ChatGPT`'s always-injected user facts and Hermes's frozen
//! `MEMORY.md` prompt block.

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{
    schema::{Confidence, Decision},
    tables,
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
pub fn load_core_memory(db: &Database) -> Vec<CoreMemoryItem> {
    load_core_memory_n(db, MAX_CORE_ITEMS)
}

/// Load core memory with a configurable limit.
#[must_use]
pub fn load_core_memory_n(db: &Database, limit: usize) -> Vec<CoreMemoryItem> {
    let Ok(read_txn) = db.begin_read() else {
        return vec![];
    };
    let Ok(table) = read_txn.open_table(tables::DECISIONS) else {
        return vec![];
    };
    let Ok(iter) = table.iter() else {
        return vec![];
    };

    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut candidates: Vec<Decision> = Vec::new();
    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(dec) = serde_json::from_slice::<Decision>(value.value()) {
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
        let database = db::open_in_memory().unwrap();
        let items = load_core_memory(&database);
        assert!(items.is_empty());
    }

    // -- Unit: high confidence decisions come first --
    #[test]
    fn test_confidence_ordering() {
        let database = db::open_in_memory().unwrap();

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
        let database = db::open_in_memory().unwrap();

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
        let database = db::open_in_memory().unwrap();
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
        let database = db::open_in_memory().unwrap();
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
        let database = db::open_in_memory().unwrap();
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
}
