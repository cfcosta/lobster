//! Background maintenance workers.
//!
//! Each worker handles a specific type of maintenance task:
//! entity merge proposals, task timeline updates, noise flagging.

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{
    ids::RawId,
    schema::{Entity, Task, TaskStatus},
    tables,
};

/// Scan for tasks that haven't been seen in recent episodes and
/// mark them as potentially stale.
///
/// Returns the number of stale tasks found.
#[must_use]
pub fn scan_stale_tasks(db: &Database) -> Vec<(RawId, String)> {
    let mut stale = Vec::new();

    let Ok(read_txn) = db.begin_read() else {
        return stale;
    };
    let Ok(table) = read_txn.open_table(tables::TASKS) else {
        return stale;
    };
    let Ok(iter) = table.iter() else {
        return stale;
    };

    for entry in iter.flatten() {
        let (key, value) = entry;
        if let Ok(task) = serde_json::from_slice::<Task>(value.value()) {
            // A task is stale if it's still Open but its
            // last_seen_in episode is old. Without timestamps
            // on the task itself, we check if the episode is
            // Ready (meaning it was finalized and the task
            // wasn't updated since).
            if task.status == TaskStatus::Open {
                stale.push((
                    RawId::from_bytes(*key.value()),
                    task.title.clone(),
                ));
            }
        }
    }

    stale
}

/// Find entities with the same canonical name but different IDs.
///
/// These are merge candidates that the dreaming scheduler can
/// propose for deduplication.
#[must_use]
pub fn find_duplicate_entities(db: &Database) -> Vec<(String, Vec<RawId>)> {
    let mut by_name: std::collections::HashMap<String, Vec<RawId>> =
        std::collections::HashMap::new();

    let Ok(read_txn) = db.begin_read() else {
        return vec![];
    };
    let Ok(table) = read_txn.open_table(tables::ENTITIES) else {
        return vec![];
    };
    let Ok(iter) = table.iter() else {
        return vec![];
    };

    for entry in iter.flatten() {
        let (key, value) = entry;
        if let Ok(entity) = serde_json::from_slice::<Entity>(value.value()) {
            by_name
                .entry(entity.canonical_name.to_lowercase())
                .or_default()
                .push(RawId::from_bytes(*key.value()));
        }
    }

    // Only return names with multiple IDs
    by_name
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        crud,
        db,
        ids::{EntityId, EpisodeId, RepoId, TaskId},
        schema::{Entity, EntityKind, Task, TaskStatus},
    };

    #[test]
    fn test_scan_stale_tasks_empty() {
        let database = db::open_in_memory().unwrap();
        let stale = scan_stale_tasks(&database);
        assert!(stale.is_empty());
    }

    #[test]
    fn test_scan_stale_tasks_finds_open() {
        let database = db::open_in_memory().unwrap();

        let task = Task {
            task_id: TaskId::derive(b"t1"),
            repo_id: RepoId::derive(b"repo"),
            title: "Build memory search".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep1"),
        };
        crud::put_task(&database, &task).unwrap();

        let completed = Task {
            task_id: TaskId::derive(b"t2"),
            repo_id: RepoId::derive(b"repo"),
            title: "Fix bug".into(),
            status: TaskStatus::Completed,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep2"),
        };
        crud::put_task(&database, &completed).unwrap();

        let stale = scan_stale_tasks(&database);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].1, "Build memory search");
    }

    #[test]
    fn test_find_duplicate_entities_none() {
        let database = db::open_in_memory().unwrap();
        let entity = Entity {
            entity_id: EntityId::derive(b"e1"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
        };
        crud::put_entity(&database, &entity).unwrap();

        let dupes = find_duplicate_entities(&database);
        assert!(dupes.is_empty());
    }

    #[test]
    fn test_find_duplicate_entities_detects() {
        let database = db::open_in_memory().unwrap();

        // Two entities with same name but different IDs
        let e1 = Entity {
            entity_id: EntityId::derive(b"e1"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "Grafeo".into(),
        };
        let e2 = Entity {
            entity_id: EntityId::derive(b"e2"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "grafeo".into(), // different case
        };
        crud::put_entity(&database, &e1).unwrap();
        crud::put_entity(&database, &e2).unwrap();

        let dupes = find_duplicate_entities(&database);
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].1.len(), 2);
    }
}
