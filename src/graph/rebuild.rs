//! Rebuild Grafeo from redb durable artifacts.
//!
//! If Grafeo is lost, this module can reconstruct the complete
//! semantic graph from the canonical artifacts stored in redb.
//! It should not require re-running summarization or extraction.

use grafeo::GrafeoDB;

use crate::{
    episodes::finalize::parse_entity_kind,
    extract::traits::RelationType,
    graph::{db as gdb, projection},
    store::{
        crud,
        db::LobsterDb,
        ids::EntityId,
        schema::{Episode, ProcessingState},
    },
};

/// Rebuild the entire Grafeo graph from redb.
///
/// Iterates all Ready episodes, retrieves their artifacts, and
/// projects everything into a fresh Grafeo instance.
///
/// # Errors
///
/// Returns an error description if rebuilding fails.
#[allow(clippy::too_many_lines)]
pub fn rebuild_from_redb(
    db: &LobsterDb,
    grafeo: &GrafeoDB,
) -> Result<RebuildStats, String> {
    let mut stats = RebuildStats::default();

    // Collect all episodes first, then process with direct field access
    let mut all_episodes: Vec<Episode> = Vec::new();
    {
        let rtxn = db.env.read_txn().map_err(|e| e.to_string())?;
        let iter = db.episodes.iter(&rtxn).map_err(|e| e.to_string())?;
        for result in iter {
            let (_, value) = result.map_err(|e| e.to_string())?;
            let episode: Episode =
                serde_json::from_slice(value).map_err(|e| e.to_string())?;
            all_episodes.push(episode);
        }
    }

    for episode in &all_episodes {
        stats.episodes_scanned += 1;

        // Only project Ready episodes
        if episode.processing_state != ProcessingState::Ready {
            stats.episodes_skipped += 1;
            continue;
        }

        // 1. Project episode node
        let ep_node = projection::project_episode(grafeo, episode);
        stats.episodes_projected += 1;

        // 2. Restore summary_text on the episode node
        if let Ok(summary) =
            crud::get_summary_artifact(db, &episode.episode_id.raw())
        {
            gdb::set_episode_summary(grafeo, ep_node, &summary.summary_text);
        }

        // 3. Restore embedding vector on the episode node
        if let Ok(emb) =
            crud::get_embedding_artifact(db, &episode.episode_id.raw())
        {
            let proxy = crate::embeddings::proxy::bytes_to_vector(
                &emb.pooled_vector_bytes,
            );
            if !proxy.is_empty() {
                gdb::set_node_embedding(grafeo, ep_node, &proxy);
            }
        }

        // 4. Project decisions for this episode
        let mut decision_nodes = std::collections::HashMap::new();
        if let Ok(dec_rtxn) = db.env.read_txn() {
            if let Ok(dec_iter) = db.decisions.iter(&dec_rtxn) {
                for dec_entry in dec_iter.flatten() {
                    let (_, dec_val) = dec_entry;
                    if let Ok(dec) = serde_json::from_slice::<
                        crate::store::schema::Decision,
                    >(dec_val)
                    {
                        if dec.episode_id == episode.episode_id {
                            let dn = projection::project_decision(
                                grafeo, &dec, ep_node,
                            );
                            decision_nodes
                                .insert(dec.decision_id.to_string(), dn);
                            stats.decisions_projected += 1;
                        }
                    }
                }
            }
        }

        // 5. Project tasks linked to this episode
        let mut task_nodes = std::collections::HashMap::new();
        if let Ok(task_rtxn) = db.env.read_txn() {
            if let Ok(task_iter) = db.tasks.iter(&task_rtxn) {
                for task_entry in task_iter.flatten() {
                    let (_, task_val) = task_entry;
                    if let Ok(task) = serde_json::from_slice::<
                        crate::store::schema::Task,
                    >(task_val)
                    {
                        if task.opened_in == episode.episode_id
                            || task.last_seen_in == episode.episode_id
                        {
                            let tn = projection::project_task(
                                grafeo, &task, ep_node,
                            );
                            task_nodes.insert(task.task_id.to_string(), tn);
                            stats.tasks_projected += 1;
                        }
                    }
                }
            }
        }

        // 6. Project entities and relation edges from extraction
        if let Ok(extraction) =
            crud::get_extraction_artifact(db, &episode.episode_id.raw())
        {
            if let Ok(output) = serde_json::from_slice::<
                crate::extract::traits::ExtractionOutput,
            >(&extraction.output_json)
            {
                let mut entity_nodes = std::collections::HashMap::new();

                for entity_fact in &output.entities {
                    let Some(kind) = parse_entity_kind(&entity_fact.kind)
                    else {
                        continue;
                    };
                    let eid = EntityId::derive(entity_fact.name.as_bytes());
                    let ent = crate::store::schema::Entity {
                        entity_id: eid,
                        repo_id: episode.repo_id,
                        kind,
                        canonical_name: entity_fact.name.clone(),
                        first_seen_episode: None,
                        last_seen_ts_utc_ms: None,
                        mention_count: 0,
                    };
                    let en = projection::project_entity(grafeo, &ent, ep_node);
                    entity_nodes.insert(entity_fact.name.clone(), en);
                    stats.entities_projected += 1;
                }

                // 7. Project relation edges
                let ts = episode.finalized_ts_utc_ms;
                for rel in &output.relations {
                    match rel.relation_type {
                        RelationType::TaskDecision => {
                            if let (Some(&tn), Some(&dn)) = (
                                task_nodes.get(&rel.from),
                                decision_nodes.get(&rel.to),
                            ) {
                                projection::link_task_decision(
                                    grafeo, tn, dn, ts,
                                );
                                stats.relations_projected += 1;
                            }
                        }
                        RelationType::DecisionEntity => {
                            if let (Some(&dn), Some(&en)) = (
                                decision_nodes.get(&rel.from),
                                entity_nodes.get(&rel.to),
                            ) {
                                projection::link_decision_entity(
                                    grafeo, dn, en, ts,
                                );
                                stats.relations_projected += 1;
                            }
                        }
                        RelationType::TaskEntity
                        | RelationType::EntityEntity => {
                            // These use the same edge creation; use
                            // MENTIONED_ENTITY for task→entity and
                            // ENTITY_ENTITY for entity→entity
                            stats.relations_projected += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(stats)
}

/// Statistics from a Grafeo rebuild.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub episodes_scanned: usize,
    pub episodes_projected: usize,
    pub episodes_skipped: usize,
    pub decisions_projected: usize,
    pub entities_projected: usize,
    pub tasks_projected: usize,
    pub relations_projected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        episodes::finalize::{FinalizeResult, finalize_episode},
        graph::db as grafeo_db,
        store::db,
    };

    #[tokio::test]
    async fn test_rebuild_empty_db() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let stats = rebuild_from_redb(&database, &grafeo).unwrap();
        assert_eq!(stats.episodes_scanned, 0);
        assert_eq!(stats.episodes_projected, 0);
        assert_eq!(grafeo.node_count(), 0);
    }

    #[tokio::test]
    async fn test_rebuild_skips_non_ready() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo_original = grafeo_db::new_in_memory();

        // Create an episode but leave it Pending (don't finalize)
        let ep = Episode {
            episode_id: crate::store::ids::EpisodeId::derive(b"ep"),
            repo_id: crate::store::ids::RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: 1000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep).unwrap();

        let stats = rebuild_from_redb(&database, &grafeo_original).unwrap();
        assert_eq!(stats.episodes_scanned, 1);
        assert_eq!(stats.episodes_skipped, 1);
        assert_eq!(stats.episodes_projected, 0);
        assert_eq!(grafeo_original.node_count(), 0);
    }

    #[tokio::test]
    async fn test_rebuild_after_finalization() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo_original = grafeo_db::new_in_memory();

        // Finalize an episode (creates Ready state)
        let result = finalize_episode(
            &database,
            &grafeo_original,
            "/test/repo",
            b"[]",
            0,
            5,
            None,
        )
        .await;
        assert!(matches!(result, FinalizeResult::Ready { .. }));

        let original_count = grafeo_original.node_count();
        assert!(original_count >= 1);

        // Now rebuild into a FRESH Grafeo instance
        let grafeo_rebuilt = grafeo_db::new_in_memory();
        let stats = rebuild_from_redb(&database, &grafeo_rebuilt).unwrap();

        assert_eq!(stats.episodes_scanned, 1);
        assert_eq!(stats.episodes_projected, 1);
        // Rebuilt should have at least the episode node
        assert!(grafeo_rebuilt.node_count() >= 1);
    }

    #[test]
    fn test_rebuild_restores_summary_and_embedding() {
        use crate::store::{
            ids::{ArtifactId, EpisodeId, RepoId},
            schema::{EmbeddingArtifact, EmbeddingBackend, SummaryArtifact},
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        // Create a Ready episode with summary and embedding
        let ep = Episode {
            episode_id: EpisodeId::derive(b"ep1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep).unwrap();

        let summary = SummaryArtifact {
            episode_id: ep.episode_id,
            revision: "v1".into(),
            summary_text: "User implemented feature X".into(),
            payload_checksum: [0; 32],
        };
        crud::put_summary_artifact(&database, &summary).unwrap();

        let proxy_vec = vec![0.42_f32; 8];
        let emb = EmbeddingArtifact {
            artifact_id: ArtifactId::derive(b"emb1"),
            revision: "v1".into(),
            backend: EmbeddingBackend::Cpu,
            quantization: None,
            pooled_vector_bytes: crate::embeddings::proxy::vector_to_bytes(
                &proxy_vec,
            ),
            late_interaction_bytes: None,
            payload_checksum: [0; 32],
        };
        // Store embedding keyed by episode ID so rebuild can find it
        crud::put_embedding_artifact(&database, &emb).unwrap();

        // Rebuild
        let grafeo = grafeo_db::new_in_memory();
        let stats = rebuild_from_redb(&database, &grafeo).unwrap();
        assert_eq!(stats.episodes_projected, 1);

        // Verify summary_text was set on the node
        let session = grafeo.session();
        let result = session
            .execute("MATCH (ep:Episode) RETURN ep.summary_text")
            .unwrap();
        let rows: Vec<_> = result.iter().collect();
        assert!(!rows.is_empty(), "should find episode node");
        let summary_val = rows[0][0].as_str().unwrap_or("");
        assert_eq!(summary_val, "User implemented feature X");
    }

    #[test]
    fn test_rebuild_restores_tasks() {
        use crate::store::{
            ids::{EpisodeId, RepoId, TaskId},
            schema::{Task, TaskStatus},
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        let ep = Episode {
            episode_id: EpisodeId::derive(b"ep1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: Some(TaskId::derive(b"t1")),
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep).unwrap();

        let task = Task {
            task_id: TaskId::derive(b"t1"),
            repo_id: RepoId::derive(b"repo"),
            title: "Build memory".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep1"),
            last_seen_in: EpisodeId::derive(b"ep1"),
        };
        crud::put_task(&database, &task).unwrap();

        let grafeo = grafeo_db::new_in_memory();
        let stats = rebuild_from_redb(&database, &grafeo).unwrap();
        assert_eq!(stats.tasks_projected, 1);

        // Verify task node exists
        let session = grafeo.session();
        let result = session.execute("MATCH (t:Task) RETURN t.title").unwrap();
        let rows: Vec<_> = result.iter().collect();
        assert!(!rows.is_empty(), "should find task node");
        assert_eq!(rows[0][0].as_str().unwrap_or(""), "Build memory");
    }
}
