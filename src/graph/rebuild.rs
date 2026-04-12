//! Rebuild Grafeo from redb durable artifacts.
//!
//! If Grafeo is lost, this module can reconstruct the complete
//! semantic graph from the canonical artifacts stored in redb.
//! It should not require re-running summarization or extraction.

use grafeo::GrafeoDB;
use redb::{Database, ReadableDatabase, ReadableTable};

use crate::{
    episodes::finalize::parse_entity_kind,
    graph::projection,
    store::{
        crud,
        schema::{Episode, ProcessingState},
        tables,
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
pub fn rebuild_from_redb(
    db: &Database,
    grafeo: &GrafeoDB,
) -> Result<RebuildStats, String> {
    let mut stats = RebuildStats::default();

    // Read all episodes from redb
    let read_txn = db.begin_read().map_err(|e| e.to_string())?;
    let table = read_txn
        .open_table(tables::EPISODES)
        .map_err(|e| e.to_string())?;

    // Iterate all entries
    let iter = table.iter().map_err(|e| e.to_string())?;

    for result in iter {
        let (_, value) = result.map_err(|e| e.to_string())?;
        let episode: Episode =
            serde_json::from_slice(value.value()).map_err(|e| e.to_string())?;

        stats.episodes_scanned += 1;

        // Only project Ready episodes
        if episode.processing_state != ProcessingState::Ready {
            stats.episodes_skipped += 1;
            continue;
        }

        // Project episode node
        let ep_node = projection::project_episode(grafeo, &episode);
        stats.episodes_projected += 1;

        // Project decisions for this episode by scanning the
        // decisions table for matching episode_id
        if let Ok(dec_txn) = db.begin_read() {
            if let Ok(dec_table) = dec_txn.open_table(tables::DECISIONS) {
                if let Ok(dec_iter) = dec_table.iter() {
                    for dec_entry in dec_iter.flatten() {
                        let (_, dec_val) = dec_entry;
                        if let Ok(dec) =
                            serde_json::from_slice::<
                                crate::store::schema::Decision,
                            >(dec_val.value())
                        {
                            if dec.episode_id == episode.episode_id {
                                projection::project_decision(
                                    grafeo, &dec, ep_node,
                                );
                                stats.decisions_projected += 1;
                            }
                        }
                    }
                }
            }
        }

        // Try to load extraction artifact for this episode
        if let Ok(extraction) =
            crud::get_extraction_artifact(db, &episode.episode_id.raw())
        {
            if let Ok(output) = serde_json::from_slice::<
                crate::extract::traits::ExtractionOutput,
            >(&extraction.output_json)
            {
                for entity_fact in &output.entities {
                    let ent = crate::store::schema::Entity {
                        entity_id: crate::store::ids::EntityId::derive(
                            entity_fact.name.as_bytes(),
                        ),
                        repo_id: episode.repo_id,
                        kind: parse_entity_kind(&entity_fact.kind),
                        canonical_name: entity_fact.name.clone(),
                    };
                    projection::project_entity(grafeo, &ent, ep_node);
                    stats.entities_projected += 1;
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
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let stats = rebuild_from_redb(&database, &grafeo).unwrap();
        assert_eq!(stats.episodes_scanned, 0);
        assert_eq!(stats.episodes_projected, 0);
        assert_eq!(grafeo.node_count(), 0);
    }

    #[tokio::test]
    async fn test_rebuild_skips_non_ready() {
        let database = db::open_in_memory().unwrap();
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
        let database = db::open_in_memory().unwrap();
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
}
