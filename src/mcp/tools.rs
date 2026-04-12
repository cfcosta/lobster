//! MCP memory tool implementations.
//!
//! These tools expose Lobster's memory to Claude Code via the
//! MCP protocol. Each tool returns JSON-serializable results.

use grafeo::GrafeoDB;
use redb::Database;
use serde::Serialize;

use crate::{rank::routes::execute_query, store::crud};

/// Result of a `memory_context` tool call.
#[derive(Debug, Clone, Serialize)]
pub struct ContextBundle {
    pub items: Vec<ContextItem>,
    pub query: String,
    pub total_candidates: usize,
}

/// A single item in the context bundle.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub artifact_type: String,
    pub score: f64,
    pub content: String,
}

/// Assemble a compact task-oriented context bundle.
///
/// This is the `memory_context` MCP tool: returns the best
/// ranked decisions, summaries, tasks, and entities for the
/// current situation.
#[must_use]
pub fn memory_context(
    query: &str,
    db: &Database,
    grafeo: &GrafeoDB,
) -> ContextBundle {
    let results = execute_query(query, db, grafeo, true);
    let total = results.len();

    let items: Vec<ContextItem> = results
        .into_iter()
        .map(|r| ContextItem {
            artifact_type: r.artifact_type,
            score: r.score,
            content: format!("Retrieved via {:?} route", r.route),
        })
        .collect();

    ContextBundle {
        items,
        query: query.to_string(),
        total_candidates: total,
    }
}

/// Result of a `memory_recent` tool call.
#[derive(Debug, Clone, Serialize)]
pub struct RecentResult {
    pub episodes: Vec<RecentEpisode>,
}

/// A recent episode summary.
#[derive(Debug, Clone, Serialize)]
pub struct RecentEpisode {
    pub episode_id: String,
    pub summary: String,
    pub state: String,
}

/// Return newest ready artifacts.
///
/// This is the `memory_recent` MCP tool.
#[must_use]
pub fn memory_recent(db: &Database, _repo_id: Option<&str>) -> RecentResult {
    // For now, scan episodes table and return Ready ones
    // (a real implementation would use an index)
    use redb::{ReadableDatabase, ReadableTable};

    use crate::store::tables;

    let mut episodes = Vec::new();

    if let Ok(read_txn) = db.begin_read() {
        if let Ok(table) = read_txn.open_table(tables::EPISODES) {
            if let Ok(iter) = table.iter() {
                for entry in iter.flatten() {
                    let (_, value) = entry;
                    if let Ok(ep) = serde_json::from_slice::<
                        crate::store::schema::Episode,
                    >(value.value())
                    {
                        if ep.processing_state
                            == crate::store::schema::ProcessingState::Ready
                        {
                            // Try to load summary
                            let summary = crud::get_summary_artifact(
                                db,
                                &ep.episode_id.raw(),
                            )
                            .map(|s| s.summary_text)
                            .unwrap_or_default();

                            episodes.push(RecentEpisode {
                                episode_id: ep.episode_id.to_string(),
                                summary,
                                state: format!("{:?}", ep.processing_state),
                            });
                        }
                    }
                }
            }
        }
    }

    RecentResult { episodes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        episodes::finalize::{FinalizeResult, finalize_episode},
        graph::db as grafeo_db,
        store::db,
    };

    #[test]
    fn test_memory_context_empty_db() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let bundle = memory_context("test query", &database, &grafeo);
        assert!(bundle.items.is_empty());
        assert_eq!(bundle.query, "test query");
        assert_eq!(bundle.total_candidates, 0);
    }

    #[test]
    fn test_context_bundle_serializes() {
        let bundle = ContextBundle {
            items: vec![ContextItem {
                artifact_type: "decision".into(),
                score: 0.85,
                content: "Use redb".into(),
            }],
            query: "storage choice".into(),
            total_candidates: 1,
        };
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(json.contains("decision"));
        assert!(json.contains("0.85"));
    }

    #[tokio::test]
    async fn test_memory_recent_returns_ready_episodes() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Finalize an episode
        let result = finalize_episode(
            &database,
            &grafeo,
            "/repo",
            b"[]",
            0,
            5,
            Some("Test task".into()),
        )
        .await;
        assert!(matches!(result, FinalizeResult::Ready { .. }));

        let recent = memory_recent(&database, None);
        assert_eq!(recent.episodes.len(), 1, "should find the Ready episode");
        assert!(!recent.episodes[0].summary.is_empty());
    }

    #[test]
    fn test_memory_recent_empty_db() {
        let database = db::open_in_memory().unwrap();
        let recent = memory_recent(&database, None);
        assert!(recent.episodes.is_empty());
    }
}
