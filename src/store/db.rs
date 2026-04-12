//! Database initialization and lifecycle management.
//!
//! Wraps redb with Lobster-specific setup: creates all tables on
//! first open and provides an in-memory backend for testing.

use std::path::Path;

use redb::{Database, backends::InMemoryBackend};

use crate::store::tables;

/// Open or create a Lobster database at the given path.
///
/// All tables are created lazily on first access, but we open them
/// once here to ensure the schema exists.
///
/// # Errors
///
/// Returns a redb error if the file cannot be created or opened.
pub fn open(path: &Path) -> Result<Database, redb::Error> {
    let db = Database::create(path)?;
    init_tables(&db)?;
    Ok(db)
}

/// Try to open the database, returning `None` if the file is locked
/// by another process (e.g., the MCP server).
///
/// This is used by hooks to opportunistically access redb when the
/// MCP server is not running, without blocking or crashing.
#[must_use]
pub fn try_open(path: &Path) -> Option<Database> {
    if !path.exists() {
        return None;
    }
    match Database::create(path) {
        Ok(db) => match init_tables(&db) {
            Ok(()) => Some(db),
            Err(e) => {
                tracing::debug!(error = %e, "try_open: init_tables failed");
                None
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, "try_open: database locked or error");
            None
        }
    }
}

/// Create an in-memory database for testing.
///
/// # Errors
///
/// Returns a redb error if the database cannot be initialized.
pub fn open_in_memory() -> Result<Database, redb::Error> {
    let backend = InMemoryBackend::new();
    let db = Database::builder().create_with_backend(backend)?;
    init_tables(&db)?;
    Ok(db)
}

/// Open a read-only snapshot of the database.
///
/// Tries `ReadOnlyDatabase` first. If the file is exclusively locked
/// (e.g. by the MCP server), copies it to a temp file, repairs it,
/// and opens the copy. Returns `None` if the file doesn't exist or
/// both approaches fail.
#[must_use]
pub fn open_snapshot(path: &Path) -> Option<Database> {
    if !path.exists() {
        return None;
    }

    // Fast path: try opening the copy directly with repair allowed
    let tmp = path.with_extension("redb.snapshot");
    if std::fs::copy(path, &tmp).is_err() {
        return None;
    }

    let result = redb::Builder::new().set_repair_callback(|_| {}).open(&tmp);

    match result {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::debug!(error = %e, "open_snapshot: failed to open copy");
            let _ = std::fs::remove_file(&tmp);
            None
        }
    }
}

/// Ensure all tables exist by opening each one in a single write
/// transaction.
fn init_tables(db: &Database) -> Result<(), redb::Error> {
    let write_txn = db.begin_write()?;
    write_txn.open_table(tables::RAW_EVENTS)?;
    write_txn.open_table(tables::EPISODES)?;
    write_txn.open_table(tables::DECISIONS)?;
    write_txn.open_table(tables::TASKS)?;
    write_txn.open_table(tables::ENTITIES)?;
    write_txn.open_table(tables::SUMMARY_ARTIFACTS)?;
    write_txn.open_table(tables::EXTRACTION_ARTIFACTS)?;
    write_txn.open_table(tables::EMBEDDING_ARTIFACTS)?;
    write_txn.open_table(tables::PROCESSING_JOBS)?;
    write_txn.open_table(tables::PROJECTION_METADATA)?;
    write_txn.open_table(tables::REPO_CONFIG)?;
    write_txn.open_table(tables::RETRIEVAL_STATS)?;
    write_txn.open_table(tables::METADATA)?;
    write_txn.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use redb::{ReadableDatabase, ReadableTable};

    use super::*;

    #[test]
    fn test_open_in_memory() {
        let db = open_in_memory().expect("in-memory db");
        // Verify we can read a table (get returns None for missing key)
        let read_txn = db.begin_read().expect("begin read");
        let table =
            read_txn.open_table(tables::RAW_EVENTS).expect("open table");
        assert!(table.get(0u64).expect("get").is_none());
    }

    #[test]
    fn test_open_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.redb");
        let db = open(&path).expect("open file db");

        // Write something, close, reopen
        {
            let write_txn = db.begin_write().expect("write");
            {
                let mut table =
                    write_txn.open_table(tables::METADATA).expect("open");
                table
                    .insert("schema_version", b"1".as_slice())
                    .expect("insert");
            }
            write_txn.commit().expect("commit");
        }
        drop(db);

        // Reopen and verify persistence
        let db2 = open(&path).expect("reopen");
        let read_txn = db2.begin_read().expect("read");
        let table = read_txn.open_table(tables::METADATA).expect("open");
        let val = table.get("schema_version").expect("get");
        assert!(val.is_some());
        assert_eq!(val.unwrap().value(), b"1");
    }

    #[test]
    fn test_try_open_nonexistent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.redb");
        assert!(try_open(&path).is_none());
    }

    #[test]
    fn test_try_open_unlocked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.redb");

        // Create the DB first, then close it
        {
            let _db = open(&path).expect("create");
        }

        // try_open should succeed on an unlocked file
        let db = try_open(&path);
        assert!(db.is_some());
    }

    #[test]
    fn test_try_open_locked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.redb");

        // Hold the DB open (simulating MCP server)
        let _holder = open(&path).expect("open");

        // try_open should fail because the file is locked
        let result = try_open(&path);
        assert!(
            result.is_none(),
            "try_open should return None when DB is locked"
        );
    }

    // ── open_snapshot ───────────────────────────────────────────

    #[test]
    fn test_open_snapshot_nonexistent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nope.redb");
        assert!(open_snapshot(&path).is_none());
    }

    #[test]
    fn test_open_snapshot_reads_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.redb");

        // Write data, then close
        {
            let db = open(&path).expect("create");
            let write_txn = db.begin_write().expect("write");
            {
                let mut table =
                    write_txn.open_table(tables::METADATA).expect("open");
                table
                    .insert("test_key", b"test_val".as_slice())
                    .expect("insert");
            }
            write_txn.commit().expect("commit");
        }

        // Snapshot should read the same data
        let snap = open_snapshot(&path).expect("snapshot");
        let read_txn = snap.begin_read().expect("read");
        let table = read_txn.open_table(tables::METADATA).expect("open");
        let val = table.get("test_key").expect("get");
        assert!(val.is_some());
        assert_eq!(val.unwrap().value(), b"test_val");
    }

    use hegel::{TestCase, generators as gs};

    /// `open_snapshot` on a populated DB produces a snapshot with
    /// the same number of raw events.
    #[hegel::test(test_cases = 20)]
    fn prop_open_snapshot_preserves_event_count(tc: TestCase) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");

        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(15));
        {
            let db = open(&path).unwrap();
            for seq in 0..n {
                let event = crate::store::schema::RawEvent {
                    seq: seq as u64,
                    repo_id: crate::store::ids::RepoId::derive(b"repo"),
                    ts_utc_ms: 1_700_000_000_000,
                    event_kind:
                        crate::store::schema::EventKind::UserPromptSubmit,
                    payload_hash: [0; 32],
                    payload_bytes: vec![],
                };
                crate::store::crud::append_raw_event(&db, &event).unwrap();
            }
        }

        let snap = open_snapshot(&path).expect("snapshot");
        let read_txn = snap.begin_read().unwrap();
        let table = read_txn.open_table(tables::RAW_EVENTS).unwrap();
        let count = table.iter().unwrap().count();
        assert_eq!(count, n);
    }

    /// `open_snapshot` works even when the original DB is locked.
    #[hegel::test(test_cases = 10)]
    fn prop_open_snapshot_while_locked(tc: TestCase) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");

        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(10));
        let db = open(&path).unwrap();
        for seq in 0..n {
            let event = crate::store::schema::RawEvent {
                seq: seq as u64,
                repo_id: crate::store::ids::RepoId::derive(b"repo"),
                ts_utc_ms: 1_700_000_000_000,
                event_kind: crate::store::schema::EventKind::UserPromptSubmit,
                payload_hash: [0; 32],
                payload_bytes: vec![],
            };
            crate::store::crud::append_raw_event(&db, &event).unwrap();
        }

        // DB is still open (locked) — snapshot should still work
        let snap = open_snapshot(&path).expect("snapshot while locked");
        let read_txn = snap.begin_read().unwrap();
        let table = read_txn.open_table(tables::RAW_EVENTS).unwrap();
        let count = table.iter().unwrap().count();
        assert_eq!(count, n);
    }
}
