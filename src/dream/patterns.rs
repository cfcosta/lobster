//! N-gram pattern detection across tool-use sequences.
//!
//! Given a collection of `EventKind` sequences (one per episode),
//! detects recurring subsequences (n-grams) that appear in at least
//! `min_frequency` episodes. These are workflow candidates.

use std::collections::HashMap;

use crate::store::schema::EventKind;

/// A detected recurring pattern with its frequency and source indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPattern {
    /// The recurring subsequence of event kinds.
    pub pattern: Vec<EventKind>,
    /// Number of sequences (episodes) containing this pattern.
    pub frequency: usize,
    /// Indices into the input sequences that contain the pattern.
    pub source_indices: Vec<usize>,
}

/// Configuration for pattern detection.
#[derive(Debug, Clone)]
pub struct PatternConfig {
    /// Minimum n-gram length (inclusive).
    pub min_len: usize,
    /// Maximum n-gram length (inclusive).
    pub max_len: usize,
    /// Minimum number of sequences a pattern must appear in.
    pub min_frequency: usize,
}

impl Default for PatternConfig {
    fn default() -> Self {
        Self {
            min_len: 2,
            max_len: 8,
            min_frequency: 2,
        }
    }
}

/// Detect recurring n-gram patterns across multiple event sequences.
///
/// Returns patterns sorted by frequency (highest first), then by
/// pattern length (longest first) for deterministic output.
#[must_use]
pub fn detect_patterns(
    sequences: &[Vec<EventKind>],
    config: &PatternConfig,
) -> Vec<DetectedPattern> {
    if sequences.len() < config.min_frequency || config.min_len > config.max_len
    {
        return vec![];
    }

    // For each n-gram, track which sequence indices contain it.
    // Key: the n-gram pattern, Value: set of sequence indices.
    let mut ngram_sources: HashMap<Vec<EventKind>, Vec<usize>> = HashMap::new();

    for (seq_idx, sequence) in sequences.iter().enumerate() {
        // Track which n-grams we've already seen in this sequence
        // to avoid counting duplicates within the same episode.
        let mut seen_in_this_seq: std::collections::HashSet<Vec<EventKind>> =
            std::collections::HashSet::new();

        for n in config.min_len..=config.max_len {
            if sequence.len() < n {
                continue;
            }
            for window in sequence.windows(n) {
                let ngram = window.to_vec();
                if seen_in_this_seq.insert(ngram.clone()) {
                    ngram_sources.entry(ngram).or_default().push(seq_idx);
                }
            }
        }
    }

    // Filter by min_frequency and collect results
    let mut results: Vec<DetectedPattern> = ngram_sources
        .into_iter()
        .filter(|(_, sources)| sources.len() >= config.min_frequency)
        .map(|(pattern, source_indices)| DetectedPattern {
            frequency: source_indices.len(),
            pattern,
            source_indices,
        })
        .collect();

    // Remove patterns that are strict sub-patterns of a longer
    // pattern with the same frequency (prefer the longest form).
    results = remove_subsumed(results);

    // Sort: highest frequency first, then longest pattern first,
    // then lexicographic on pattern for determinism.
    results.sort_by(|a, b| {
        b.frequency
            .cmp(&a.frequency)
            .then_with(|| b.pattern.len().cmp(&a.pattern.len()))
            .then_with(|| {
                format!("{:?}", a.pattern).cmp(&format!("{:?}", b.pattern))
            })
    });

    results
}

/// Remove patterns that are strict subsequences of a longer pattern
/// with equal or greater frequency.
fn remove_subsumed(mut patterns: Vec<DetectedPattern>) -> Vec<DetectedPattern> {
    // Sort longest first so we check superseding patterns first
    patterns.sort_by_key(|p| std::cmp::Reverse(p.pattern.len()));

    let mut kept = Vec::new();
    for candidate in &patterns {
        let subsumed = kept.iter().any(|longer: &DetectedPattern| {
            longer.pattern.len() > candidate.pattern.len()
                && longer.frequency >= candidate.frequency
                && is_subsequence(&candidate.pattern, &longer.pattern)
        });
        if !subsumed {
            kept.push(candidate.clone());
        }
    }
    kept
}

/// Check if `short` appears as a contiguous subsequence of `long`.
fn is_subsequence(short: &[EventKind], long: &[EventKind]) -> bool {
    if short.len() > long.len() {
        return false;
    }
    long.windows(short.len()).any(|w| w == short)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn all_event_kinds() -> Vec<EventKind> {
        vec![
            EventKind::UserPromptSubmit,
            EventKind::AssistantResponse,
            EventKind::ToolUse,
            EventKind::ToolResult,
            EventKind::ToolUseFailure,
            EventKind::FileRead,
            EventKind::FileWrite,
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
            EventKind::PlanTransition,
        ]
    }

    #[hegel::composite]
    fn gen_event_kind(tc: hegel::TestCase) -> EventKind {
        tc.draw(gs::sampled_from(all_event_kinds()))
    }

    #[hegel::composite]
    fn gen_sequence(tc: hegel::TestCase) -> Vec<EventKind> {
        let len: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(15));
        let mut seq = Vec::with_capacity(len);
        for _ in 0..len {
            seq.push(tc.draw(gen_event_kind()));
        }
        seq
    }

    // -- Property: every returned pattern appears in >= min_frequency sequences --
    #[hegel::test(test_cases = 100)]
    fn prop_frequency_guarantee(tc: TestCase) {
        let n_seqs: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
        let mut sequences = Vec::with_capacity(n_seqs);
        for _ in 0..n_seqs {
            sequences.push(tc.draw(gen_sequence()));
        }

        let config = PatternConfig::default();
        let results = detect_patterns(&sequences, &config);

        for pattern in &results {
            assert!(
                pattern.frequency >= config.min_frequency,
                "pattern {:?} has frequency {} < min {}",
                pattern.pattern,
                pattern.frequency,
                config.min_frequency
            );
        }
    }

    // -- Property: every returned pattern actually occurs in its source sequences --
    #[hegel::test(test_cases = 100)]
    fn prop_pattern_actually_occurs(tc: TestCase) {
        let n_seqs: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
        let mut sequences = Vec::with_capacity(n_seqs);
        for _ in 0..n_seqs {
            sequences.push(tc.draw(gen_sequence()));
        }

        let config = PatternConfig::default();
        let results = detect_patterns(&sequences, &config);

        for detected in &results {
            for &idx in &detected.source_indices {
                assert!(idx < sequences.len(), "source index out of bounds");
                assert!(
                    is_subsequence(&detected.pattern, &sequences[idx]),
                    "pattern {:?} not found in sequence {}",
                    detected.pattern,
                    idx
                );
            }
        }
    }

    // -- Property: pattern length is within configured bounds --
    #[hegel::test(test_cases = 100)]
    fn prop_pattern_length_bounded(tc: TestCase) {
        let n_seqs: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
        let mut sequences = Vec::with_capacity(n_seqs);
        for _ in 0..n_seqs {
            sequences.push(tc.draw(gen_sequence()));
        }

        let min_len: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(4));
        let max_len: usize =
            tc.draw(gs::integers::<usize>().min_value(min_len).max_value(8));
        let config = PatternConfig {
            min_len,
            max_len,
            min_frequency: 2,
        };

        let results = detect_patterns(&sequences, &config);
        for pattern in &results {
            assert!(
                pattern.pattern.len() >= min_len,
                "pattern too short: {} < {}",
                pattern.pattern.len(),
                min_len
            );
            assert!(
                pattern.pattern.len() <= max_len,
                "pattern too long: {} > {}",
                pattern.pattern.len(),
                max_len
            );
        }
    }

    // -- Property: deterministic output for same input --
    #[hegel::test(test_cases = 50)]
    fn prop_deterministic(tc: TestCase) {
        let n_seqs: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
        let mut sequences = Vec::with_capacity(n_seqs);
        for _ in 0..n_seqs {
            sequences.push(tc.draw(gen_sequence()));
        }

        let config = PatternConfig::default();
        let r1 = detect_patterns(&sequences, &config);
        let r2 = detect_patterns(&sequences, &config);
        assert_eq!(r1, r2, "same input must produce same output");
    }

    // -- Property: no results when fewer sequences than min_frequency --
    #[hegel::test(test_cases = 50)]
    fn prop_empty_below_min_frequency(tc: TestCase) {
        let seq = tc.draw(gen_sequence());
        let config = PatternConfig {
            min_frequency: 2,
            ..PatternConfig::default()
        };
        let results = detect_patterns(&[seq], &config);
        assert!(
            results.is_empty(),
            "single sequence cannot meet min_frequency=2"
        );
    }

    // -- Unit: known pattern is detected --
    #[test]
    fn test_known_pattern_detected() {
        let pattern = vec![
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];
        let seq1 = vec![
            EventKind::UserPromptSubmit,
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
            EventKind::AssistantResponse,
        ];
        let seq2 = vec![
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];

        let config = PatternConfig::default();
        let results = detect_patterns(&[seq1, seq2], &config);

        let found = results
            .iter()
            .any(|r| r.pattern == pattern && r.frequency >= 2);
        assert!(found, "expected pattern not detected: {results:?}");
    }

    // -- Unit: is_subsequence works --
    #[test]
    fn test_is_subsequence() {
        let short = vec![EventKind::FileEdit, EventKind::TestRun];
        let long = vec![
            EventKind::ToolUse,
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];
        assert!(is_subsequence(&short, &long));
        assert!(!is_subsequence(&long, &short));
        assert!(is_subsequence(&short, &short));
    }

    // -- Property: source_indices length matches frequency --
    #[hegel::test(test_cases = 100)]
    fn prop_source_indices_match_frequency(tc: TestCase) {
        let n_seqs: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
        let mut sequences = Vec::with_capacity(n_seqs);
        for _ in 0..n_seqs {
            sequences.push(tc.draw(gen_sequence()));
        }

        let config = PatternConfig::default();
        let results = detect_patterns(&sequences, &config);

        for pattern in &results {
            assert_eq!(
                pattern.source_indices.len(),
                pattern.frequency,
                "source_indices length must equal frequency"
            );
        }
    }

    // -- Property: empty sequences produce no patterns --
    #[test]
    fn test_empty_input() {
        let results = detect_patterns(&[], &PatternConfig::default());
        assert!(results.is_empty());
    }
}
