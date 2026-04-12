//! Cross-platform filesystem watcher for the staging directory.
//!
//! Uses the `notify` crate (`inotify` on Linux, `FSEvents` on macOS,
//! `ReadDirectoryChanges` on Windows) to detect new staged events
//! and signal the ingestion loop via a tokio channel.

use std::path::{Path, PathBuf};

use notify::{
    Event,
    EventKind,
    RecommendedWatcher,
    RecursiveMode,
    Watcher,
    event::CreateKind,
};
use tokio::sync::mpsc;

/// A notification that new staged files are available.
#[derive(Debug, Clone)]
pub struct StagingNotification {
    /// Paths of the new files (may be empty if we just know
    /// "something changed").
    pub paths: Vec<PathBuf>,
}

/// Spawn a filesystem watcher on the staging directory.
///
/// Returns a receiver that yields notifications whenever new `.json`
/// files appear in the staging directory. The watcher runs on a
/// background thread managed by `notify`.
///
/// # Errors
///
/// Returns an error if the watcher cannot be initialized or if the
/// staging directory cannot be watched.
pub fn watch_staging(
    storage_dir: &Path,
) -> Result<
    (mpsc::UnboundedReceiver<StagingNotification>, WatcherGuard),
    notify::Error,
> {
    let staging = super::staging::staging_dir(storage_dir);
    std::fs::create_dir_all(&staging).map_err(|e| {
        notify::Error::generic(&format!("create staging dir: {e}"))
    })?;

    let (tx, rx) = mpsc::unbounded_channel();

    let watcher = {
        notify::recommended_watcher(
            move |res: Result<Event, notify::Error>| {
                let Ok(event) = res else {
                    return;
                };

                // Only fire on file creation or modification — these
                // are the events that indicate a new staged file.
                let dominated = matches!(
                    event.kind,
                    EventKind::Create(CreateKind::File | CreateKind::Any)
                        | EventKind::Modify(_)
                );
                if !dominated {
                    return;
                }

                // Filter to .json files (ignore .tmp and other artifacts)
                let json_paths: Vec<PathBuf> = event
                    .paths
                    .into_iter()
                    .filter(|p| {
                        p.extension().is_some_and(|ext| ext == "json")
                            && !p
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .starts_with('.')
                    })
                    .collect();

                if json_paths.is_empty() {
                    return;
                }

                let _ = tx.send(StagingNotification { paths: json_paths });
            },
        )?
    };

    // Start watching
    let mut watcher = watcher;
    watcher.watch(&staging, RecursiveMode::NonRecursive)?;

    Ok((rx, WatcherGuard { _watcher: watcher }))
}

/// Guard that keeps the watcher alive. Drop it to stop watching.
pub struct WatcherGuard {
    _watcher: RecommendedWatcher,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::staging;

    #[tokio::test]
    async fn test_watcher_detects_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        let (mut rx, _guard) = watch_staging(storage).unwrap();

        // Stage an event — the watcher should detect it
        staging::stage_event(storage, r#"{"test": true}"#).unwrap();

        // Wait for notification with timeout
        let notification =
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("timeout waiting for notification")
                .expect("channel closed");

        assert!(
            !notification.paths.is_empty(),
            "should have detected the new file"
        );
        assert!(
            notification.paths[0]
                .extension()
                .is_some_and(|ext| ext == "json")
        );
    }

    #[tokio::test]
    async fn test_watcher_ignores_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();
        let staging_path = staging::staging_dir(storage);
        std::fs::create_dir_all(&staging_path).unwrap();

        let (mut rx, _guard) = watch_staging(storage).unwrap();

        // Write a dot-prefixed temp file (like staging's atomic write tmp)
        std::fs::write(staging_path.join(".temp_event.json"), "tmp").unwrap();

        // Then write a real .json file — SHOULD trigger
        staging::stage_event(storage, r#"{"real": true}"#).unwrap();

        // Collect all notifications for a bit
        let mut json_paths = Vec::new();
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                rx.recv(),
            )
            .await
            {
                Ok(Some(n)) => json_paths.extend(n.paths),
                _ => break,
            }
        }

        // All received paths should be non-dotfiles
        for p in &json_paths {
            let name = p.file_name().unwrap().to_string_lossy();
            assert!(
                !name.starts_with('.'),
                "should not include dotfiles, got: {name}"
            );
        }
        assert!(
            !json_paths.is_empty(),
            "should have detected the real json file"
        );
    }

    #[tokio::test]
    async fn test_watcher_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path();

        let (mut rx, _guard) = watch_staging(storage).unwrap();

        // Stage several events
        for i in 0..3 {
            staging::stage_event(storage, &format!(r#"{{"seq": {i}}}"#))
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Collect notifications for a bit
        let mut total_paths = 0;
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                rx.recv(),
            )
            .await
            {
                Ok(Some(n)) => total_paths += n.paths.len(),
                _ => break,
            }
        }

        assert!(
            total_paths >= 3,
            "should have detected at least 3 files, got {total_paths}"
        );
    }
}
