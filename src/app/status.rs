//! Status reporting: scan redb and report episode counts by state.

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{schema::ProcessingState, tables};

/// Counts of episodes by processing state.
#[derive(Debug, Default)]
pub struct StatusReport {
    pub pending: usize,
    pub ready: usize,
    pub retry_queued: usize,
    pub failed_final: usize,
    pub summary_artifacts: usize,
    pub extraction_artifacts: usize,
}

impl StatusReport {
    #[must_use]
    pub const fn total_episodes(&self) -> usize {
        self.pending + self.ready + self.retry_queued + self.failed_final
    }
}

impl std::fmt::Display for StatusReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Episodes: {} total", self.total_episodes())?;
        writeln!(f, "  Ready:        {}", self.ready)?;
        writeln!(f, "  Pending:      {}", self.pending)?;
        writeln!(f, "  RetryQueued:  {}", self.retry_queued)?;
        writeln!(f, "  FailedFinal:  {}", self.failed_final)?;
        writeln!(f, "Artifacts:")?;
        writeln!(f, "  Summaries:    {}", self.summary_artifacts)?;
        write!(f, "  Extractions:  {}", self.extraction_artifacts)
    }
}

/// Scan the redb database and produce a status report.
#[must_use]
pub fn scan(db: &Database) -> StatusReport {
    let mut report = StatusReport::default();

    // Count episodes by state
    if let Ok(read_txn) = db.begin_read() {
        if let Ok(table) = read_txn.open_table(tables::EPISODES) {
            if let Ok(iter) = table.iter() {
                for entry in iter.flatten() {
                    let (_, value) = entry;
                    if let Ok(ep) = serde_json::from_slice::<
                        crate::store::schema::Episode,
                    >(value.value())
                    {
                        match ep.processing_state {
                            ProcessingState::Pending => {
                                report.pending += 1;
                            }
                            ProcessingState::Ready => {
                                report.ready += 1;
                            }
                            ProcessingState::RetryQueued => {
                                report.retry_queued += 1;
                            }
                            ProcessingState::FailedFinal => {
                                report.failed_final += 1;
                            }
                        }
                    }
                }
            }
        }

        // Count summary artifacts
        if let Ok(table) = read_txn.open_table(tables::SUMMARY_ARTIFACTS) {
            if let Ok(iter) = table.iter() {
                report.summary_artifacts = iter.flatten().count();
            }
        }

        // Count extraction artifacts
        if let Ok(table) = read_txn.open_table(tables::EXTRACTION_ARTIFACTS) {
            if let Ok(iter) = table.iter() {
                report.extraction_artifacts = iter.flatten().count();
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        episodes::finalize::{FinalizeResult, finalize_episode},
        graph::db as grafeo_db,
        store::{crud, db, ids::*, schema::*},
    };

    #[test]
    fn test_empty_db() {
        let database = db::open_in_memory().unwrap();
        let report = scan(&database);
        assert_eq!(report.total_episodes(), 0);
        assert_eq!(report.summary_artifacts, 0);
    }

    #[test]
    fn test_counts_by_state() {
        let database = db::open_in_memory().unwrap();

        for (i, state) in [
            ProcessingState::Ready,
            ProcessingState::Ready,
            ProcessingState::Pending,
            ProcessingState::RetryQueued,
            ProcessingState::FailedFinal,
        ]
        .iter()
        .enumerate()
        {
            let ep = Episode {
                episode_id: EpisodeId::derive(format!("ep-{i}").as_bytes()),
                repo_id: RepoId::derive(b"repo"),
                start_seq: 0,
                end_seq: 5,
                task_id: None,
                processing_state: *state,
                finalized_ts_utc_ms: 1000,
                retry_count: 0,
            };
            crud::put_episode(&database, &ep).unwrap();
        }

        let report = scan(&database);
        assert_eq!(report.total_episodes(), 5);
        assert_eq!(report.ready, 2);
        assert_eq!(report.pending, 1);
        assert_eq!(report.retry_queued, 1);
        assert_eq!(report.failed_final, 1);
    }

    #[tokio::test]
    async fn test_counts_after_finalization() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let result =
            finalize_episode(&database, &grafeo, "/repo", b"[]", 0, 5, None)
                .await;
        assert!(matches!(result, FinalizeResult::Ready { .. }));

        let report = scan(&database);
        assert_eq!(report.ready, 1);
        assert_eq!(report.summary_artifacts, 1);
        assert_eq!(report.extraction_artifacts, 1);
    }

    #[test]
    fn test_display_format() {
        let report = StatusReport {
            pending: 1,
            ready: 5,
            retry_queued: 2,
            failed_final: 0,
            summary_artifacts: 5,
            extraction_artifacts: 4,
        };
        let output = report.to_string();
        assert!(output.contains("8 total"));
        assert!(output.contains("Ready:        5"));
        assert!(output.contains("Summaries:    5"));
    }
}
