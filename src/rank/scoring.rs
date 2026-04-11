//! Composite retrieval scoring and normalization.
//!
//! The scoring formula combines multiple signals into a single
//! normalized score. Weights are versioned constants; changes
//! require a version bump.

use crate::rank::route::RetrievalRoute;

/// Scoring weight configuration. Versioned and fixture-tested.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoringWeights {
    pub semantic: f64,
    pub recency: f64,
    pub task_overlap: f64,
    pub graph_support: f64,
    pub decision_bonus: f64,
    pub noise_penalty: f64,
}

/// Default weights from the retrieval routing spec.
pub const DEFAULT_WEIGHTS: ScoringWeights = ScoringWeights {
    semantic: 0.40,
    recency: 0.20,
    task_overlap: 0.15,
    graph_support: 0.10,
    decision_bonus: 0.10,
    noise_penalty: 0.05,
};

/// Input signals for composite scoring.
#[derive(Debug, Clone, Copy)]
pub struct ScoringInput {
    /// Normalized semantic similarity (0.0–1.0).
    pub semantic: f64,
    /// Recency score with exponential decay (0.0–1.0).
    pub recency: f64,
    /// 1.0 if artifact shares `task_id` with current context.
    pub task_overlap: f64,
    /// Fraction of graph neighbors also in candidate set (0.0–1.0).
    /// Only meaningful for `HybridGraph` route.
    pub graph_support: f64,
    /// True if this is a decision artifact.
    pub is_decision: bool,
    /// True if flagged as low-signal during dreaming.
    pub is_noisy: bool,
}

/// Compute the composite score.
#[must_use]
#[allow(clippy::suboptimal_flops)]
pub fn composite_score(
    input: &ScoringInput,
    weights: &ScoringWeights,
    route: RetrievalRoute,
) -> f64 {
    let decision_bonus = if input.is_decision {
        weights.decision_bonus
    } else {
        0.0
    };
    let noise_penalty = if input.is_noisy {
        weights.noise_penalty
    } else {
        0.0
    };
    let graph = if route == RetrievalRoute::HybridGraph {
        weights.graph_support * input.graph_support
    } else {
        0.0
    };

    weights.semantic * input.semantic
        + weights.recency * input.recency
        + weights.task_overlap * input.task_overlap
        + graph
        + decision_bonus
        - noise_penalty
}

/// Normalize a composite score to [0.0, 1.0].
///
/// Divides by the maximum achievable score (all positive components
/// at maximum + decision bonus, no noise penalty).
#[must_use]
pub fn normalize_score(
    raw: f64,
    weights: &ScoringWeights,
    route: RetrievalRoute,
) -> f64 {
    let max = weights.semantic
        + weights.recency
        + weights.task_overlap
        + if route == RetrievalRoute::HybridGraph {
            weights.graph_support
        } else {
            0.0
        }
        + weights.decision_bonus;

    if max <= 0.0 {
        return 0.0;
    }
    (raw / max).clamp(0.0, 1.0)
}

/// Per-route confidence thresholds for automatic recall.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn auto_threshold(route: RetrievalRoute) -> f64 {
    match route {
        RetrievalRoute::Exact => 1.0,
        RetrievalRoute::Hybrid => 0.4,
        RetrievalRoute::HybridGraph => 0.35,
        RetrievalRoute::Abstain => 1.0,
    }
}

/// Per-route confidence thresholds for explicit MCP recall.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn mcp_threshold(route: RetrievalRoute) -> f64 {
    match route {
        RetrievalRoute::Exact => 1.0,
        RetrievalRoute::Hybrid => 0.25,
        RetrievalRoute::HybridGraph => 0.20,
        RetrievalRoute::Abstain => 1.0,
    }
}

/// Per-route MMR diversity lambda.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn mmr_lambda(route: RetrievalRoute) -> f64 {
    match route {
        RetrievalRoute::Exact => 1.0,
        RetrievalRoute::Hybrid => 0.7,
        RetrievalRoute::HybridGraph => 0.6,
        RetrievalRoute::Abstain => 1.0,
    }
}

/// Per-route candidate fetch budget.
#[must_use]
pub const fn fetch_budget(route: RetrievalRoute) -> usize {
    match route {
        RetrievalRoute::Exact => 5,
        RetrievalRoute::Hybrid => 20,
        RetrievalRoute::HybridGraph => 30,
        RetrievalRoute::Abstain => 0,
    }
}

/// Per-route surface budget (automatic recall).
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn auto_surface_budget(route: RetrievalRoute) -> usize {
    match route {
        RetrievalRoute::Exact => 3,
        RetrievalRoute::Hybrid => 3,
        RetrievalRoute::HybridGraph => 5,
        RetrievalRoute::Abstain => 0,
    }
}

/// Per-route surface budget (MCP explicit recall).
#[must_use]
pub const fn mcp_surface_budget(route: RetrievalRoute) -> usize {
    match route {
        RetrievalRoute::Exact => 5,
        RetrievalRoute::Hybrid => 10,
        RetrievalRoute::HybridGraph => 15,
        RetrievalRoute::Abstain => 0,
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::needless_pass_by_ref_mut,
    clippy::match_same_arms
)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn gen_scoring_input(tc: &mut TestCase) -> ScoringInput {
        ScoringInput {
            semantic: tc.draw(
                gs::floats::<f64>()
                    .min_value(0.0)
                    .max_value(1.0)
                    .allow_nan(false),
            ),
            recency: tc.draw(
                gs::floats::<f64>()
                    .min_value(0.0)
                    .max_value(1.0)
                    .allow_nan(false),
            ),
            task_overlap: tc.draw(gs::sampled_from(vec![0.0_f64, 1.0])),
            graph_support: tc.draw(
                gs::floats::<f64>()
                    .min_value(0.0)
                    .max_value(1.0)
                    .allow_nan(false),
            ),
            is_decision: tc.draw(gs::booleans()),
            is_noisy: tc.draw(gs::booleans()),
        }
    }

    fn gen_route(tc: &mut TestCase) -> RetrievalRoute {
        tc.draw(gs::sampled_from(vec![
            RetrievalRoute::Exact,
            RetrievalRoute::Hybrid,
            RetrievalRoute::HybridGraph,
        ]))
    }

    // -- Property: normalized score is in [0.0, 1.0] --
    #[hegel::test(test_cases = 500)]
    fn prop_normalized_in_range(mut tc: TestCase) {
        let input = gen_scoring_input(&mut tc);
        let route = gen_route(&mut tc);
        let raw = composite_score(&input, &DEFAULT_WEIGHTS, route);
        let normalized = normalize_score(raw, &DEFAULT_WEIGHTS, route);
        assert!(
            (0.0..=1.0).contains(&normalized),
            "normalized score {normalized} out of range"
        );
    }

    // -- Property: monotonicity in semantic score --
    // Increasing semantic score (holding others fixed) never
    // decreases composite score.
    #[hegel::test(test_cases = 500)]
    fn prop_monotone_semantic(mut tc: TestCase) {
        let mut input = gen_scoring_input(&mut tc);
        let route = gen_route(&mut tc);

        let lo = tc.draw(
            gs::floats::<f64>()
                .min_value(0.0)
                .max_value(0.5)
                .allow_nan(false),
        );
        let hi = tc.draw(
            gs::floats::<f64>()
                .min_value(lo)
                .max_value(1.0)
                .allow_nan(false),
        );

        input.semantic = lo;
        let score_lo = composite_score(&input, &DEFAULT_WEIGHTS, route);
        input.semantic = hi;
        let score_hi = composite_score(&input, &DEFAULT_WEIGHTS, route);

        assert!(
            score_hi >= score_lo,
            "semantic: {lo} -> {hi}, score: {score_lo} -> {score_hi}"
        );
    }

    // -- Property: monotonicity in recency score --
    #[hegel::test(test_cases = 500)]
    fn prop_monotone_recency(mut tc: TestCase) {
        let mut input = gen_scoring_input(&mut tc);
        let route = gen_route(&mut tc);

        let lo = tc.draw(
            gs::floats::<f64>()
                .min_value(0.0)
                .max_value(0.5)
                .allow_nan(false),
        );
        let hi = tc.draw(
            gs::floats::<f64>()
                .min_value(lo)
                .max_value(1.0)
                .allow_nan(false),
        );

        input.recency = lo;
        let score_lo = composite_score(&input, &DEFAULT_WEIGHTS, route);
        input.recency = hi;
        let score_hi = composite_score(&input, &DEFAULT_WEIGHTS, route);

        assert!(score_hi >= score_lo);
    }

    // -- Property: decision bonus increases score --
    #[hegel::test(test_cases = 200)]
    fn prop_decision_bonus_positive(mut tc: TestCase) {
        let mut input = gen_scoring_input(&mut tc);
        let route = gen_route(&mut tc);

        input.is_decision = false;
        input.is_noisy = false;
        let score_no_bonus = composite_score(&input, &DEFAULT_WEIGHTS, route);

        input.is_decision = true;
        let score_with_bonus = composite_score(&input, &DEFAULT_WEIGHTS, route);

        assert!(score_with_bonus > score_no_bonus);
    }

    // -- Property: noise penalty decreases score --
    #[hegel::test(test_cases = 200)]
    fn prop_noise_penalty_negative(mut tc: TestCase) {
        let mut input = gen_scoring_input(&mut tc);
        let route = gen_route(&mut tc);

        input.is_noisy = false;
        let score_clean = composite_score(&input, &DEFAULT_WEIGHTS, route);

        input.is_noisy = true;
        let score_noisy = composite_score(&input, &DEFAULT_WEIGHTS, route);

        assert!(score_noisy < score_clean);
    }

    // -- Unit tests: known thresholds --
    #[test]
    fn test_auto_thresholds() {
        assert_eq!(auto_threshold(RetrievalRoute::Exact), 1.0);
        assert_eq!(auto_threshold(RetrievalRoute::Hybrid), 0.4);
        assert_eq!(auto_threshold(RetrievalRoute::HybridGraph), 0.35);
    }

    #[test]
    fn test_mcp_thresholds_lower() {
        assert!(
            mcp_threshold(RetrievalRoute::Hybrid)
                < auto_threshold(RetrievalRoute::Hybrid)
        );
        assert!(
            mcp_threshold(RetrievalRoute::HybridGraph)
                < auto_threshold(RetrievalRoute::HybridGraph)
        );
    }

    #[test]
    fn test_fetch_budgets() {
        assert_eq!(fetch_budget(RetrievalRoute::Exact), 5);
        assert_eq!(fetch_budget(RetrievalRoute::Hybrid), 20);
        assert_eq!(fetch_budget(RetrievalRoute::HybridGraph), 30);
        assert_eq!(fetch_budget(RetrievalRoute::Abstain), 0);
    }
}
