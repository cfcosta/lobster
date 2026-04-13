//! Application configuration with convention-over-configuration
//! defaults.

use std::path::{Path, PathBuf};

/// Resolve the `.lobster` storage directory for a repo.
///
/// Walks up from `start_dir` looking for a `.lobster` directory.
/// If none found, returns `start_dir/.lobster`.
#[must_use]
pub fn resolve_storage_path(start_dir: &Path) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(".lobster");
        if candidate.is_dir() {
            return candidate;
        }
        if !dir.pop() {
            break;
        }
    }
    start_dir.join(".lobster")
}

/// Path to the LMDB database directory within the storage directory.
#[must_use]
pub fn db_path(storage_dir: &Path) -> PathBuf {
    storage_dir.join("lmdb")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_creates_default_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_storage_path(dir.path());
        assert_eq!(result, dir.path().join(".lobster"));
    }

    #[test]
    fn test_resolve_finds_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let lobster_dir = dir.path().join(".lobster");
        std::fs::create_dir(&lobster_dir).unwrap();

        let sub = dir.path().join("src");
        std::fs::create_dir(&sub).unwrap();

        let result = resolve_storage_path(&sub);
        assert_eq!(result, lobster_dir);
    }

    #[test]
    fn test_db_path() {
        let storage = PathBuf::from("/tmp/.lobster");
        assert_eq!(db_path(&storage), PathBuf::from("/tmp/.lobster/lmdb"));
    }
}
