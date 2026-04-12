//! Proxy vector reduction and pooling.
//!
//! Mean-pools per-token `ColBERT` embeddings into a single proxy
//! vector. The reduction function is versioned and frozen for
//! determinism.

/// Mean-pool a set of token embedding vectors into a single
/// proxy vector.
///
/// Input: a flat slice of f32 values representing N tokens of
/// D dimensions (total length = N * D). Padding tokens (all
/// zeros) are excluded from the mean.
///
/// Output: a Vec<f32> of length D.
///
/// This is the "frozen" reduction rule from the architecture:
/// the exact same input must always produce the exact same
/// output, per Lobster release.
#[must_use]
pub fn mean_pool(token_embeddings: &[f32], dimensions: usize) -> Vec<f32> {
    if dimensions == 0 || token_embeddings.is_empty() {
        return vec![0.0; dimensions];
    }

    let n_tokens = token_embeddings.len() / dimensions;
    if n_tokens == 0 {
        return vec![0.0; dimensions];
    }

    let mut sum = vec![0.0_f64; dimensions];
    let mut non_padding_count = 0u64;

    for token_idx in 0..n_tokens {
        let start = token_idx * dimensions;
        let end = start + dimensions;
        let token = &token_embeddings[start..end];

        // Skip padding tokens (all zeros)
        let is_padding = token.iter().all(|&v| v == 0.0);
        if is_padding {
            continue;
        }

        non_padding_count += 1;
        for (i, &v) in token.iter().enumerate() {
            sum[i] += f64::from(v);
        }
    }

    if non_padding_count == 0 {
        return vec![0.0; dimensions];
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let count = non_padding_count as f64;
    #[allow(clippy::cast_possible_truncation)]
    let result: Vec<f32> = sum.iter().map(|&s| (s / count) as f32).collect();
    result
}

/// Serialize a proxy vector to bytes for storage.
#[must_use]
pub fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Deserialize a proxy vector from bytes.
#[must_use]
pub fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        })
        .collect()
}

/// Artifact-specific pooling policy.
///
/// Different artifact classes get different pooling levels:
/// - Decisions: full (no pooling) — high-value, short text
/// - Active task summaries: light (`pool_factor=2`)
/// - Durable constraints: full (no pooling)
/// - Episode summaries (recent): light (`pool_factor=2`)
/// - Episode summaries (old): heavy (proxy only)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolingPolicy {
    /// Full late-interaction: keep all token embeddings.
    Full,
    /// Light pooling: reduce tokens by `pool_factor`.
    Light,
    /// Heavy: proxy vector only, no late-interaction bytes.
    ProxyOnly,
}

/// Determine the pooling policy for an artifact class.
/// Determine the pooling policy for an artifact class.
///
/// Per spec table:
/// - Decisions: full (no pooling)
/// - Active task summaries: light (`pool_factor=2`)
/// - Durable constraints: full (no pooling)
/// - Episode summaries (recent): light (`pool_factor=2`)
/// - Episode summaries (old): proxy only
#[must_use]
#[allow(clippy::match_same_arms)]
pub fn policy_for(artifact_class: &str) -> PoolingPolicy {
    match artifact_class {
        "decision" | "durable_constraint" | "constraint" => PoolingPolicy::Full,
        "task" | "summary_recent" | "summary" => PoolingPolicy::Light,
        "summary_old" | "component" => PoolingPolicy::ProxyOnly,
        _ => PoolingPolicy::ProxyOnly,
    }
}

/// Proxy vector reduction version. Changes to the reduction
/// algorithm require bumping this.
pub const PROXY_REDUCTION_VERSION: &str = "mean-pool-v1";

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // ── Property: mean_pool is deterministic ─────────────
    #[hegel::test(test_cases = 200)]
    fn prop_mean_pool_deterministic(tc: TestCase) {
        let dims: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(16));
        let n_tokens: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(8));
        let data: Vec<f32> = (0..dims * n_tokens)
            .map(|_| {
                tc.draw(
                    gs::floats::<f32>()
                        .min_value(-1.0)
                        .max_value(1.0)
                        .allow_nan(false)
                        .allow_infinity(false),
                )
            })
            .collect();

        let v1 = mean_pool(&data, dims);
        let v2 = mean_pool(&data, dims);
        assert_eq!(v1, v2, "mean_pool must be deterministic");
    }

    // ── Property: output dimension matches input dimension ──
    #[hegel::test(test_cases = 200)]
    fn prop_output_dimension_correct(tc: TestCase) {
        let dims: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(32));
        let n_tokens: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(8));
        let data: Vec<f32> = vec![1.0; dims * n_tokens];

        let result = mean_pool(&data, dims);
        assert_eq!(result.len(), dims, "output must have {dims} dimensions");
    }

    // ── Property: vector bytes round-trip ─────────────────
    #[hegel::test(test_cases = 200)]
    fn prop_vector_bytes_roundtrip(tc: TestCase) {
        let len: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(64));
        let vector: Vec<f32> = (0..len)
            .map(|_| {
                tc.draw(
                    gs::floats::<f32>()
                        .min_value(-100.0)
                        .max_value(100.0)
                        .allow_nan(false)
                        .allow_infinity(false),
                )
            })
            .collect();

        let bytes = vector_to_bytes(&vector);
        let recovered = bytes_to_vector(&bytes);
        assert_eq!(vector, recovered);
    }

    // ── Unit tests ───────────────────────────────────────
    #[test]
    fn test_mean_pool_single_token() {
        let data = vec![1.0, 2.0, 3.0];
        let result = mean_pool(&data, 3);
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_mean_pool_two_tokens() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        let result = mean_pool(&data, 2);
        // Token 1: [1.0, 2.0], Token 2: [3.0, 4.0]
        // Mean: [2.0, 3.0]
        assert_eq!(result, vec![2.0, 3.0]);
    }

    #[test]
    fn test_mean_pool_skips_padding() {
        // 3 tokens of 2 dims, middle one is padding
        let data = vec![1.0, 2.0, 0.0, 0.0, 3.0, 4.0];
        let result = mean_pool(&data, 2);
        // Only tokens [1,2] and [3,4] counted
        assert_eq!(result, vec![2.0, 3.0]);
    }

    #[test]
    fn test_mean_pool_all_padding() {
        let data = vec![0.0, 0.0, 0.0, 0.0];
        let result = mean_pool(&data, 2);
        assert_eq!(result, vec![0.0, 0.0]);
    }

    #[test]
    fn test_mean_pool_empty() {
        let result = mean_pool(&[], 4);
        assert_eq!(result, vec![0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_pooling_policy() {
        assert_eq!(policy_for("decision"), PoolingPolicy::Full);
        assert_eq!(policy_for("durable_constraint"), PoolingPolicy::Full);
        assert_eq!(policy_for("constraint"), PoolingPolicy::Full);
        assert_eq!(policy_for("task"), PoolingPolicy::Light);
        assert_eq!(policy_for("summary"), PoolingPolicy::Light);
        assert_eq!(policy_for("summary_recent"), PoolingPolicy::Light);
        assert_eq!(policy_for("summary_old"), PoolingPolicy::ProxyOnly);
    }
}
