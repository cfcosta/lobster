//! File-based event staging for lock-free hook capture.
//!
//! Hooks write events here as individual JSON files. The MCP server
//! (or standalone hook with DB access) drains them into redb.
//!
//! File naming: `{timestamp_nanos}_{random}.json` ensures ordering
//! and avoids collisions between concurrent hook processes.

use std::path::{Path, PathBuf};

/// Returns the staging directory path within the storage dir.
#[must_use]
pub fn staging_dir(storage_dir: &Path) -> PathBuf {
    storage_dir.join("staging")
}

/// Write a hook event JSON to the staging directory.
///
/// Creates the staging directory if it doesn't exist. Uses atomic
/// write (write to temp, then rename) to prevent partial reads.
///
/// # Errors
///
/// Returns an IO error if the write fails.
pub fn stage_event(
    storage_dir: &Path,
    event_json: &str,
) -> std::io::Result<PathBuf> {
    let dir = staging_dir(storage_dir);
    std::fs::create_dir_all(&dir)?;

    let name = generate_filename();
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!(".{name}.tmp"));

    // Atomic write: write to temp file, then rename
    std::fs::write(&tmp_path, event_json)?;
    std::fs::rename(&tmp_path, &final_path)?;

    Ok(final_path)
}

/// List all staged event files in chronological order.
///
/// Returns file paths sorted by name (which sorts chronologically
/// due to the timestamp prefix).
///
/// # Errors
///
/// Returns an IO error if the directory can't be read.
pub fn list_staged(storage_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dir = staging_dir(storage_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "json")
                && !p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with('.')
        })
        .collect();

    entries.sort();
    Ok(entries)
}

/// Read and remove a single staged event file.
///
/// Returns the JSON contents. The file is deleted after reading.
///
/// # Errors
///
/// Returns an IO error if the file can't be read or deleted.
pub fn consume_staged(path: &Path) -> std::io::Result<String> {
    let contents = std::fs::read_to_string(path)?;
    std::fs::remove_file(path)?;
    Ok(contents)
}

/// Drain all staged events, returning their JSON contents in order.
///
/// Each file is read and deleted atomically. If reading a file fails,
/// it's skipped (left for a future drain attempt).
///
/// # Errors
///
/// Returns an IO error only if listing the directory fails.
pub fn drain_staged(storage_dir: &Path) -> std::io::Result<Vec<String>> {
    let files = list_staged(storage_dir)?;
    let mut events = Vec::with_capacity(files.len());

    for path in files {
        match consume_staged(&path) {
            Ok(json) => events.push(json),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to consume staged event, will retry later"
                );
            }
        }
    }

    Ok(events)
}

/// Generate a unique, sortable filename.
///
/// Format: `{nanosecond_timestamp}_{random_u32}.json`
fn generate_filename() -> String {
    use std::time::SystemTime;

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Simple random suffix using thread-local state to avoid collisions
    let random: u32 = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        nanos.hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        // Mix in address of a stack variable for extra entropy
        let stack_var = 0u8;
        (std::ptr::addr_of!(stack_var) as u64).hash(&mut hasher);
        #[allow(clippy::cast_possible_truncation)]
        let result = hasher.finish() as u32;
        result
    };

    format!("{nanos:020}_{random:08x}.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        stage_event(storage, r#"{"event": "first"}"#).unwrap();
        stage_event(storage, r#"{"event": "second"}"#).unwrap();

        let files = list_staged(storage).unwrap();
        assert_eq!(files.len(), 2);
        // Should be in chronological order
        assert!(files[0] < files[1]);
    }

    #[test]
    fn test_stage_and_drain() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        stage_event(storage, r#"{"a": 1}"#).unwrap();
        stage_event(storage, r#"{"b": 2}"#).unwrap();

        let events = drain_staged(storage).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("\"a\""));
        assert!(events[1].contains("\"b\""));

        // Directory should be empty now
        let remaining = list_staged(storage).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_drain_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let events = drain_staged(dir.path()).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_drain_nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("nope");
        let events = drain_staged(&nonexistent).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_consume_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        let path = stage_event(storage, "test").unwrap();
        assert!(path.exists());

        let contents = consume_staged(&path).unwrap();
        assert_eq!(contents, "test");
        assert!(!path.exists());
    }

    #[test]
    fn test_tmp_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();
        let staging = staging_dir(storage);
        std::fs::create_dir_all(&staging).unwrap();

        // Write a normal file and a tmp file
        stage_event(storage, "real").unwrap();
        std::fs::write(staging.join(".partial.json.tmp"), "partial").unwrap();

        let files = list_staged(storage).unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_filename_is_sortable() {
        let a = generate_filename();
        // Small sleep to ensure different timestamp
        std::thread::sleep(std::time::Duration::from_millis(1));
        let b = generate_filename();
        assert!(a < b, "filenames should be chronologically sortable");
    }

    use hegel::{TestCase, generators as gs};

    /// Property: `stage_event` round-trips — anything staged can be
    /// drained back with identical content.
    #[hegel::test(test_cases = 50)]
    fn prop_stage_drain_roundtrip(tc: TestCase) {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        let json: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(500)
                .alphabet("abcdefghijklmnopqrstuvwxyz0123456789{}:,\" "),
        );

        stage_event(storage, &json).unwrap();
        let drained = drain_staged(storage).unwrap();

        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0], json);
    }

    /// Property: multiple staged events drain in order and all appear.
    #[hegel::test(test_cases = 30)]
    fn prop_stage_preserves_order(tc: TestCase) {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        let count: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(10));
        let mut originals = Vec::with_capacity(count);

        for i in 0..count {
            let json = format!(r#"{{"seq": {i}}}"#);
            stage_event(storage, &json).unwrap();
            originals.push(json);
            // Ensure distinct timestamps
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let drained = drain_staged(storage).unwrap();
        assert_eq!(drained.len(), count);
        assert_eq!(drained, originals);
    }
}
