//! Cross-store visibility protocol.
//!
//! Readiness is episode-scoped: derived summaries, decisions,
//! entities, and graph relations inherit the visibility state of
//! their parent episode. Only `Ready`-state episodes and their
//! derived artifacts are eligible for retrieval.
//!
//! Protocol:
//! 1. Persist episode and artifacts in redb as `Pending`
//! 2. Apply Grafeo projection
//! 3. Record projection metadata in redb
//! 4. Flip episode to `Ready`
//! 5. All retrieval intersects with the ready set

use redb::Database;

use crate::store::{crud, ids::RawId, schema::ProcessingState};

/// Check whether an episode is visible for retrieval.
///
/// An episode is visible only if it exists in redb AND has
/// `ProcessingState::Ready`. Pending, `RetryQueued`, and
/// `FailedFinal` episodes are not visible.
#[must_use]
pub fn is_episode_visible(db: &Database, episode_id: &RawId) -> bool {
    crud::get_episode(db, episode_id)
        .is_ok_and(|ep| ep.processing_state == ProcessingState::Ready)
}

/// Filter a set of episode IDs to only those that are visible.
///
/// This is the "ready-set intersection" that every retrieval
/// path must perform before returning results.
#[must_use]
pub fn filter_visible(db: &Database, episode_ids: &[RawId]) -> Vec<RawId> {
    episode_ids
        .iter()
        .filter(|id| is_episode_visible(db, id))
        .copied()
        .collect()
}

/// Get the processing state of an episode, or `None` if it
/// doesn't exist.
#[must_use]
pub fn get_state(db: &Database, episode_id: &RawId) -> Option<ProcessingState> {
    crud::get_episode(db, episode_id)
        .ok()
        .map(|ep| ep.processing_state)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        db,
        ids::{EpisodeId, RepoId},
        schema::Episode,
    };

    fn make_episode(
        database: &Database,
        suffix: &[u8],
        state: ProcessingState,
    ) -> EpisodeId {
        let id = EpisodeId::derive(suffix);
        let ep = Episode {
            episode_id: id,
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: state,
            finalized_ts_utc_ms: 1_000,
            retry_count: 0,
        };
        crud::put_episode(database, &ep).unwrap();
        id
    }

    // ── Property: only Ready episodes are visible ────────
    // Invariant: is_episode_visible returns true iff state
    // is Ready. This is the core safety guarantee.
    #[test]
    fn test_only_ready_is_visible() {
        let database = db::open_in_memory().unwrap();

        let ready = make_episode(&database, b"r", ProcessingState::Ready);
        let pending = make_episode(&database, b"p", ProcessingState::Pending);
        let retry = make_episode(&database, b"q", ProcessingState::RetryQueued);
        let failed =
            make_episode(&database, b"f", ProcessingState::FailedFinal);

        assert!(is_episode_visible(&database, &ready.raw()));
        assert!(!is_episode_visible(&database, &pending.raw()));
        assert!(!is_episode_visible(&database, &retry.raw()));
        assert!(!is_episode_visible(&database, &failed.raw()));
    }

    // ── Property: nonexistent episodes are not visible ───
    #[test]
    fn test_nonexistent_not_visible() {
        let database = db::open_in_memory().unwrap();
        let ghost = EpisodeId::derive(b"ghost");
        assert!(!is_episode_visible(&database, &ghost.raw()));
    }

    // ── Property: filter_visible only keeps Ready ────────
    #[test]
    fn test_filter_visible() {
        let database = db::open_in_memory().unwrap();

        let r1 = make_episode(&database, b"r1", ProcessingState::Ready);
        let r2 = make_episode(&database, b"r2", ProcessingState::Ready);
        let p1 = make_episode(&database, b"p1", ProcessingState::Pending);
        let ghost = EpisodeId::derive(b"ghost");

        let candidates = vec![r1.raw(), p1.raw(), r2.raw(), ghost.raw()];
        let visible = filter_visible(&database, &candidates);

        assert_eq!(visible.len(), 2);
        assert!(visible.contains(&r1.raw()));
        assert!(visible.contains(&r2.raw()));
    }

    // ── PBT: visibility is deterministic ─────────────────
    // Same database state → same visibility decision.
    #[hegel::test(test_cases = 100)]
    fn prop_visibility_deterministic(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let state_idx: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let states = [
            ProcessingState::Pending,
            ProcessingState::Ready,
            ProcessingState::RetryQueued,
            ProcessingState::FailedFinal,
        ];
        let state = states[state_idx];
        let suffix: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));

        let id = make_episode(&database, &suffix, state);

        let v1 = is_episode_visible(&database, &id.raw());
        let v2 = is_episode_visible(&database, &id.raw());
        assert_eq!(v1, v2, "visibility must be deterministic");
        assert_eq!(
            v1,
            state == ProcessingState::Ready,
            "only Ready should be visible"
        );
    }

    // ── PBT: filter_visible ⊆ input ─────────────────────
    // The filtered set is always a subset of the input.
    #[hegel::test(test_cases = 50)]
    fn prop_filter_is_subset(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(10));
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let state_idx: usize =
                tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
            let states = [
                ProcessingState::Pending,
                ProcessingState::Ready,
                ProcessingState::RetryQueued,
                ProcessingState::FailedFinal,
            ];
            let suffix = format!("ep-{i}");
            let id =
                make_episode(&database, suffix.as_bytes(), states[state_idx]);
            ids.push(id.raw());
        }

        let visible = filter_visible(&database, &ids);

        // Every visible ID must be in the original set
        for v in &visible {
            assert!(
                ids.contains(v),
                "filtered result must be a subset of input"
            );
        }
    }
}
