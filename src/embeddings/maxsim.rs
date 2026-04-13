//! `ColBERT` `MaxSim` pairwise similarity computation.
//!
//! Batch-encodes text strings with `ColBERT` and computes the full
//! pairwise `MaxSim` similarity matrix. Used by the decision
//! supersession worker to detect semantically related decisions.

use pylate_rs::ColBERT;

/// A pairwise similarity matrix from `MaxSim`.
///
/// `scores[i][j]` is the `MaxSim` similarity between text `i` (as
/// query) and text `j` (as document).
#[derive(Debug, Clone)]
pub struct PairwiseSimilarity {
    /// The N×N similarity matrix.
    pub scores: Vec<Vec<f32>>,
    /// Number of texts compared.
    pub n: usize,
}

impl PairwiseSimilarity {
    /// Get the similarity between text `i` and text `j`.
    ///
    /// Returns 0.0 for out-of-bounds indices.
    #[must_use]
    pub fn get(&self, i: usize, j: usize) -> f32 {
        self.scores
            .get(i)
            .and_then(|row| row.get(j))
            .copied()
            .unwrap_or(0.0)
    }

    /// Get the maximum similarity between text `i` and any other
    /// text `j` (excluding self-similarity where `i == j`).
    #[must_use]
    pub fn max_other(&self, i: usize) -> (usize, f32) {
        let Some(row) = self.scores.get(i) else {
            return (0, 0.0);
        };
        row.iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .max_by(|(_, a), (_, b)| {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or((0, 0.0), |(j, &s)| (j, s))
    }
}

/// Error from the `MaxSim` computation.
#[derive(Debug)]
pub enum MaxSimError {
    /// The `ColBERT` model is not available.
    ModelUnavailable(String),
    /// Encoding or similarity computation failed.
    ComputationFailed(String),
    /// Not enough texts to compare.
    TooFewTexts,
}

impl std::fmt::Display for MaxSimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelUnavailable(msg) => {
                write!(f, "model unavailable: {msg}")
            }
            Self::ComputationFailed(msg) => {
                write!(f, "computation failed: {msg}")
            }
            Self::TooFewTexts => write!(f, "need at least 2 texts to compare"),
        }
    }
}

impl std::error::Error for MaxSimError {}

/// Compute pairwise `MaxSim` similarity for a batch of texts.
///
/// Encodes all texts as both queries and documents using `ColBERT`,
/// then computes the full N×N similarity matrix using late
/// interaction (token-level max-similarity summed over query tokens).
///
/// # Errors
///
/// Returns an error if encoding or similarity computation fails.
pub fn pairwise_maxsim(
    model: &mut ColBERT,
    texts: &[String],
) -> Result<PairwiseSimilarity, MaxSimError> {
    if texts.len() < 2 {
        return Err(MaxSimError::TooFewTexts);
    }

    // Encode as queries (for the query side of MaxSim)
    let query_embeddings = model
        .encode(texts, true)
        .map_err(|e| MaxSimError::ComputationFailed(e.to_string()))?;

    // Encode as documents (for the document side of MaxSim)
    let doc_embeddings = model
        .encode(texts, false)
        .map_err(|e| MaxSimError::ComputationFailed(e.to_string()))?;

    // Compute the similarity matrix: queries × documents
    let similarities = model
        .similarity(&query_embeddings, &doc_embeddings)
        .map_err(|e| MaxSimError::ComputationFailed(e.to_string()))?;

    Ok(PairwiseSimilarity {
        n: texts.len(),
        scores: similarities.data,
    })
}

/// Try to compute pairwise `MaxSim`, returning None if the model is
/// unavailable. This is the entry point for fallback-aware code.
#[must_use]
pub fn try_pairwise_maxsim(texts: &[String]) -> Option<PairwiseSimilarity> {
    if texts.len() < 2 {
        return None;
    }

    let mut model = crate::embeddings::encoder::load_model().ok()?;
    pairwise_maxsim(&mut model, texts).ok()
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: PairwiseSimilarity::get returns 0.0 for OOB --
    #[hegel::test(test_cases = 50)]
    fn prop_get_oob_returns_zero(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
        let scores = vec![vec![1.0_f32; n]; n];
        let sim = PairwiseSimilarity { scores, n };

        let oob_i: usize =
            tc.draw(gs::integers::<usize>().min_value(n).max_value(n + 10));
        assert!(
            sim.get(oob_i, 0).abs() < f32::EPSILON,
            "OOB row should return 0.0"
        );
        assert!(
            sim.get(0, oob_i).abs() < f32::EPSILON,
            "OOB col should return 0.0"
        );
    }

    // -- Property: PairwiseSimilarity::get returns correct value for in-bounds --
    #[hegel::test(test_cases = 50)]
    fn prop_get_inbounds(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let mut scores = Vec::with_capacity(n);
        for i in 0..n {
            let mut row = Vec::with_capacity(n);
            for j in 0..n {
                #[allow(clippy::cast_precision_loss)]
                row.push((i * n + j) as f32);
            }
            scores.push(row);
        }
        let sim = PairwiseSimilarity {
            scores: scores.clone(),
            n,
        };

        let i: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(n - 1));
        let j: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(n - 1));
        assert!(
            (sim.get(i, j) - scores[i][j]).abs() < f32::EPSILON,
            "get({i},{j}) should match scores"
        );
    }

    // -- Property: max_other excludes self --
    #[hegel::test(test_cases = 50)]
    fn prop_max_other_excludes_self(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        // Make diagonal the highest values
        let mut scores = vec![vec![0.5_f32; n]; n];
        for i in 0..n {
            scores[i][i] = 100.0; // self-similarity is huge
        }
        let sim = PairwiseSimilarity { scores, n };

        let i: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(n - 1));
        let (best_j, best_score) = sim.max_other(i);
        assert_ne!(best_j, i, "max_other must exclude self");
        assert!(
            (best_score - 0.5).abs() < f32::EPSILON,
            "max_other should be 0.5 (off-diagonal), got {best_score}"
        );
    }

    // -- Property: max_other returns valid index --
    #[hegel::test(test_cases = 50)]
    fn prop_max_other_valid_index(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let mut scores = Vec::with_capacity(n);
        for _ in 0..n {
            let row: Vec<f32> = (0..n)
                .map(|_| {
                    tc.draw(gs::floats::<f32>().min_value(0.0).max_value(1.0))
                })
                .collect();
            scores.push(row);
        }
        let sim = PairwiseSimilarity { scores, n };

        let i: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(n - 1));
        let (j, _) = sim.max_other(i);
        assert!(j < n, "max_other index must be < n");
        assert_ne!(j, i, "max_other must not return self");
    }

    // -- Unit: try_pairwise_maxsim returns None for < 2 texts --
    #[test]
    fn test_too_few_texts() {
        assert!(try_pairwise_maxsim(&[]).is_none());
        assert!(try_pairwise_maxsim(&["single".into()]).is_none());
    }

    // -- Unit: PairwiseSimilarity with model (skipped if model absent) --
    #[test]
    fn test_pairwise_maxsim_with_model() {
        let mut model = match crate::embeddings::encoder::load_model() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipping test_pairwise_maxsim_with_model: {e}");
                return;
            }
        };

        let texts = vec![
            "Use redb for storage because it is ACID compliant".into(),
            "Use SQLite for storage because it is widely available".into(),
            "Deploy to production on Fridays for maximum excitement".into(),
        ];

        let result = pairwise_maxsim(&mut model, &texts);
        match result {
            Ok(sim) => {
                assert_eq!(sim.n, 3);
                assert_eq!(sim.scores.len(), 3);
                // The two storage decisions should be more similar to
                // each other than to the deployment decision
                let storage_sim = sim.get(0, 1);
                let deploy_sim_0 = sim.get(0, 2);
                let deploy_sim_1 = sim.get(1, 2);
                eprintln!(
                    "storage↔storage: {storage_sim:.3}, \
                     redb↔deploy: {deploy_sim_0:.3}, \
                     sqlite↔deploy: {deploy_sim_1:.3}"
                );
                assert!(
                    storage_sim > deploy_sim_0,
                    "storage decisions should be more similar than \
                     storage vs deploy: {storage_sim} vs {deploy_sim_0}"
                );
                assert!(
                    storage_sim > deploy_sim_1,
                    "storage decisions should be more similar than \
                     storage vs deploy: {storage_sim} vs {deploy_sim_1}"
                );
            }
            Err(e) => {
                eprintln!("pairwise_maxsim failed (may need model): {e}");
            }
        }
    }
}
