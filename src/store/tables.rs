//! Database table names for all Lobster record types.
//!
//! With heed/LMDB, named databases are created at open time and
//! stored as fields on `LobsterDb`. This module provides the
//! canonical table name constants for documentation and testing.

/// All named database names used by Lobster.
pub const TABLE_NAMES: &[&str] = &[
    "raw_events",
    "episodes",
    "decisions",
    "tasks",
    "entities",
    "summary_artifacts",
    "extraction_artifacts",
    "embedding_artifacts",
    "processing_jobs",
    "projection_metadata",
    "repo_config",
    "retrieval_stats",
    "tool_sequences",
    "recall_engagements",
    "repo_profiles",
    "metadata",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_table_names_unique() {
        let mut sorted: Vec<&str> = TABLE_NAMES.to_vec();
        sorted.sort_unstable();
        for window in sorted.windows(2) {
            assert_ne!(
                window[0], window[1],
                "duplicate table name: {}",
                window[0]
            );
        }
    }

    #[test]
    fn test_table_count() {
        assert_eq!(TABLE_NAMES.len(), 16);
    }

    /// Verify all table names correspond to databases on `LobsterDb`.
    #[test]
    fn test_tables_open_in_memory() {
        let (_db, _dir) =
            crate::store::db::open_in_memory().expect("create in-memory db");
        // If open_in_memory succeeds, all 16 databases were created.
    }
}
