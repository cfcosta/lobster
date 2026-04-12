//! Recency scoring: exponential decay from artifact timestamp.
//!
//! Per spec: "`recency_score` (0.0–1.0): exponential decay from
//! artifact timestamp. Half-life tunable per artifact class."

/// Default half-life in milliseconds (1 hour).
const DEFAULT_HALF_LIFE_MS: f64 = 3_600_000.0;

/// Compute recency score using exponential decay.
///
/// Returns a value in [0.0, 1.0] where 1.0 means "just now" and
/// values approach 0.0 as the artifact ages.
///
/// Formula: `2^(-age_ms / half_life_ms)`
#[must_use]
pub fn recency_score(
    artifact_ts_ms: i64,
    now_ms: i64,
    half_life_ms: f64,
) -> f64 {
    if artifact_ts_ms >= now_ms {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let age_ms = (now_ms - artifact_ts_ms) as f64;
    let decay = (-age_ms / half_life_ms).exp2();
    decay.clamp(0.0, 1.0)
}

/// Compute recency with the default half-life.
#[must_use]
pub fn recency_score_default(artifact_ts_ms: i64, now_ms: i64) -> f64 {
    recency_score(artifact_ts_ms, now_ms, DEFAULT_HALF_LIFE_MS)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    #[test]
    fn test_current_timestamp_is_one() {
        let now = 1_700_000_000_000_i64;
        let score = recency_score_default(now, now);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_one_half_life_ago_is_half() {
        let now = 1_700_000_000_000_i64;
        let one_hour_ago = now - 3_600_000;
        let score = recency_score_default(one_hour_ago, now);
        assert!(
            (score - 0.5).abs() < 0.01,
            "one half-life ago should be ~0.5, got {score}"
        );
    }

    #[test]
    fn test_very_old_approaches_zero() {
        let now = 1_700_000_000_000_i64;
        let week_ago = now - 7 * 24 * 3_600_000;
        let score = recency_score_default(week_ago, now);
        assert!(score < 0.01, "a week ago should be near zero, got {score}");
    }

    #[test]
    fn test_future_timestamp_clamped_to_one() {
        let now = 1_700_000_000_000_i64;
        let score = recency_score_default(now + 1000, now);
        assert!((score - 1.0).abs() < 1e-6);
    }

    /// Property: recency is monotonically decreasing with age.
    #[hegel::test(test_cases = 200)]
    fn prop_recency_monotone(tc: TestCase) {
        let now: i64 = 1_700_000_000_000;
        let age1: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(100_000_000));
        let age2: i64 = tc
            .draw(gs::integers::<i64>().min_value(age1).max_value(200_000_000));

        let s1 = recency_score_default(now - age1, now);
        let s2 = recency_score_default(now - age2, now);
        assert!(
            s1 >= s2,
            "older artifacts must not score higher: age1={age1}, s1={s1}, age2={age2}, s2={s2}"
        );
    }

    /// Property: recency is always in [0.0, 1.0].
    #[hegel::test(test_cases = 200)]
    fn prop_recency_in_range(tc: TestCase) {
        let now: i64 = 1_700_000_000_000;
        let ts: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(now));
        let score = recency_score_default(ts, now);
        assert!((0.0..=1.0).contains(&score), "recency {score} out of range");
    }
}
