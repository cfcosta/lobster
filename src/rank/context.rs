//! Retrieval context: current task detection and scoring helpers.

use redb::{Database, ReadableDatabase, ReadableTable};

use crate::store::{
    ids::{RawId, RepoId, TaskId},
    schema::Task,
    tables,
};

/// Find the most recently seen open task for a repo.
///
/// Scans the tasks table and returns the task whose `last_seen_in`
/// is most recent. This is the "current task context" used for
/// `task_overlap_score`.
#[must_use]
pub fn find_current_task(db: &Database, repo_id: &RepoId) -> Option<TaskId> {
    let read_txn = db.begin_read().ok()?;
    let table = read_txn.open_table(tables::TASKS).ok()?;
    let iter = table.iter().ok()?;

    let mut best: Option<Task> = None;

    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(task) = serde_json::from_slice::<Task>(value.value()) {
            if task.repo_id == *repo_id
                && task.status == crate::store::schema::TaskStatus::Open
            {
                match &best {
                    None => best = Some(task),
                    Some(existing) => {
                        // Compare by last_seen_in — higher
                        // episode IDs are more recent (they're
                        // derived from seq numbers)
                        if task.last_seen_in > existing.last_seen_in {
                            best = Some(task);
                        }
                    }
                }
            }
        }
    }

    best.map(|t| t.task_id)
}

/// Compute `task_overlap_score` for a candidate.
///
/// Returns 1.0 if the candidate's episode has the same `task_id`
/// as the current context, 0.0 otherwise.
#[must_use]
pub fn task_overlap(
    db: &Database,
    candidate_id: &RawId,
    current_task: Option<&TaskId>,
) -> f64 {
    let Some(current) = current_task else {
        return 0.0;
    };

    // Check if the candidate's episode has this task
    if let Ok(ep) = crate::store::crud::get_episode(db, candidate_id) {
        if let Some(task_id) = &ep.task_id {
            if task_id == current {
                return 1.0;
            }
        }
    }

    0.0
}

/// Compute `graph_support_score` for a candidate.
///
/// Returns the fraction of the candidate's Grafeo neighbors that
/// also appear in the candidate set. Only meaningful for
/// `HybridGraph` route.
#[must_use]
pub fn graph_support(
    grafeo: &grafeo::GrafeoDB,
    candidate_id: &str,
    candidate_set: &[RawId],
) -> f64 {
    let session = grafeo.session();
    let query = format!(
        "MATCH (n)-[]->(m) WHERE n.episode_id = '{candidate_id}' \
         OR n.decision_id = '{candidate_id}' \
         RETURN m.episode_id, m.decision_id, m.entity_id"
    );

    let Ok(result) = session.execute(&query) else {
        return 0.0;
    };

    let mut total_neighbors = 0u64;
    let mut in_candidate_set = 0u64;

    for row in result.iter() {
        for col in &row[..3] {
            if let Some(id_str) = col.as_str() {
                if let Ok(raw_id) = id_str.parse::<RawId>() {
                    total_neighbors += 1;
                    if candidate_set.contains(&raw_id) {
                        in_candidate_set += 1;
                    }
                }
            }
        }
    }

    if total_neighbors == 0 {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let score = in_candidate_set as f64 / total_neighbors as f64;
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        crud,
        db,
        ids::{EpisodeId, RepoId, TaskId},
        schema::{Task, TaskStatus},
    };

    #[test]
    fn test_find_current_task_empty() {
        let database = db::open_in_memory().unwrap();
        let repo = RepoId::derive(b"repo");
        assert!(find_current_task(&database, &repo).is_none());
    }

    #[test]
    fn test_find_current_task_returns_open() {
        let database = db::open_in_memory().unwrap();
        let repo = RepoId::derive(b"repo");

        let task = Task {
            task_id: TaskId::derive(b"t1"),
            repo_id: repo,
            title: "Build memory".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep2"),
        };
        crud::put_task(&database, &task).unwrap();

        let found = find_current_task(&database, &repo);
        assert_eq!(found, Some(TaskId::derive(b"t1")));
    }

    #[test]
    fn test_find_current_task_ignores_completed() {
        let database = db::open_in_memory().unwrap();
        let repo = RepoId::derive(b"repo");

        let completed = Task {
            task_id: TaskId::derive(b"t1"),
            repo_id: repo,
            title: "Done task".into(),
            status: TaskStatus::Completed,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep3"),
        };
        crud::put_task(&database, &completed).unwrap();

        assert!(find_current_task(&database, &repo).is_none());
    }

    #[test]
    fn test_task_overlap_no_context() {
        let database = db::open_in_memory().unwrap();
        let id = EpisodeId::derive(b"ep").raw();
        assert!((task_overlap(&database, &id, None)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_graph_support_empty() {
        let grafeo = crate::graph::db::new_in_memory();
        let score = graph_support(&grafeo, "nonexistent", &[]);
        assert!((score).abs() < f64::EPSILON);
    }
}
