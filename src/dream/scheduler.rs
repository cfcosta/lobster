//! Dreaming scheduler for background maintenance.
//!
//! Dreaming means maintenance and consolidation — not speculative
//! autonomy. The scheduler runs maintenance jobs during idle time,
//! end-of-session, or after significant events.

use std::time::{Duration, Instant};

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{crud, schema::ProcessingState, tables};

/// Configuration for the dreaming scheduler.
#[derive(Debug, Clone)]
pub struct DreamConfig {
    /// Maximum wall-clock time per maintenance cycle.
    pub cycle_budget: Duration,
    /// Maximum retries before marking `FailedFinal`.
    pub max_retries: u32,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            cycle_budget: Duration::from_secs(5),
            max_retries: 2,
        }
    }
}

/// Result of a single dreaming cycle.
#[derive(Debug, Default)]
pub struct DreamCycleResult {
    pub retries_attempted: usize,
    pub retries_succeeded: usize,
    pub episodes_failed_final: usize,
    pub budget_exhausted: bool,
}

/// Run one dreaming cycle: process pending maintenance jobs.
///
/// Currently implements:
/// - Retry `RetryQueued` episodes
///
/// Future workers:
/// - Entity merge proposals
/// - Summary pyramid compression
/// - Task timeline maintenance
/// - Graph link backfill
/// - Statistics recalculation
#[must_use]
pub fn run_cycle(db: &Database, config: &DreamConfig) -> DreamCycleResult {
    let start = Instant::now();
    let mut result = DreamCycleResult::default();

    // Find RetryQueued episodes
    let retry_episodes = find_retry_queued(db);

    for episode_id_bytes in &retry_episodes {
        // Check budget
        if start.elapsed() >= config.cycle_budget {
            result.budget_exhausted = true;
            break;
        }

        result.retries_attempted += 1;

        // For now, retry just marks as FailedFinal since we
        // don't have a retry mechanism that would produce
        // different results. In a real system, this would
        // re-run extraction with tighter constraints.
        if let Ok(mut ep) = crud::get_episode(db, episode_id_bytes) {
            ep.processing_state = ProcessingState::FailedFinal;
            let _ = crud::put_episode(db, &ep);
            result.episodes_failed_final += 1;
        }
    }

    result
}

/// Find all episodes in `RetryQueued` state.
fn find_retry_queued(db: &Database) -> Vec<crate::store::ids::RawId> {
    let mut result = Vec::new();

    let Ok(read_txn) = db.begin_read() else {
        return result;
    };

    let Ok(table) = read_txn.open_table(tables::EPISODES) else {
        return result;
    };

    let Ok(iter) = table.iter() else {
        return result;
    };

    for (key, value) in iter.flatten() {
        if let Ok(ep) = serde_json::from_slice::<crate::store::schema::Episode>(
            value.value(),
        ) {
            if ep.processing_state == ProcessingState::RetryQueued {
                result.push(crate::store::ids::RawId::from_bytes(*key.value()));
            }
        }
    }

    result
}

/// Check if dreaming should yield to interactive work.
///
/// Returns `true` if the cycle budget is exhausted.
#[must_use]
pub fn should_yield(start: &Instant, config: &DreamConfig) -> bool {
    start.elapsed() >= config.cycle_budget
}

#[cfg(test)]
mod tests {
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
    ) {
        let ep = Episode {
            episode_id: EpisodeId::derive(suffix),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: state,
            finalized_ts_utc_ms: 1000,
        };
        crud::put_episode(database, &ep).unwrap();
    }

    #[test]
    fn test_empty_db_noop() {
        let database = db::open_in_memory().unwrap();
        let config = DreamConfig::default();
        let result = run_cycle(&database, &config);
        assert_eq!(result.retries_attempted, 0);
        assert_eq!(result.episodes_failed_final, 0);
        assert!(!result.budget_exhausted);
    }

    #[test]
    fn test_retry_queued_gets_processed() {
        let database = db::open_in_memory().unwrap();
        make_episode(&database, b"retry1", ProcessingState::RetryQueued);
        make_episode(&database, b"retry2", ProcessingState::RetryQueued);
        make_episode(&database, b"ready1", ProcessingState::Ready);

        let config = DreamConfig::default();
        let result = run_cycle(&database, &config);

        assert_eq!(result.retries_attempted, 2);
        assert_eq!(result.episodes_failed_final, 2);

        // Verify state was updated
        let ep =
            crud::get_episode(&database, &EpisodeId::derive(b"retry1").raw())
                .unwrap();
        assert_eq!(ep.processing_state, ProcessingState::FailedFinal);
    }

    #[test]
    fn test_ready_not_touched() {
        let database = db::open_in_memory().unwrap();
        make_episode(&database, b"ready", ProcessingState::Ready);

        let config = DreamConfig::default();
        let result = run_cycle(&database, &config);

        assert_eq!(result.retries_attempted, 0);

        // Ready episode unchanged
        let ep =
            crud::get_episode(&database, &EpisodeId::derive(b"ready").raw())
                .unwrap();
        assert_eq!(ep.processing_state, ProcessingState::Ready);
    }

    #[test]
    fn test_budget_enforcement() {
        let config = DreamConfig {
            cycle_budget: Duration::from_millis(0),
            max_retries: 2,
        };

        let start = Instant::now();
        // With 0ms budget, should yield immediately
        std::thread::sleep(Duration::from_millis(1));
        assert!(should_yield(&start, &config));
    }

    #[test]
    fn test_find_retry_queued() {
        let database = db::open_in_memory().unwrap();
        make_episode(&database, b"r1", ProcessingState::RetryQueued);
        make_episode(&database, b"p1", ProcessingState::Pending);
        make_episode(&database, b"r2", ProcessingState::RetryQueued);

        let retries = find_retry_queued(&database);
        assert_eq!(retries.len(), 2);
    }
}
