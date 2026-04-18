//! Database initialization and lifecycle management.
//!
//! Wraps LMDB (via heed) with Lobster-specific setup: creates all
//! named databases on first open. LMDB provides MVCC — multiple
//! readers can coexist with a single writer without blocking.
#![allow(unsafe_code)]

use std::path::Path;

use heed::{
    Database,
    Env,
    EnvOpenOptions,
    types::{Bytes, Str, U64},
};
use thiserror::Error;

/// Default map size: 1 GiB. LMDB requires a pre-declared maximum.
const DEFAULT_MAP_SIZE: usize = 1024 * 1024 * 1024;

/// Maximum number of named databases (we have 15, leave headroom).
const MAX_DBS: u32 = 20;

/// A sequence-keyed database (u64 -> bytes).
pub type SeqDb = Database<U64<byteorder::BigEndian>, Bytes>;

/// An ID-keyed database ([u8] -> bytes).
pub type IdDb = Database<Bytes, Bytes>;

/// A string-keyed database (str -> bytes).
pub type StrDb = Database<Str, Bytes>;

/// The Lobster database: an LMDB environment plus all named databases.
pub struct LobsterDb {
    pub env: Env,

    // Core record tables
    pub raw_events: SeqDb,
    pub episodes: IdDb,
    pub decisions: IdDb,
    pub tasks: IdDb,
    pub entities: IdDb,

    // Artifact tables
    pub summary_artifacts: IdDb,
    pub extraction_artifacts: IdDb,
    pub embedding_artifacts: IdDb,

    // Operational tables
    pub processing_jobs: SeqDb,
    pub projection_metadata: IdDb,
    pub repo_config: IdDb,
    pub retrieval_stats: StrDb,
    pub tool_sequences: IdDb,
    pub recall_engagements: IdDb,
    pub repo_profiles: IdDb,
    pub metadata: StrDb,
}

/// Errors returned by [`open_snapshot`].
///
/// The snapshot path copies the live `data.mdb` to a temporary
/// directory before opening it, so failures split cleanly between
/// the copy step and the heed open step.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("I/O error preparing snapshot: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Heed(#[from] heed::Error),
}

/// Open or create a Lobster database at the given directory path.
///
/// Creates all named databases on first access. The path must be a
/// directory (LMDB stores `data.mdb` and `lock.mdb` inside it).
///
/// # Errors
///
/// Returns a heed error if the environment cannot be opened.
pub fn open(path: &Path) -> Result<LobsterDb, heed::Error> {
    std::fs::create_dir_all(path).map_err(|e| {
        heed::Error::Io(std::io::Error::new(
            e.kind(),
            format!("create db dir: {e}"),
        ))
    })?;

    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(DEFAULT_MAP_SIZE)
            .max_dbs(MAX_DBS)
            .open(path)?
    };

    create_databases(&env)
}

/// Open a consistent snapshot of the Lobster database without
/// contending for the live writer lock.
///
/// Copies `data.mdb` from `path` to a fresh temporary directory and
/// opens that copy. LMDB commits write meta pages atomically, so a
/// file-level copy of `data.mdb` always reflects some consistent
/// committed state (possibly slightly stale — this is fine for the
/// `lobster status` use case).
///
/// The returned [`tempfile::TempDir`] must stay alive for as long as
/// the returned [`LobsterDb`] is used; dropping it removes the
/// snapshot files.
///
/// # Errors
///
/// Returns [`SnapshotError::Io`] if the snapshot directory or
/// `data.mdb` copy fails, or [`SnapshotError::Heed`] if the copied
/// environment cannot be opened.
pub fn open_snapshot(
    path: &Path,
) -> Result<(LobsterDb, tempfile::TempDir), SnapshotError> {
    let snapshot_dir = tempfile::tempdir()?;
    std::fs::copy(path.join("data.mdb"), snapshot_dir.path().join("data.mdb"))?;
    let db = open(snapshot_dir.path())?;
    Ok((db, snapshot_dir))
}

/// Create a temporary database for testing.
///
/// Uses a temp directory that lives as long as the returned struct.
///
/// # Errors
///
/// Returns a heed error if the database cannot be initialized.
pub fn open_in_memory() -> Result<(LobsterDb, tempfile::TempDir), heed::Error> {
    let dir = tempfile::tempdir().map_err(|e| {
        heed::Error::Io(std::io::Error::new(e.kind(), format!("tempdir: {e}")))
    })?;

    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(DEFAULT_MAP_SIZE)
            .max_dbs(MAX_DBS)
            .open(dir.path())?
    };

    let db = create_databases(&env)?;
    Ok((db, dir))
}

/// Create all named databases within an environment.
fn create_databases(env: &Env) -> Result<LobsterDb, heed::Error> {
    let mut wtxn = env.write_txn()?;

    let raw_events = env.create_database(&mut wtxn, Some("raw_events"))?;
    let episodes = env.create_database(&mut wtxn, Some("episodes"))?;
    let decisions = env.create_database(&mut wtxn, Some("decisions"))?;
    let tasks = env.create_database(&mut wtxn, Some("tasks"))?;
    let entities = env.create_database(&mut wtxn, Some("entities"))?;
    let summary_artifacts =
        env.create_database(&mut wtxn, Some("summary_artifacts"))?;
    let extraction_artifacts =
        env.create_database(&mut wtxn, Some("extraction_artifacts"))?;
    let embedding_artifacts =
        env.create_database(&mut wtxn, Some("embedding_artifacts"))?;
    let processing_jobs =
        env.create_database(&mut wtxn, Some("processing_jobs"))?;
    let projection_metadata =
        env.create_database(&mut wtxn, Some("projection_metadata"))?;
    let repo_config = env.create_database(&mut wtxn, Some("repo_config"))?;
    let retrieval_stats =
        env.create_database(&mut wtxn, Some("retrieval_stats"))?;
    let tool_sequences =
        env.create_database(&mut wtxn, Some("tool_sequences"))?;
    let recall_engagements =
        env.create_database(&mut wtxn, Some("recall_engagements"))?;
    let repo_profiles =
        env.create_database(&mut wtxn, Some("repo_profiles"))?;
    let metadata = env.create_database(&mut wtxn, Some("metadata"))?;

    wtxn.commit()?;

    Ok(LobsterDb {
        env: env.clone(),
        raw_events,
        episodes,
        decisions,
        tasks,
        entities,
        summary_artifacts,
        extraction_artifacts,
        embedding_artifacts,
        processing_jobs,
        projection_metadata,
        repo_config,
        retrieval_stats,
        tool_sequences,
        recall_engagements,
        repo_profiles,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let (db, _dir) = open_in_memory().expect("in-memory db");
        let rtxn = db.env.read_txn().expect("begin read");
        assert!(db.raw_events.get(&rtxn, &0u64).expect("get").is_none());
    }

    #[test]
    fn test_open_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = open(dir.path()).expect("open file db");

        // Write something, verify persistence
        {
            let mut wtxn = db.env.write_txn().expect("write");
            db.metadata
                .put(&mut wtxn, "schema_version", b"1")
                .expect("insert");
            wtxn.commit().expect("commit");
        }

        // Read back
        let rtxn = db.env.read_txn().expect("read");
        let val = db.metadata.get(&rtxn, "schema_version").expect("get");
        assert!(val.is_some());
        assert_eq!(val.unwrap(), b"1");
    }

    #[test]
    fn test_reopen_persists() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Write and close
        {
            let db = open(dir.path()).expect("open");
            let mut wtxn = db.env.write_txn().expect("write");
            db.metadata
                .put(&mut wtxn, "test_key", b"test_val")
                .expect("insert");
            wtxn.commit().expect("commit");
        }

        // Reopen and verify
        let db2 = open(dir.path()).expect("reopen");
        let rtxn = db2.env.read_txn().expect("read");
        let val = db2.metadata.get(&rtxn, "test_key").expect("get");
        assert_eq!(val.unwrap(), b"test_val");
    }

    #[test]
    fn test_open_snapshot_reads_committed_data() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Populate and close the writer so data.mdb is flushed.
        {
            let db = open(dir.path()).expect("open");
            let mut wtxn = db.env.write_txn().expect("write");
            db.metadata
                .put(&mut wtxn, "schema_version", b"1")
                .expect("put");
            wtxn.commit().expect("commit");
        }

        let (ro, _guard) = open_snapshot(dir.path()).expect("snapshot");
        let rtxn = ro.env.read_txn().expect("read");
        let val = ro.metadata.get(&rtxn, "schema_version").expect("get");
        assert_eq!(val, Some(b"1".as_slice()));
    }

    #[test]
    fn test_open_snapshot_missing_store_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Never initialized — data.mdb does not exist.
        let result = open_snapshot(dir.path());
        assert!(result.is_err(), "snapshot of uninitialized dir must fail");
    }

    #[test]
    fn test_open_snapshot_coexists_with_live_writer() {
        // Exercises the fix for `lobster status` lock contention:
        // opening a snapshot while a writer is attached to the live
        // store must succeed, because the snapshot lives at a
        // separate path.
        let dir = tempfile::tempdir().expect("tempdir");
        let writer = open(dir.path()).expect("writer");

        // Commit something so the snapshot has visible data.
        {
            let mut wtxn = writer.env.write_txn().expect("write");
            writer.metadata.put(&mut wtxn, "k", b"v").expect("put");
            wtxn.commit().expect("commit");
        }

        let (ro, _guard) = open_snapshot(dir.path())
            .expect("snapshot while writer is attached");
        let rtxn = ro.env.read_txn().expect("read");
        let val = ro.metadata.get(&rtxn, "k").expect("get");
        assert_eq!(val, Some(b"v".as_slice()));
    }

    #[test]
    fn test_concurrent_read_write_across_threads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = std::sync::Arc::new(open(dir.path()).expect("open"));

        // Write initial data
        {
            let mut wtxn = db.env.write_txn().expect("write");
            db.raw_events.put(&mut wtxn, &0u64, b"event0").expect("put");
            wtxn.commit().expect("commit");
        }

        // Read from another thread while main thread could write
        let db2 = db;
        let handle = std::thread::spawn(move || {
            let rtxn = db2.env.read_txn().expect("read");
            let val = db2.raw_events.get(&rtxn, &0u64).expect("get");
            assert_eq!(val.unwrap(), b"event0");
        });
        handle.join().expect("thread join");
    }
}
