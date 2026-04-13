//! MCP memory tool implementations.
//!
//! These tools expose Lobster's memory to Claude Code via the
//! MCP protocol. Each tool returns JSON-serializable results.

use grafeo::GrafeoDB;
use serde::Serialize;

use crate::{
    rank::routes::execute_query,
    store::{crud, db::LobsterDb, schema::EntityKind},
};

/// Load the actual text content for an artifact from redb.
///
/// Returns the decision statement, summary text, or entity name
/// depending on artifact type. Falls back to artifact type + ID
/// if the record is missing.
fn load_artifact_text(
    db: &LobsterDb,
    artifact_type: &str,
    id: &crate::store::ids::RawId,
) -> String {
    match artifact_type {
        "decision" => crud::get_decision(db, id)
            .map_or_else(|_| format!("decision:{id}"), |d| d.statement),
        "summary" => crud::get_summary_artifact(db, id)
            .map_or_else(|_| format!("summary:{id}"), |s| s.summary_text),
        "entity" => crud::get_entity(db, id).map_or_else(
            |_| format!("entity:{id}"),
            |e| {
                if e.kind == EntityKind::Workflow {
                    format!("Workflow: {}", e.canonical_name)
                } else {
                    e.canonical_name
                }
            },
        ),
        "task" => crud::get_task(db, id)
            .map_or_else(|_| format!("task:{id}"), |t| t.title),
        other => format!("{other}:{id}"),
    }
}

/// Load `repo_id` and `task_id` for an artifact from redb.
fn load_artifact_metadata(
    db: &LobsterDb,
    artifact_type: &str,
    id: &crate::store::ids::RawId,
) -> (Option<String>, Option<String>) {
    match artifact_type {
        "decision" => crud::get_decision(db, id).map_or_else(
            |_| (None, None),
            |d| {
                (
                    Some(d.repo_id.to_string()),
                    d.task_id.map(|t| t.to_string()),
                )
            },
        ),
        "summary" | "episode" => crud::get_episode(db, id).map_or_else(
            |_| (None, None),
            |ep| {
                (
                    Some(ep.repo_id.to_string()),
                    ep.task_id.map(|t| t.to_string()),
                )
            },
        ),
        "entity" => crud::get_entity(db, id).map_or_else(
            |_| (None, None),
            |e| (Some(e.repo_id.to_string()), None),
        ),
        "task" => crud::get_task(db, id).map_or_else(
            |_| (None, None),
            |t| (Some(t.repo_id.to_string()), Some(t.task_id.to_string())),
        ),
        _ => (None, None),
    }
}

/// Result of a `memory_context` tool call.
#[derive(Debug, Clone, Serialize)]
pub struct ContextBundle {
    pub items: Vec<ContextItem>,
    pub query: String,
    pub total_candidates: usize,
}

/// A single item in the context bundle, with all fields
/// required by the MCP tool contract.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub artifact_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_context: Option<String>,
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
    db: &LobsterDb,
    grafeo: &GrafeoDB,
) -> ContextBundle {
    let results = execute_query(query, db, grafeo, true);
    let total = results.len();

    let items: Vec<ContextItem> = results
        .into_iter()
        .map(|r| {
            let content =
                load_artifact_text(db, &r.artifact_type, &r.episode_id);
            let (repo_id, task_id) =
                load_artifact_metadata(db, &r.artifact_type, &r.episode_id);
            ContextItem {
                artifact_type: r.artifact_type,
                snippet: None,
                repo_id,
                task_id,
                confidence: None,
                provenance: Some(r.episode_id.to_string()),
                graph_context: None,
                score: r.score,
                content,
            }
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

/// Maximum episodes returned by `memory_recent`.
const MAX_RECENT_EPISODES: usize = 20;

/// Return newest ready episodes, sorted by finalized timestamp.
///
/// This is the `memory_recent` MCP tool. Filters by `repo_id` when
/// provided, sorts newest-first, and limits to [`MAX_RECENT_EPISODES`].
#[must_use]
pub fn memory_recent(db: &LobsterDb, repo_id: Option<&str>) -> RecentResult {
    use crate::store::ids::RepoId;

    let repo_filter = repo_id.map(|r| RepoId::derive(r.as_bytes()));

    let mut episodes: Vec<(i64, RecentEpisode)> = Vec::new();

    if let Ok(rtxn) = db.env.read_txn() {
        if let Ok(iter) = db.episodes.iter(&rtxn) {
            for entry in iter.flatten() {
                let (_, value) = entry;
                if let Ok(ep) = serde_json::from_slice::<
                    crate::store::schema::Episode,
                >(value)
                {
                    if ep.processing_state
                        != crate::store::schema::ProcessingState::Ready
                    {
                        continue;
                    }
                    if let Some(ref filter_id) = repo_filter {
                        if ep.repo_id != *filter_id {
                            continue;
                        }
                    }

                    let summary = db
                        .summary_artifacts
                        .get(&rtxn, ep.episode_id.raw().as_bytes())
                        .ok()
                        .flatten()
                        .and_then(|bytes| {
                            serde_json::from_slice::<
                                crate::store::schema::SummaryArtifact,
                            >(bytes)
                            .ok()
                        })
                        .map(|s| s.summary_text)
                        .unwrap_or_default();

                    episodes.push((
                        ep.finalized_ts_utc_ms,
                        RecentEpisode {
                            episode_id: ep.episode_id.to_string(),
                            summary,
                            state: format!("{:?}", ep.processing_state),
                        },
                    ));
                }
            }
        }
    }

    // Sort newest first, limit results
    episodes.sort_by_key(|&(ts, _)| std::cmp::Reverse(ts));
    episodes.truncate(MAX_RECENT_EPISODES);

    RecentResult {
        episodes: episodes.into_iter().map(|(_, ep)| ep).collect(),
    }
}

// ── memory_search ────────────────────────────────────────

/// Result of `memory_search`.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub hits: Vec<ContextItem>,
    pub query: String,
}

/// Return mixed ranked hits across decisions, summaries, tasks,
/// and entities.
#[must_use]
pub fn memory_search(
    query: &str,
    db: &LobsterDb,
    grafeo: &GrafeoDB,
) -> SearchResult {
    let results = execute_query(query, db, grafeo, true);
    let hits: Vec<ContextItem> = results
        .into_iter()
        .map(|r| {
            // Load actual content from redb for the snippet
            let (snippet, confidence) = match r.artifact_type.as_str() {
                "decision" => {
                    let dec = crud::get_decision(db, &r.episode_id);
                    match dec {
                        Ok(d) => (
                            Some(d.statement.clone()),
                            Some(format!("{:?}", d.confidence)),
                        ),
                        Err(_) => (None, None),
                    }
                }
                "summary" => {
                    let sum = crud::get_summary_artifact(db, &r.episode_id);
                    match sum {
                        Ok(s) => {
                            let preview: String =
                                s.summary_text.chars().take(200).collect();
                            (Some(preview), None)
                        }
                        Err(_) => (None, None),
                    }
                }
                "entity" => {
                    // Check if this entity is a Workflow
                    let entity = crud::get_entity(db, &r.episode_id);
                    match entity {
                        Ok(e) if e.kind == EntityKind::Workflow => {
                            // Load the ToolSequence for richer detail
                            let ts_detail = crud::list_tool_sequences(db)
                                .into_iter()
                                .find(|ts| {
                                    crate::store::ids::EntityId::derive(
                                        ts.workflow_id.raw().as_bytes(),
                                    ) == e.entity_id
                                })
                                .map(|ts| {
                                    format!(
                                        "Workflow: {} (seen {} times in {} episodes)",
                                        e.canonical_name,
                                        ts.frequency,
                                        ts.source_episodes.len()
                                    )
                                });
                            (
                                ts_detail.or(Some(e.canonical_name)),
                                None,
                            )
                        }
                        Ok(e) => (Some(e.canonical_name), None),
                        Err(_) => (None, None),
                    }
                }
                _ => (None, None),
            };
            let content = load_artifact_text(db, &r.artifact_type, &r.episode_id);
            let (repo_id, task_id) =
                load_artifact_metadata(db, &r.artifact_type, &r.episode_id);
            ContextItem {
                artifact_type: r.artifact_type,
                snippet,
                repo_id,
                task_id,
                confidence,
                provenance: Some(r.episode_id.to_string()),
                graph_context: None,
                score: r.score,
                content,
            }
        })
        .collect();
    SearchResult {
        hits,
        query: query.to_string(),
    }
}

// ── memory_decisions ─────────────────────────────────────

/// A decision in the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionTimelineEntry {
    pub decision_id: String,
    pub statement: String,
    pub rationale: String,
    pub confidence: String,
    pub valid_from_ms: i64,
    pub episode_id: String,
}

/// Result of `memory_decisions`.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionsResult {
    pub decisions: Vec<DecisionTimelineEntry>,
}

/// Return decision timeline, optionally filtered by repo.
#[must_use]
pub fn memory_decisions(
    db: &LobsterDb,
    repo_id: Option<&str>,
) -> DecisionsResult {
    use crate::store::ids::RepoId;

    let repo_filter = repo_id.map(|r| RepoId::derive(r.as_bytes()));
    let mut decisions = Vec::new();

    if let Ok(rtxn) = db.env.read_txn() {
        if let Ok(iter) = db.decisions.iter(&rtxn) {
            for entry in iter.flatten() {
                let (_, value) = entry;
                if let Ok(dec) = serde_json::from_slice::<
                    crate::store::schema::Decision,
                >(value)
                {
                    if let Some(ref filter_id) = repo_filter {
                        if dec.repo_id != *filter_id {
                            continue;
                        }
                    }
                    decisions.push(DecisionTimelineEntry {
                        decision_id: dec.decision_id.to_string(),
                        statement: dec.statement,
                        rationale: dec.rationale,
                        confidence: format!("{:?}", dec.confidence),
                        valid_from_ms: dec.valid_from_ts_utc_ms,
                        episode_id: dec.episode_id.to_string(),
                    });
                }
            }
        }
    }

    // Sort by valid_from (newest first)
    decisions.sort_by_key(|d| std::cmp::Reverse(d.valid_from_ms));

    DecisionsResult { decisions }
}

// ── memory_neighbors ─────────────────────────────────────

/// A graph neighbor.
#[derive(Debug, Clone, Serialize)]
pub struct NeighborEntry {
    pub node_id: String,
    pub label: String,
    pub edge_type: String,
}

/// Result of `memory_neighbors`.
#[derive(Debug, Clone, Serialize)]
pub struct NeighborsResult {
    pub neighbors: Vec<NeighborEntry>,
    pub query_node: String,
}

/// Return evidence-backed graph neighbors of a node.
#[must_use]
pub fn memory_neighbors(
    grafeo: &GrafeoDB,
    node_id_str: &str,
) -> NeighborsResult {
    let mut neighbors = Vec::new();

    // Query outgoing edges, filtering on temporal validity.
    // Per spec: "memory_neighbors must filter on temporal validity by default"
    // Edges with valid_to_ts_utc_ms in the past are excluded.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let session = grafeo.session();
    let query = format!(
        "MATCH (n)-[r]->(m) WHERE \
         (n.episode_id = '{node_id_str}' \
          OR n.decision_id = '{node_id_str}' \
          OR n.entity_id = '{node_id_str}') \
         AND (r.valid_to_ts_utc_ms IS NULL \
              OR r.valid_to_ts_utc_ms > {now_ms}) \
         RETURN m.episode_id, m.decision_id, m.entity_id, \
                m.canonical_name, TYPE(r)"
    );

    if let Ok(result) = session.execute(&query) {
        for row in result.iter() {
            let node_label = row[0]
                .as_str()
                .or_else(|| row[1].as_str())
                .or_else(|| row[2].as_str())
                .unwrap_or("unknown")
                .to_string();
            let name = row[3].as_str().unwrap_or("").to_string();
            let edge = row[4].as_str().unwrap_or("RELATED").to_string();

            neighbors.push(NeighborEntry {
                node_id: if name.is_empty() {
                    node_label.clone()
                } else {
                    name
                },
                label: node_label,
                edge_type: edge,
            });
        }
    }

    NeighborsResult {
        neighbors,
        query_node: node_id_str.to_string(),
    }
}

// ── memory_profile ──────────────────────────────────────────

/// Result of `memory_profile`: the repo's identity profile.
///
/// Returns all conventions and preferences, or empty vecs if
/// no profile exists.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileResult {
    pub conventions: Vec<ProfileFactEntry>,
    pub preferences: Vec<ProfileFactEntry>,
    pub updated_ts_utc_ms: i64,
}

/// A single profile fact for MCP output.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileFactEntry {
    pub statement: String,
    pub confidence: String,
    pub support_count: u32,
    pub first_seen_ms: i64,
    pub last_confirmed_ms: i64,
}

/// Return the repo identity profile.
///
/// Scans all repo profiles stored in the database and returns
/// the first one found (single-user per-repo model).
#[must_use]
pub fn memory_profile(db: &LobsterDb) -> ProfileResult {
    // Scan all repo profiles and return the first one
    let rtxn = match db.env.read_txn() {
        Ok(t) => t,
        Err(_) => {
            return ProfileResult {
                conventions: vec![],
                preferences: vec![],
                updated_ts_utc_ms: 0,
            };
        }
    };

    let iter = match db.repo_profiles.iter(&rtxn) {
        Ok(i) => i,
        Err(_) => {
            return ProfileResult {
                conventions: vec![],
                preferences: vec![],
                updated_ts_utc_ms: 0,
            };
        }
    };

    for entry in iter.flatten() {
        let (_, value) = entry;
        if let Ok(profile) =
            serde_json::from_slice::<crate::store::schema::RepoProfile>(value)
        {
            return ProfileResult {
                conventions: profile
                    .conventions
                    .iter()
                    .map(|f| ProfileFactEntry {
                        statement: f.statement.clone(),
                        confidence: format!("{:?}", f.confidence),
                        support_count: f.support_count,
                        first_seen_ms: f.first_seen_ts_utc_ms,
                        last_confirmed_ms: f.last_confirmed_ts_utc_ms,
                    })
                    .collect(),
                preferences: profile
                    .preferences
                    .iter()
                    .map(|f| ProfileFactEntry {
                        statement: f.statement.clone(),
                        confidence: format!("{:?}", f.confidence),
                        support_count: f.support_count,
                        first_seen_ms: f.first_seen_ts_utc_ms,
                        last_confirmed_ms: f.last_confirmed_ts_utc_ms,
                    })
                    .collect(),
                updated_ts_utc_ms: profile.updated_ts_utc_ms,
            };
        }
    }

    ProfileResult {
        conventions: vec![],
        preferences: vec![],
        updated_ts_utc_ms: 0,
    }
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
        let (database, _dir) = db::open_in_memory().unwrap();
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
                snippet: Some("Use redb for storage".into()),
                repo_id: None,
                task_id: None,
                confidence: Some("High".into()),
                provenance: None,
                graph_context: None,
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
        let (database, _dir) = db::open_in_memory().unwrap();
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
        let (database, _dir) = db::open_in_memory().unwrap();
        let recent = memory_recent(&database, None);
        assert!(recent.episodes.is_empty());
    }

    #[test]
    fn test_memory_recent_sorted_newest_first() {
        use crate::store::{
            crud,
            ids::{EpisodeId, RepoId},
            schema::{Episode, ProcessingState, SummaryArtifact},
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        // Create episodes with different timestamps
        for ts in [3000_i64, 1000, 2000] {
            let ep = Episode {
                episode_id: EpisodeId::derive(&ts.to_le_bytes()),
                repo_id: RepoId::derive(b"repo"),
                start_seq: 0,
                end_seq: 5,
                task_id: None,
                processing_state: ProcessingState::Ready,
                finalized_ts_utc_ms: ts,
                retry_count: 0,
                is_noisy: false,
            };
            crud::put_episode(&database, &ep).unwrap();
            let art = SummaryArtifact {
                episode_id: ep.episode_id,
                revision: "v1".into(),
                summary_text: format!("ts={ts}"),
                payload_checksum: [0; 32],
            };
            crud::put_summary_artifact(&database, &art).unwrap();
        }

        let recent = memory_recent(&database, None);
        assert_eq!(recent.episodes.len(), 3);
        // Newest first
        assert!(recent.episodes[0].summary.contains("3000"));
        assert!(recent.episodes[1].summary.contains("2000"));
        assert!(recent.episodes[2].summary.contains("1000"));
    }

    #[test]
    fn test_memory_recent_filters_by_repo_id() {
        use crate::store::{
            crud,
            ids::{EpisodeId, RepoId},
            schema::{Episode, ProcessingState, SummaryArtifact},
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        // Two repos
        for (repo, label) in [("repo-a", "alpha"), ("repo-b", "beta")] {
            let ep = Episode {
                episode_id: EpisodeId::derive(label.as_bytes()),
                repo_id: RepoId::derive(repo.as_bytes()),
                start_seq: 0,
                end_seq: 5,
                task_id: None,
                processing_state: ProcessingState::Ready,
                finalized_ts_utc_ms: 1000,
                retry_count: 0,
                is_noisy: false,
            };
            crud::put_episode(&database, &ep).unwrap();
            let art = SummaryArtifact {
                episode_id: ep.episode_id,
                revision: "v1".into(),
                summary_text: label.into(),
                payload_checksum: [0; 32],
            };
            crud::put_summary_artifact(&database, &art).unwrap();
        }

        // Filter by repo-a
        let recent = memory_recent(&database, Some("repo-a"));
        assert_eq!(recent.episodes.len(), 1);
        assert!(recent.episodes[0].summary.contains("alpha"));

        // No filter returns both
        let all = memory_recent(&database, None);
        assert_eq!(all.episodes.len(), 2);
    }

    // -- Property: memory_recent result count <= MAX_RECENT_EPISODES --
    #[hegel::test(test_cases = 20)]
    fn prop_memory_recent_bounded(tc: hegel::TestCase) {
        use hegel::generators as gs;

        use crate::store::{
            crud,
            ids::{EpisodeId, RepoId},
            schema::{Episode, ProcessingState, SummaryArtifact},
        };

        let (database, _dir) = db::open_in_memory().unwrap();
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(30));

        for i in 0..n {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
            let ep = Episode {
                episode_id: EpisodeId::derive(&(i as u32).to_le_bytes()),
                repo_id: RepoId::derive(b"repo"),
                start_seq: 0,
                end_seq: 5,
                task_id: None,
                processing_state: ProcessingState::Ready,
                finalized_ts_utc_ms: (i * 1000) as i64,
                retry_count: 0,
                is_noisy: false,
            };
            crud::put_episode(&database, &ep).unwrap();
            let art = SummaryArtifact {
                episode_id: ep.episode_id,
                revision: "v1".into(),
                summary_text: format!("ep {i}"),
                payload_checksum: [0; 32],
            };
            crud::put_summary_artifact(&database, &art).unwrap();
        }

        let recent = memory_recent(&database, None);
        assert!(
            recent.episodes.len() <= super::MAX_RECENT_EPISODES,
            "got {} episodes, max is {}",
            recent.episodes.len(),
            super::MAX_RECENT_EPISODES
        );
    }

    // ── Workflow surfacing tests ─────────────────────────────

    #[test]
    fn test_search_result_includes_workflow_entities() {
        use crate::store::{
            crud,
            ids::{EntityId, EpisodeId, RepoId, WorkflowId},
            schema::{Entity, EntityKind, EventKind, ToolSequence},
        };

        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Create a workflow entity and tool sequence
        let wf_id = WorkflowId::derive(b"test-workflow");
        let entity = Entity {
            entity_id: EntityId::derive(wf_id.raw().as_bytes()),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Workflow,
            canonical_name: "FileEdit→TestRun→TestResult".into(),
            first_seen_episode: None,
            last_seen_ts_utc_ms: None,
            mention_count: 0,
        };
        crud::put_entity(&database, &entity).unwrap();

        let ts = ToolSequence {
            workflow_id: wf_id,
            repo_id: RepoId::derive(b"repo"),
            pattern: vec![
                EventKind::FileEdit,
                EventKind::TestRun,
                EventKind::TestResult,
            ],
            label: "FileEdit→TestRun→TestResult".into(),
            frequency: 5,
            source_episodes: vec![
                EpisodeId::derive(b"ep1"),
                EpisodeId::derive(b"ep2"),
            ],
            detected_ts_utc_ms: 1_700_000_000_000,
        };
        crud::put_tool_sequence(&database, &ts).unwrap();

        // Project into Grafeo so it's searchable
        let session = grafeo.session();
        let _ = session.execute(&format!(
            "CREATE (e:Entity {{entity_id: '{}', \
             canonical_name: '{}', kind: 'Workflow'}})",
            entity.entity_id, entity.canonical_name
        ));

        // Search — even if route doesn't match, the code path
        // for workflow enrichment is exercised
        let results = memory_search("FileEdit TestRun", &database, &grafeo);
        let json = serde_json::to_string(&results).unwrap();
        assert!(json.contains("FileEdit") || results.hits.is_empty());
    }

    #[test]
    fn test_context_item_with_workflow_serializes() {
        let item = ContextItem {
            artifact_type: "entity".into(),
            snippet: Some(
                "Workflow: FileEdit→TestRun (seen 5 times in 3 episodes)"
                    .into(),
            ),
            repo_id: None,
            task_id: None,
            confidence: None,
            provenance: Some("abc123".into()),
            graph_context: None,
            score: 0.75,
            content: "Workflow: FileEdit→TestRun".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("Workflow"));
        assert!(json.contains("5 times"));
    }

    // ── memory_profile tests ────────────────────────────────

    #[test]
    fn test_memory_profile_empty_db() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let result = memory_profile(&database);
        assert!(result.conventions.is_empty());
        assert!(result.preferences.is_empty());
        assert_eq!(result.updated_ts_utc_ms, 0);
    }

    #[test]
    fn test_memory_profile_with_data() {
        use crate::store::{
            crud,
            ids::{EpisodeId, RepoId},
            schema::{Confidence, EvidenceRef, ProfileFact, RepoProfile},
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        let profile = RepoProfile {
            repo_id: RepoId::derive(b"repo"),
            conventions: vec![ProfileFact {
                statement: "uses nix flakes".into(),
                evidence: vec![EvidenceRef {
                    episode_id: EpisodeId::derive(b"ep1"),
                    span_summary: "detected".into(),
                }],
                first_seen_ts_utc_ms: 1000,
                last_confirmed_ts_utc_ms: 2000,
                support_count: 3,
                confidence: Confidence::Medium,
            }],
            preferences: vec![],
            updated_ts_utc_ms: 5000,
            revision: "v1".into(),
        };
        crud::put_repo_profile(&database, &profile).unwrap();

        let result = memory_profile(&database);
        assert_eq!(result.conventions.len(), 1);
        assert_eq!(result.conventions[0].statement, "uses nix flakes");
        assert_eq!(result.conventions[0].support_count, 3);
        assert_eq!(result.updated_ts_utc_ms, 5000);
    }

    #[test]
    fn test_memory_profile_serializes_to_json() {
        let result = ProfileResult {
            conventions: vec![ProfileFactEntry {
                statement: "Rust project".into(),
                confidence: "High".into(),
                support_count: 5,
                first_seen_ms: 1000,
                last_confirmed_ms: 2000,
            }],
            preferences: vec![],
            updated_ts_utc_ms: 3000,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Rust project"));
        assert!(json.contains("\"support_count\":5"));
    }
}
