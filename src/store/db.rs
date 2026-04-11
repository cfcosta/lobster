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
    use redb::ReadableDatabase;

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
}
