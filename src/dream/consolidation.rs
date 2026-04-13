//! Summary pyramid consolidation.
//!
//! When multiple episodes share the same task, this worker merges
//! their individual summaries into a single consolidated task
//! summary. This reduces retrieval corpus size and improves ranking
//! signal by replacing N separate episode summaries with one
//! coherent task-level summary.

use std::collections::HashMap;

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{
    crud,
    ids::{EpisodeId, TaskId},
    schema::{Episode, ProcessingState},
    tables,
};

/// Result of a consolidation pass.
#[derive(Debug, Default)]
pub struct ConsolidationResult {
    /// Number of tasks with multiple episodes found.
    pub multi_episode_tasks: usize,
    /// Number of consolidated summaries produced.
    pub summaries_produced: usize,
}

/// A consolidated summary for a task spanning multiple episodes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConsolidatedTaskSummary {
    pub task_id: TaskId,
    pub episode_count: usize,
    pub consolidated_text: String,
    pub source_episodes: Vec<EpisodeId>,
    pub produced_ts_utc_ms: i64,
}

/// Merge multiple episode summaries into a single consolidated text.
///
/// Produces a compact summary that combines the key information from
/// each episode summary, prefixed with the episode count.
#[must_use]
pub fn merge_summaries(summaries: &[(EpisodeId, String)]) -> String {
    if summaries.is_empty() {
        return String::new();
    }
    if summaries.len() == 1 {
        return summaries[0].1.clone();
    }

    // Filter out "No significant changes" entries
    let meaningful: Vec<&(EpisodeId, String)> = summaries
        .iter()
        .filter(|(_, s)| {
            !s.is_empty()
                && !s.eq_ignore_ascii_case("No significant changes.")
                && !s.eq_ignore_ascii_case("No significant changes")
        })
        .collect();

    if meaningful.is_empty() {
        return "No significant changes across multiple sessions.".into();
    }

    if meaningful.len() == 1 {
        return meaningful[0].1.clone();
    }

    let mut parts = Vec::with_capacity(meaningful.len() + 1);
    parts.push(format!("Consolidated from {} episodes:", meaningful.len()));
    for (i, (_, text)) in meaningful.iter().enumerate() {
        // Trim each summary and add as a numbered entry
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            parts.push(format!("{}. {trimmed}", i + 1));
        }
    }

    parts.join("\n")
}

/// Scan for tasks with multiple Ready episodes and produce
/// consolidated summaries.
///
/// Returns consolidated summaries keyed by task ID. These can be
/// stored or used to replace individual episode summaries in
/// retrieval.
#[allow(clippy::must_use_candidate)]
pub fn scan_consolidation_candidates(
    db: &Database,
) -> (ConsolidationResult, Vec<ConsolidatedTaskSummary>) {
    let mut result = ConsolidationResult::default();
    let mut consolidated = Vec::new();

    // Load all Ready episodes grouped by task_id
    let Ok(read_txn) = db.begin_read() else {
        return (result, consolidated);
    };
    let Ok(table) = read_txn.open_table(tables::EPISODES) else {
        return (result, consolidated);
    };
    let Ok(iter) = table.iter() else {
        return (result, consolidated);
    };

    let mut by_task: HashMap<TaskId, Vec<Episode>> = HashMap::new();
    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(ep) = serde_json::from_slice::<Episode>(value.value()) {
            if ep.processing_state == ProcessingState::Ready {
                if let Some(task_id) = ep.task_id {
                    by_task.entry(task_id).or_default().push(ep);
                }
            }
        }
    }
    drop(table);
    drop(read_txn);

    // For each task with 2+ episodes, merge summaries
    for (task_id, episodes) in &by_task {
        if episodes.len() < 2 {
            continue;
        }

        result.multi_episode_tasks += 1;

        // Load summary for each episode
        let mut summaries: Vec<(EpisodeId, String)> = Vec::new();
        for ep in episodes {
            if let Ok(art) =
                crud::get_summary_artifact(db, &ep.episode_id.raw())
            {
                summaries.push((ep.episode_id, art.summary_text));
            }
        }

        if summaries.len() < 2 {
            continue;
        }

        // Sort by episode finalized timestamp for chronological order
        let mut episodes_sorted: Vec<&Episode> = episodes.iter().collect();
        episodes_sorted.sort_by_key(|ep| ep.finalized_ts_utc_ms);
        let sorted_summaries: Vec<(EpisodeId, String)> = episodes_sorted
            .iter()
            .filter_map(|ep| {
                summaries
                    .iter()
                    .find(|(id, _)| *id == ep.episode_id)
                    .cloned()
            })
            .collect();

        let merged = merge_summaries(&sorted_summaries);
        let now_ms = chrono::Utc::now().timestamp_millis();

        consolidated.push(ConsolidatedTaskSummary {
            task_id: *task_id,
            episode_count: sorted_summaries.len(),
            consolidated_text: merged,
            source_episodes: sorted_summaries
                .iter()
                .map(|(id, _)| *id)
                .collect(),
            produced_ts_utc_ms: now_ms,
        });

        result.summaries_produced += 1;
    }

    (result, consolidated)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        db,
        ids::{EpisodeId, RepoId, TaskId},
        schema::{Episode, ProcessingState, SummaryArtifact},
    };

    // -- Property: merge_summaries output contains all meaningful inputs --
    #[hegel::test(test_cases = 50)]
    fn prop_merge_contains_inputs(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let mut summaries = Vec::with_capacity(n);
        for i in 0..n {
            let text: String = tc.draw(
                gs::text()
                    .min_size(5)
                    .max_size(50)
                    .alphabet("abcdefghijklmnop "),
            );
            #[allow(clippy::cast_possible_truncation)]
            summaries
                .push((EpisodeId::derive(&(i as u32).to_le_bytes()), text));
        }

        let merged = merge_summaries(&summaries);
        // Each non-trivial summary text should appear in the output
        for (_, text) in &summaries {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                assert!(
                    merged.contains(trimmed),
                    "merged text should contain: {trimmed}"
                );
            }
        }
    }

    // -- Property: merge of empty inputs is empty/default --
    #[test]
    fn test_merge_empty() {
        assert_eq!(merge_summaries(&[]), "");
    }

    // -- Property: merge of single input returns that input --
    #[hegel::test(test_cases = 30)]
    fn prop_merge_single_identity(tc: TestCase) {
        let text: String = tc.draw(gs::text().min_size(1).max_size(50));
        let summaries = vec![(EpisodeId::derive(b"ep1"), text.clone())];
        let merged = merge_summaries(&summaries);
        assert_eq!(merged, text);
    }

    // -- Property: merge filters "No significant changes" --
    #[test]
    fn test_merge_filters_noise() {
        let summaries = vec![
            (EpisodeId::derive(b"ep1"), "No significant changes.".into()),
            (EpisodeId::derive(b"ep2"), "No significant changes.".into()),
        ];
        let merged = merge_summaries(&summaries);
        assert_eq!(merged, "No significant changes across multiple sessions.");
    }

    // -- Unit: consolidation with no multi-episode tasks --
    #[test]
    fn test_consolidation_no_multi_episode() {
        let database = db::open_in_memory().unwrap();

        // Single episode with a task
        let ep = Episode {
            episode_id: EpisodeId::derive(b"ep1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: Some(TaskId::derive(b"t1")),
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep).unwrap();

        let (result, consolidated) = scan_consolidation_candidates(&database);
        assert_eq!(result.multi_episode_tasks, 0);
        assert!(consolidated.is_empty());
    }

    // -- Unit: consolidation with multi-episode task --
    #[test]
    fn test_consolidation_multi_episode() {
        let database = db::open_in_memory().unwrap();
        let task_id = TaskId::derive(b"task1");

        for i in 0..3u32 {
            let ep = Episode {
                episode_id: EpisodeId::derive(&i.to_le_bytes()),
                repo_id: RepoId::derive(b"repo"),
                start_seq: u64::from(i) * 10,
                end_seq: u64::from(i) * 10 + 5,
                task_id: Some(task_id),
                processing_state: ProcessingState::Ready,
                finalized_ts_utc_ms: i64::from(i) * 1000,
                retry_count: 0,
                is_noisy: false,
            };
            crud::put_episode(&database, &ep).unwrap();

            let art = SummaryArtifact {
                episode_id: EpisodeId::derive(&i.to_le_bytes()),
                revision: "v1".into(),
                summary_text: format!("Episode {i} did work on feature X"),
                payload_checksum: [0; 32],
            };
            crud::put_summary_artifact(&database, &art).unwrap();
        }

        let (result, consolidated) = scan_consolidation_candidates(&database);
        assert_eq!(result.multi_episode_tasks, 1);
        assert_eq!(result.summaries_produced, 1);
        assert_eq!(consolidated.len(), 1);
        assert_eq!(consolidated[0].episode_count, 3);
        assert!(consolidated[0].consolidated_text.contains("Episode 0"));
        assert!(consolidated[0].consolidated_text.contains("Episode 2"));
    }

    // -- Property: consolidated summary serde round-trip --
    #[hegel::test(test_cases = 50)]
    fn prop_consolidated_summary_roundtrip(tc: TestCase) {
        let task_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let n_eps: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let mut eps = Vec::with_capacity(n_eps);
        for i in 0..n_eps {
            let mut ep_in = task_input.clone();
            #[allow(clippy::cast_possible_truncation)]
            ep_in.extend_from_slice(&(i as u32).to_le_bytes());
            eps.push(EpisodeId::derive(&ep_in));
        }

        let cs = ConsolidatedTaskSummary {
            task_id: TaskId::derive(&task_input),
            episode_count: n_eps,
            consolidated_text: tc.draw(gs::text().min_size(1).max_size(200)),
            source_episodes: eps,
            produced_ts_utc_ms: tc.draw(
                gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2),
            ),
        };
        let json = serde_json::to_string(&cs).unwrap();
        let parsed: ConsolidatedTaskSummary =
            serde_json::from_str(&json).unwrap();
        assert_eq!(cs, parsed);
    }

    // -- Property: episode_count matches source_episodes length --
    #[test]
    fn test_episode_count_consistency() {
        let database = db::open_in_memory().unwrap();
        let task_id = TaskId::derive(b"task1");

        for i in 0..4u32 {
            let ep = Episode {
                episode_id: EpisodeId::derive(&i.to_le_bytes()),
                repo_id: RepoId::derive(b"repo"),
                start_seq: u64::from(i) * 10,
                end_seq: u64::from(i) * 10 + 5,
                task_id: Some(task_id),
                processing_state: ProcessingState::Ready,
                finalized_ts_utc_ms: i64::from(i) * 1000,
                retry_count: 0,
                is_noisy: false,
            };
            crud::put_episode(&database, &ep).unwrap();

            let art = SummaryArtifact {
                episode_id: EpisodeId::derive(&i.to_le_bytes()),
                revision: "v1".into(),
                summary_text: format!("Work item {i}"),
                payload_checksum: [0; 32],
            };
            crud::put_summary_artifact(&database, &art).unwrap();
        }

        let (_, consolidated) = scan_consolidation_candidates(&database);
        for cs in &consolidated {
            assert_eq!(cs.episode_count, cs.source_episodes.len());
        }
    }
}
