//! redb table definitions for all Lobster record types.
//!
//! Tables are defined as `const` at module level using redb's
//! `TableDefinition`. Keys are either `u64` (sequence numbers)
//! or `&[u8; 16]` (raw ID bytes). Values are `&[u8]` containing
//! serde_json-serialized records.

use redb::TableDefinition;

// ── Core record tables ───────────────────────────────────────

/// Raw hook events, keyed by monotonic sequence number.
pub const RAW_EVENTS: TableDefinition<u64, &[u8]> =
    TableDefinition::new("raw_events");

/// Episode records, keyed by episode ID bytes.
pub const EPISODES: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("episodes");

/// Decision records, keyed by decision ID bytes.
pub const DECISIONS: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("decisions");

/// Task records, keyed by task ID bytes.
pub const TASKS: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("tasks");

/// Entity records, keyed by entity ID bytes.
pub const ENTITIES: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("entities");

// ── Artifact tables ──────────────────────────────────────────

/// Summary artifacts, keyed by episode ID bytes.
pub const SUMMARY_ARTIFACTS: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("summary_artifacts");

/// Extraction artifacts, keyed by episode ID bytes.
pub const EXTRACTION_ARTIFACTS: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("extraction_artifacts");

/// Embedding artifacts, keyed by artifact ID bytes.
pub const EMBEDDING_ARTIFACTS: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("embedding_artifacts");

// ── Operational tables ───────────────────────────────────────

/// Processing jobs (embedding, extraction, projection), keyed by
/// a job sequence number.
pub const PROCESSING_JOBS: TableDefinition<u64, &[u8]> =
    TableDefinition::new("processing_jobs");

/// Projection metadata (version, applied-at, checksum), keyed by
/// episode ID bytes.
pub const PROJECTION_METADATA: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("projection_metadata");

/// Per-repo configuration and ignore rules, keyed by repo ID
/// bytes.
pub const REPO_CONFIG: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("repo_config");

/// Retrieval statistics and surfacing telemetry, keyed by a stats
/// key string.
pub const RETRIEVAL_STATS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("retrieval_stats");

/// Tool-use workflow patterns (procedural memory), keyed by
/// workflow ID bytes.
pub const TOOL_SEQUENCES: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("tool_sequences");

/// Global metadata (schema version, last sequence number, etc.).
pub const METADATA: TableDefinition<&str, &[u8]> =
    TableDefinition::new("metadata");

#[cfg(test)]
mod tests {
    use super::*;

    // Table names must be unique — verified by opening all tables
    // in one transaction (redb would error on duplicate names with
    // different type signatures, and same-name same-type would
    // silently share storage). We list them here for documentation.
    #[test]
    fn test_all_table_names_unique() {
        let names = [
            "raw_events",
            "episodes",
            "decisions",
            "tasks",
            "entities",
            "summary_artifacts",
            "extraction_artifacts",
            "embedding_artifacts",
            "tool_sequences",
            "processing_jobs",
            "projection_metadata",
            "repo_config",
            "retrieval_stats",
            "metadata",
        ];
        let mut sorted = names;
        sorted.sort_unstable();
        for window in sorted.windows(2) {
            assert_ne!(
                window[0], window[1],
                "duplicate table name: {}",
                window[0]
            );
        }
    }

    // Verify all table definitions can be used with redb's
    // InMemoryBackend (catches type constraint issues at compile
    // time and runtime).
    #[test]
    fn test_tables_open_in_memory() {
        use redb::{Database, backends::InMemoryBackend};

        let backend = InMemoryBackend::new();
        let db = Database::builder()
            .create_with_backend(backend)
            .expect("create in-memory db");

        let write_txn = db.begin_write().expect("begin write");
        // Open every table to verify the type parameters work
        write_txn.open_table(RAW_EVENTS).expect("raw_events");
        write_txn.open_table(EPISODES).expect("episodes");
        write_txn.open_table(DECISIONS).expect("decisions");
        write_txn.open_table(TASKS).expect("tasks");
        write_txn.open_table(ENTITIES).expect("entities");
        write_txn
            .open_table(SUMMARY_ARTIFACTS)
            .expect("summary_artifacts");
        write_txn
            .open_table(EXTRACTION_ARTIFACTS)
            .expect("extraction_artifacts");
        write_txn
            .open_table(EMBEDDING_ARTIFACTS)
            .expect("embedding_artifacts");
        write_txn
            .open_table(TOOL_SEQUENCES)
            .expect("tool_sequences");
        write_txn
            .open_table(PROCESSING_JOBS)
            .expect("processing_jobs");
        write_txn
            .open_table(PROJECTION_METADATA)
            .expect("projection_metadata");
        write_txn.open_table(REPO_CONFIG).expect("repo_config");
        write_txn
            .open_table(RETRIEVAL_STATS)
            .expect("retrieval_stats");
        write_txn.open_table(METADATA).expect("metadata");
        write_txn.commit().expect("commit");
    }
}
