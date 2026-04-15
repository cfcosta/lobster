//! Dreaming scheduler for background maintenance.
//!
//! Dreaming means maintenance and consolidation — not speculative
//! autonomy. The scheduler runs maintenance jobs during idle time,
//! end-of-session, or after significant events.

use std::time::{Duration, Instant};

use crate::store::{crud, db::LobsterDb, schema::ProcessingState};

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
    pub profile_updated: bool,
}

/// Run one dreaming cycle: process pending maintenance jobs.
///
/// Currently implements:
/// - Retry `RetryQueued` episodes
///
/// Additional workers (run from the MCP server loop):
/// - Workflow pattern mining (`dream::workers::scan_workflow_patterns`)
/// - Decision supersession (`dream::supersession::scan_superseded_decisions`)
/// - Summary consolidation (`dream::consolidation::scan_consolidation_candidates`)
/// - Entity dedup (`dream::workers::find_duplicate_entities`)
/// - Stale task scanning (`dream::workers::scan_stale_tasks`)
#[must_use]
pub fn run_cycle(db: &LobsterDb, config: &DreamConfig) -> DreamCycleResult {
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

        if let Ok(mut ep) = crud::get_episode(db, episode_id_bytes) {
            if ep.retry_count >= config.max_retries {
                // Exhausted retry budget → FailedFinal
                ep.processing_state = ProcessingState::FailedFinal;
                let _ = crud::put_episode(db, &ep);
                result.episodes_failed_final += 1;
            } else {
                // Re-attempt analysis with the unified LLM call
                let summary_text =
                    crud::get_summary_artifact(db, &ep.episode_id.raw())
                        .map(|s| s.summary_text)
                        .unwrap_or_default();

                let prompt = format!(
                    "Repository: (retry)\n\n\
                     Summary from previous attempt:\n{summary_text}"
                );

                let analysis_ok =
                    tokio::runtime::Handle::try_current().ok().and_then(|h| {
                        tokio::task::block_in_place(|| {
                            h.block_on(async {
                                crate::extract::rig_extractor::analyze(&prompt)
                                    .await
                                    .ok()
                            })
                        })
                    });

                if let Some(analysis) = analysis_ok {
                    let output = crate::extract::traits::ExtractionOutput::from(
                        &analysis,
                    );
                    if crate::extract::validate::validate(&output).is_ok() {
                        ep.processing_state = ProcessingState::Ready;
                        let _ = crud::put_episode(db, &ep);
                        result.retries_succeeded += 1;
                        continue;
                    }
                }

                // Retry failed — increment counter
                ep.retry_count += 1;
                if ep.retry_count >= config.max_retries {
                    ep.processing_state = ProcessingState::FailedFinal;
                    result.episodes_failed_final += 1;
                }
                let _ = crud::put_episode(db, &ep);
            }
        }
    }

    // Update repo profiles from convention detection
    if !result.budget_exhausted {
        result.profile_updated = update_profiles(db);
    }

    result
}

/// Rebuild and persist profiles for all repos found in episodes.
fn update_profiles(db: &LobsterDb) -> bool {
    let repo_ids = collect_repo_ids(db);
    let mut updated = false;

    for repo_id in &repo_ids {
        let profile = crate::dream::profile::build_profile(db, repo_id);
        if profile.fact_count() > 0 {
            let _ = crud::put_repo_profile(db, &profile);
            updated = true;
        }
    }

    updated
}

/// Collect all distinct repo IDs from episodes.
fn collect_repo_ids(db: &LobsterDb) -> Vec<crate::store::ids::RepoId> {
    let mut repo_ids = std::collections::HashSet::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return vec![];
    };
    let Ok(iter) = db.episodes.iter(&rtxn) else {
        return vec![];
    };

    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(ep) =
            serde_json::from_slice::<crate::store::schema::Episode>(value)
        {
            repo_ids.insert(ep.repo_id);
        }
    }

    repo_ids.into_iter().collect()
}

/// Find all episodes in `RetryQueued` state.
fn find_retry_queued(db: &LobsterDb) -> Vec<crate::store::ids::RawId> {
    let mut result = Vec::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return result;
    };

    let Ok(iter) = db.episodes.iter(&rtxn) else {
        return result;
    };

    for entry in iter.flatten() {
        let (key, value) = entry;
        if let Ok(ep) =
            serde_json::from_slice::<crate::store::schema::Episode>(value)
        {
            if ep.processing_state == ProcessingState::RetryQueued {
                let key_bytes: [u8; 16] = key.try_into().unwrap_or([0; 16]);
                result.push(crate::store::ids::RawId::from_bytes(key_bytes));
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
        database: &LobsterDb,
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
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(database, &ep).unwrap();
    }

    #[test]
    fn test_empty_db_noop() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = DreamConfig::default();
        let result = run_cycle(&database, &config);
        assert_eq!(result.retries_attempted, 0);
        assert_eq!(result.episodes_failed_final, 0);
        assert!(!result.budget_exhausted);
    }

    #[test]
    fn test_retry_queued_exhausted_goes_to_failed_final() {
        let (database, _dir) = db::open_in_memory().unwrap();
        // Create episodes with retry_count >= max_retries so they
        // go straight to FailedFinal
        let ep1 = Episode {
            episode_id: EpisodeId::derive(b"retry1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::RetryQueued,
            finalized_ts_utc_ms: 1000,
            retry_count: 2,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep1).unwrap();
        let mut ep2 = ep1;
        ep2.episode_id = EpisodeId::derive(b"retry2");
        ep2.retry_count = 3;
        crud::put_episode(&database, &ep2).unwrap();
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
        let (database, _dir) = db::open_in_memory().unwrap();
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
        let (database, _dir) = db::open_in_memory().unwrap();
        make_episode(&database, b"r1", ProcessingState::RetryQueued);
        make_episode(&database, b"p1", ProcessingState::Pending);
        make_episode(&database, b"r2", ProcessingState::RetryQueued);

        let retries = find_retry_queued(&database);
        assert_eq!(retries.len(), 2);
    }

    // ── Profile update tests ─────────────────────────────────

    #[test]
    fn test_collect_repo_ids_empty() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let ids = collect_repo_ids(&database);
        assert!(ids.is_empty());
    }

    #[test]
    fn test_collect_repo_ids_deduplicates() {
        let (database, _dir) = db::open_in_memory().unwrap();
        // Two episodes with same repo_id
        make_episode(&database, b"ep1", ProcessingState::Ready);
        make_episode(&database, b"ep2", ProcessingState::Ready);
        let ids = collect_repo_ids(&database);
        assert_eq!(ids.len(), 1, "same repo should produce one ID");
    }

    #[test]
    fn test_update_profiles_no_episodes() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let updated = update_profiles(&database);
        assert!(!updated, "no episodes means no profile to update");
    }

    #[test]
    fn test_dream_cycle_includes_profile_update() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let config = DreamConfig::default();
        let result = run_cycle(&database, &config);
        // With no episodes, profile_updated should be false
        assert!(!result.profile_updated);
    }

    // -- Property: collect_repo_ids is idempotent --
    #[hegel::test(test_cases = 20)]
    fn prop_collect_repo_ids_deterministic(tc: hegel::TestCase) {
        let (database, _dir) = db::open_in_memory().unwrap();
        let n: usize = tc.draw(
            hegel::generators::integers::<usize>()
                .min_value(0)
                .max_value(5),
        );
        for i in 0..n {
            #[allow(clippy::cast_possible_truncation)]
            make_episode(
                &database,
                &(i as u32).to_le_bytes(),
                ProcessingState::Ready,
            );
        }
        let ids1 = collect_repo_ids(&database);
        let ids2 = collect_repo_ids(&database);
        assert_eq!(ids1.len(), ids2.len());
    }
}
