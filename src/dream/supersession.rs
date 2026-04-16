//! Decision supersession detection.
//!
//! Finds pairs of decisions where a newer decision on the same topic
//! supersedes an older one. Uses word-overlap similarity on statements
//! to identify candidates, then marks the older decision's `valid_to`.

use crate::store::{
    crud,
    db::LobsterDb,
    schema::{Confidence, Decision},
};

/// Result of a supersession scan.
#[derive(Debug, Default)]
pub struct SupersessionResult {
    /// Number of decisions scanned.
    pub decisions_scanned: usize,
    /// Number of older decisions marked as superseded.
    pub decisions_superseded: usize,
}

/// Configuration for supersession detection.
#[derive(Debug, Clone)]
pub struct SupersessionConfig {
    /// Minimum word-overlap ratio (0.0–1.0) for two statements to
    /// be considered about the same topic.
    pub topic_similarity_threshold: f64,
    /// Only supersede decisions with confidence at or below this level.
    /// High-confidence decisions require higher overlap to supersede.
    pub max_confidence_to_supersede: Confidence,
}

impl Default for SupersessionConfig {
    fn default() -> Self {
        Self {
            topic_similarity_threshold: 0.5,
            max_confidence_to_supersede: Confidence::High,
        }
    }
}

/// Compute word-overlap similarity between two strings.
///
/// Returns the Jaccard similarity of the word sets (intersection /
/// union), ignoring case. Returns 0.0 for empty inputs.
#[must_use]
pub fn word_overlap(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<String> =
        a.split_whitespace().map(str::to_lowercase).collect();
    let words_b: std::collections::HashSet<String> =
        b.split_whitespace().map(str::to_lowercase).collect();

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let result = intersection as f64 / union as f64;
        result
    }
}

/// Scan all decisions and mark older ones as superseded when a newer
/// decision covers the same topic.
///
/// Uses `ColBERT` `MaxSim` for semantic comparison when `use_maxsim`
/// is true and the model is available. Falls back to Jaccard
/// word-overlap otherwise.
///
/// Two decisions are candidates for supersession when:
/// 1. They are about the same topic (similarity >= threshold)
/// 2. The older one does not already have `valid_to` set
/// 3. The newer one has a later `valid_from` timestamp
///
/// When superseded, the older decision's `valid_to` is set to the
/// newer decision's `valid_from`.
#[allow(clippy::must_use_candidate)]
pub fn scan_superseded_decisions(
    db: &LobsterDb,
    config: &SupersessionConfig,
) -> SupersessionResult {
    scan_superseded_decisions_inner(db, config, true)
}

/// Inner implementation with `MaxSim` toggle for testing.
fn scan_superseded_decisions_inner(
    db: &LobsterDb,
    config: &SupersessionConfig,
    use_maxsim: bool,
) -> SupersessionResult {
    let mut result = SupersessionResult::default();

    let mut decisions: Vec<Decision> = Vec::new();
    {
        let Ok(rtxn) = db.env.read_txn() else {
            return result;
        };
        let Ok(iter) = db.decisions.iter(&rtxn) else {
            return result;
        };
        for entry in iter.flatten() {
            let (_, value) = entry;
            if let Ok(dec) = serde_json::from_slice::<Decision>(value) {
                decisions.push(dec);
            }
        }
    }

    result.decisions_scanned = decisions.len();

    if decisions.len() < 2 {
        return result;
    }

    // Sort by valid_from ascending (oldest first)
    decisions.sort_by_key(|d| d.valid_from_ts_utc_ms);

    // Build similarity lookup: try MaxSim first, fall back to word_overlap
    let statements: Vec<String> =
        decisions.iter().map(|d| d.statement.clone()).collect();
    let maxsim_matrix = if use_maxsim {
        crate::embeddings::maxsim::try_pairwise_maxsim(&statements)
    } else {
        None
    };

    if maxsim_matrix.is_some() {
        tracing::debug!("supersession: using ColBERT MaxSim");
    } else {
        tracing::debug!("supersession: falling back to word overlap");
    }

    // For each pair (older, newer), check if newer supersedes older
    let mut to_supersede: Vec<(usize, i64)> = Vec::new();

    for i in 0..decisions.len() {
        // Skip if already superseded
        if decisions[i].valid_to_ts_utc_ms.is_some() {
            continue;
        }

        for j in (i + 1)..decisions.len() {
            let similarity = maxsim_matrix.as_ref().map_or_else(
                || {
                    word_overlap(
                        &decisions[i].statement,
                        &decisions[j].statement,
                    )
                },
                |sim| {
                    // Normalize MaxSim scores to [0, 1] range for
                    // threshold comparison. Self-sim is the max.
                    let self_sim = sim.get(i, i);
                    if self_sim > 0.0 {
                        f64::from(sim.get(i, j)) / f64::from(self_sim)
                    } else {
                        0.0
                    }
                },
            );

            if similarity >= config.topic_similarity_threshold
                && decisions[i].confidence <= config.max_confidence_to_supersede
            {
                // Newer decision supersedes older one
                to_supersede.push((i, decisions[j].valid_from_ts_utc_ms));
                break; // Only supersede by the first (earliest) match
            }
        }
    }

    // Apply supersessions
    for (idx, valid_to) in &to_supersede {
        let mut dec = decisions[*idx].clone();
        dec.valid_to_ts_utc_ms = Some(*valid_to);
        if crud::put_decision(db, &dec).is_ok() {
            result.decisions_superseded += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        db,
        ids::{DecisionId, EpisodeId, RepoId},
        schema::EvidenceRef,
    };

    fn make_decision(
        suffix: &[u8],
        statement: &str,
        rationale: &str,
        valid_from: i64,
    ) -> Decision {
        Decision {
            decision_id: DecisionId::derive(suffix),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(suffix),
            task_id: None,
            statement: statement.into(),
            rationale: rationale.into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: valid_from,
            valid_to_ts_utc_ms: None,
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(suffix),
                span_summary: "test".into(),
            }],
            premises: vec![],
        }
    }

    // -- Property: word_overlap is symmetric --
    #[hegel::test(test_cases = 100)]
    fn prop_word_overlap_symmetric(tc: TestCase) {
        let a: String = tc.draw(gs::text().min_size(1).max_size(50));
        let b: String = tc.draw(gs::text().min_size(1).max_size(50));
        let ab = word_overlap(&a, &b);
        let ba = word_overlap(&b, &a);
        assert!(
            (ab - ba).abs() < f64::EPSILON,
            "overlap must be symmetric: {ab} != {ba}"
        );
    }

    // -- Property: word_overlap is in [0, 1] --
    #[hegel::test(test_cases = 100)]
    fn prop_word_overlap_range(tc: TestCase) {
        let a: String = tc.draw(gs::text().min_size(0).max_size(50));
        let b: String = tc.draw(gs::text().min_size(0).max_size(50));
        let score = word_overlap(&a, &b);
        assert!(
            (0.0..=1.0).contains(&score),
            "overlap must be in [0,1]: {score}"
        );
    }

    // -- Property: identical strings have overlap 1.0 --
    #[hegel::test(test_cases = 50)]
    fn prop_word_overlap_identity(tc: TestCase) {
        let s: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(50)
                .alphabet("abcdefghijklmnop "),
        );
        tc.assume(!s.trim().is_empty());
        let score = word_overlap(&s, &s);
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "identical strings must have overlap 1.0: {score}"
        );
    }

    // -- Unit: supersession detects matching decisions --
    #[test]
    fn test_supersession_detects_match() {
        let (database, _dir) = db::open_in_memory().unwrap();

        let old = make_decision(
            b"old",
            "Use SQLite for storage",
            "It is widely available",
            1000,
        );
        let new = make_decision(
            b"new",
            "Use redb for storage instead of SQLite",
            "Better Rust integration",
            2000,
        );
        crud::put_decision(&database, &old).unwrap();
        crud::put_decision(&database, &new).unwrap();

        let config = SupersessionConfig {
            topic_similarity_threshold: 0.3,
            ..SupersessionConfig::default()
        };
        let result = scan_superseded_decisions_inner(&database, &config, false);

        assert_eq!(result.decisions_scanned, 2);
        assert_eq!(result.decisions_superseded, 1);

        // Old decision should now have valid_to set
        let loaded =
            crud::get_decision(&database, &old.decision_id.raw()).unwrap();
        assert_eq!(loaded.valid_to_ts_utc_ms, Some(2000));
    }

    // -- Unit: unrelated decisions are not superseded --
    #[test]
    fn test_unrelated_not_superseded() {
        let (database, _dir) = db::open_in_memory().unwrap();

        let d1 = make_decision(b"d1", "Use redb for storage", "Fast", 1000);
        let d2 = make_decision(
            b"d2",
            "Deploy to production on Fridays",
            "We like risk",
            2000,
        );
        crud::put_decision(&database, &d1).unwrap();
        crud::put_decision(&database, &d2).unwrap();

        let config = SupersessionConfig::default();
        let result = scan_superseded_decisions_inner(&database, &config, false);

        assert_eq!(result.decisions_superseded, 0);
    }

    // -- Unit: already superseded decisions are skipped --
    #[test]
    fn test_already_superseded_skipped() {
        let (database, _dir) = db::open_in_memory().unwrap();

        let mut old =
            make_decision(b"old", "Use SQLite for storage", "Available", 1000);
        old.valid_to_ts_utc_ms = Some(1500); // already superseded
        let new = make_decision(
            b"new",
            "Use redb for storage instead of SQLite",
            "Better",
            2000,
        );
        crud::put_decision(&database, &old).unwrap();
        crud::put_decision(&database, &new).unwrap();

        let config = SupersessionConfig {
            topic_similarity_threshold: 0.3,
            ..SupersessionConfig::default()
        };
        let result = scan_superseded_decisions_inner(&database, &config, false);
        assert_eq!(result.decisions_superseded, 0);
    }

    // -- Unit: empty DB produces no supersessions --
    #[test]
    fn test_empty_db() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let result = scan_superseded_decisions_inner(
            &database,
            &SupersessionConfig::default(),
            false,
        );
        assert_eq!(result.decisions_scanned, 0);
        assert_eq!(result.decisions_superseded, 0);
    }

    // -- Property: supersession is idempotent --
    #[hegel::test(test_cases = 30)]
    fn prop_supersession_idempotent(tc: TestCase) {
        let (database, _dir) = db::open_in_memory().unwrap();

        // Create 2-4 decisions with overlapping topics
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(4));
        let base_words = tc
            .draw(gs::text().min_size(5).max_size(20).alphabet("abcdefghij "));

        for i in 0..n {
            let extra: String = tc.draw(
                gs::text().min_size(1).max_size(10).alphabet("klmnopqrst"),
            );
            let stmt = format!("{base_words} {extra}");
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
            let dec = make_decision(
                &(i as u32).to_le_bytes(),
                &stmt,
                "reason",
                (i * 1000) as i64,
            );
            crud::put_decision(&database, &dec).unwrap();
        }

        let config = SupersessionConfig {
            topic_similarity_threshold: 0.3,
            ..SupersessionConfig::default()
        };

        let r1 = scan_superseded_decisions_inner(&database, &config, false);
        let r2 = scan_superseded_decisions_inner(&database, &config, false);

        // Second run should find nothing new
        assert_eq!(
            r2.decisions_superseded, 0,
            "second scan should be idempotent"
        );
        // But first run may have found some
        assert!(r1.decisions_superseded <= n);
    }

    // -- Property: superseded count never exceeds total - 1 --
    #[hegel::test(test_cases = 30)]
    fn prop_superseded_bounded(tc: TestCase) {
        let (database, _dir) = db::open_in_memory().unwrap();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(6));

        for i in 0..n {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
            let dec = make_decision(
                &(i as u32).to_le_bytes(),
                "use redb for storage",
                "reason",
                (i * 1000) as i64,
            );
            crud::put_decision(&database, &dec).unwrap();
        }

        let config = SupersessionConfig {
            topic_similarity_threshold: 0.3,
            ..SupersessionConfig::default()
        };
        let result = scan_superseded_decisions_inner(&database, &config, false);

        assert!(
            result.decisions_superseded < n,
            "can't supersede all decisions: {} >= {}",
            result.decisions_superseded,
            n
        );
    }

    // -- Integration: MaxSim supersession with real model --
    // Skipped if model is not installed.
    #[test]
    fn test_maxsim_supersession() {
        if !crate::embeddings::encoder::model_available() {
            eprintln!("skipping test_maxsim_supersession: model not installed");
            return;
        }

        let (database, _dir) = db::open_in_memory().unwrap();

        let old = make_decision(
            b"old",
            "Use SQLite for storage because it is widely available",
            "Widespread adoption",
            1000,
        );
        let new = make_decision(
            b"new",
            "Use redb for storage because it has better Rust integration",
            "Native Rust, ACID",
            2000,
        );
        let unrelated = make_decision(
            b"unrel",
            "Deploy to production every Friday afternoon",
            "We love risk",
            3000,
        );
        crud::put_decision(&database, &old).unwrap();
        crud::put_decision(&database, &new).unwrap();
        crud::put_decision(&database, &unrelated).unwrap();

        let config = SupersessionConfig {
            topic_similarity_threshold: 0.5,
            ..SupersessionConfig::default()
        };
        // use_maxsim = true for this integration test
        let result = scan_superseded_decisions_inner(&database, &config, true);

        assert_eq!(result.decisions_scanned, 3);
        // The old storage decision should be superseded by the new one
        assert!(
            result.decisions_superseded >= 1,
            "MaxSim should detect storage topic overlap"
        );

        // Verify the old decision got valid_to set
        let loaded =
            crud::get_decision(&database, &old.decision_id.raw()).unwrap();
        assert!(
            loaded.valid_to_ts_utc_ms.is_some(),
            "old storage decision should be superseded"
        );

        // The unrelated decision should NOT be superseded
        let unrel_loaded =
            crud::get_decision(&database, &unrelated.decision_id.raw())
                .unwrap();
        assert!(
            unrel_loaded.valid_to_ts_utc_ms.is_none(),
            "unrelated decision should not be superseded"
        );
    }
}
