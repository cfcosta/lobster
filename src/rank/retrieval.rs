//! Retrieval execution: MMR diversity, ready-set intersection,
//! and cosine reranking.

use redb::Database;

use crate::store::{crud, ids::RawId, schema::ProcessingState};

/// Check if an episode is in the Ready state.
///
/// Returns `true` only if the episode exists and has
/// `ProcessingState::Ready`. All retrieval results must pass
/// this check before being surfaced.
#[must_use]
pub fn is_ready(db: &Database, episode_id: &RawId) -> bool {
    crud::get_episode(db, episode_id)
        .is_ok_and(|ep| ep.processing_state == ProcessingState::Ready)
}

/// Filter a list of candidate episode IDs to only those in Ready
/// state.
#[must_use]
pub fn intersect_ready_set(db: &Database, candidates: &[RawId]) -> Vec<RawId> {
    candidates
        .iter()
        .filter(|id| is_ready(db, id))
        .copied()
        .collect()
}

/// A scored candidate for retrieval.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub id: RawId,
    pub score: f64,
    pub artifact_type: String,
}

/// Apply Maximal Marginal Relevance to deduplicate results.
///
/// `lambda` controls the diversity/relevance trade-off:
/// - 1.0 = pure relevance (no diversity penalty)
/// - 0.0 = pure diversity
///
/// `similarity_fn` computes pairwise similarity between two
/// candidates (0.0 to 1.0).
#[must_use]
pub fn apply_mmr(
    candidates: &[ScoredCandidate],
    max_results: usize,
    lambda: f64,
    similarity_fn: impl Fn(&RawId, &RawId) -> f64,
) -> Vec<ScoredCandidate> {
    if candidates.is_empty() || max_results == 0 {
        return vec![];
    }

    let mut selected: Vec<ScoredCandidate> = Vec::new();
    let mut remaining: Vec<usize> = (0..candidates.len()).collect();

    while selected.len() < max_results && !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_mmr = f64::NEG_INFINITY;

        for (ri, &ci) in remaining.iter().enumerate() {
            let candidate = &candidates[ci];
            let relevance = candidate.score;

            let max_sim = if selected.is_empty() {
                0.0
            } else {
                selected
                    .iter()
                    .map(|s| similarity_fn(&candidate.id, &s.id))
                    .fold(0.0_f64, f64::max)
            };

            let mmr = (1.0 - lambda).mul_add(-max_sim, lambda * relevance);

            if mmr > best_mmr {
                best_mmr = mmr;
                best_idx = ri;
            }
        }

        let chosen = remaining.remove(best_idx);
        selected.push(candidates[chosen].clone());
    }

    selected
}

/// Cosine similarity between two vectors.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot = x.mul_add(y, dot);
        norm_a = x.mul_add(x, norm_a);
        norm_b = y.mul_add(y, norm_b);
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{db, ids::EpisodeId, schema::Episode};

    #[test]
    fn test_ready_set_filters_non_ready() {
        let database = db::open_in_memory().unwrap();

        // Create a Ready episode and a Pending episode
        let ready_ep = Episode {
            episode_id: EpisodeId::derive(b"ready"),
            repo_id: crate::store::ids::RepoId::derive(b"r"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1_000,
            retry_count: 0,
        };
        let pending_ep = Episode {
            episode_id: EpisodeId::derive(b"pending"),
            repo_id: crate::store::ids::RepoId::derive(b"r"),
            start_seq: 6,
            end_seq: 10,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: 2_000,
            retry_count: 0,
        };

        crud::put_episode(&database, &ready_ep).unwrap();
        crud::put_episode(&database, &pending_ep).unwrap();

        let candidates = vec![
            ready_ep.episode_id.raw(),
            pending_ep.episode_id.raw(),
            EpisodeId::derive(b"nonexistent").raw(),
        ];

        let result = intersect_ready_set(&database, &candidates);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ready_ep.episode_id.raw());
    }

    #[test]
    fn test_mmr_selects_top_k() {
        let candidates = vec![
            ScoredCandidate {
                id: EpisodeId::derive(b"a").raw(),
                score: 0.9,
                artifact_type: "decision".into(),
            },
            ScoredCandidate {
                id: EpisodeId::derive(b"b").raw(),
                score: 0.8,
                artifact_type: "summary".into(),
            },
            ScoredCandidate {
                id: EpisodeId::derive(b"c").raw(),
                score: 0.7,
                artifact_type: "entity".into(),
            },
        ];

        // With lambda=1.0, no diversity penalty
        let result = apply_mmr(&candidates, 2, 1.0, |_, _| 0.0);
        assert_eq!(result.len(), 2);
        assert!(result[0].score >= result[1].score);
    }

    #[test]
    fn test_mmr_penalizes_similar() {
        let a_id = EpisodeId::derive(b"a").raw();
        let b_id = EpisodeId::derive(b"b").raw();
        let c_id = EpisodeId::derive(b"c").raw();

        let candidates = vec![
            ScoredCandidate {
                id: a_id,
                score: 0.9,
                artifact_type: "decision".into(),
            },
            ScoredCandidate {
                id: b_id,
                score: 0.85,
                artifact_type: "decision".into(),
            },
            ScoredCandidate {
                id: c_id,
                score: 0.7,
                artifact_type: "summary".into(),
            },
        ];

        // a and b are very similar, c is different
        let sim = |x: &RawId, y: &RawId| {
            if (*x == a_id && *y == b_id) || (*x == b_id && *y == a_id) {
                0.95 // very similar
            } else {
                0.1 // not similar
            }
        };

        // With diversity (lambda=0.5), should prefer c over b
        let result = apply_mmr(&candidates, 2, 0.5, sim);
        assert_eq!(result.len(), 2);
        // First should be a (highest score)
        assert_eq!(result[0].id, a_id);
        // Second should be c (diverse) not b (similar to a)
        assert_eq!(result[1].id, c_id);
    }

    // -- Property: cosine similarity is in [-1, 1] --
    #[hegel::test(test_cases = 200)]
    #[allow(clippy::needless_pass_by_ref_mut)]
    fn prop_cosine_in_range(tc: TestCase) {
        let len: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(64));
        let a: Vec<f32> = (0..len)
            .map(|_| {
                tc.draw(
                    gs::floats::<f32>()
                        .min_value(-10.0)
                        .max_value(10.0)
                        .allow_nan(false)
                        .allow_infinity(false),
                )
            })
            .collect();
        let b: Vec<f32> = (0..len)
            .map(|_| {
                tc.draw(
                    gs::floats::<f32>()
                        .min_value(-10.0)
                        .max_value(10.0)
                        .allow_nan(false)
                        .allow_infinity(false),
                )
            })
            .collect();

        let sim = cosine_similarity(&a, &b);
        assert!(
            (-1.01..=1.01).contains(&sim),
            "cosine sim {sim} out of range"
        );
    }

    // -- Property: cosine self-similarity is 1.0 --
    #[hegel::test(test_cases = 100)]
    #[allow(clippy::needless_pass_by_ref_mut)]
    fn prop_cosine_self_similarity(tc: TestCase) {
        let len: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(32));
        let v: Vec<f32> = (0..len)
            .map(|_| {
                tc.draw(
                    gs::floats::<f32>()
                        .min_value(0.1)
                        .max_value(10.0)
                        .allow_nan(false)
                        .allow_infinity(false),
                )
            })
            .collect();

        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "self-similarity should be ~1.0, got {sim}"
        );
    }

    #[test]
    fn test_cosine_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_mmr_empty_input() {
        let result: Vec<ScoredCandidate> = apply_mmr(&[], 5, 0.7, |_, _| 0.0);
        assert!(result.is_empty());
    }
}
