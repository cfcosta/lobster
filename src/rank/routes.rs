//! Route execution: MaxSim-first retrieval with graph expansion.
//!
//! The pipeline:
//! 1. Score ALL episode embeddings via ColBERT MaxSim
//! 2. BM25 text search on decisions, entities, tasks
//! 3. Graph expansion on top hits (1-hop neighbors)
//! 4. Composite scoring, MMR diversity, threshold filtering

use grafeo::GrafeoDB;

use crate::{
    rank::{
        classifier::classify_query,
        retrieval::{ScoredCandidate, apply_mmr},
        route::RetrievalRoute,
        scoring::{
            self,
            DEFAULT_WEIGHTS,
            ScoringInput,
            auto_threshold,
            composite_score,
            mmr_lambda,
            normalize_score,
        },
    },
    store::{db::LobsterDb, ids::RawId, visibility},
};

/// A retrieval result with score and metadata.
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub episode_id: RawId,
    pub score: f64,
    pub artifact_type: String,
    pub route: RetrievalRoute,
}

/// Execute a retrieval query end-to-end.
pub fn execute_query(
    query: &str,
    db: &LobsterDb,
    grafeo: &GrafeoDB,
    is_mcp: bool,
) -> Vec<RetrievalResult> {
    execute_query_with_context(query, db, grafeo, is_mcp, None)
}

/// Execute a query with optional task context for scoring.
pub fn execute_query_with_context(
    query: &str,
    db: &LobsterDb,
    grafeo: &GrafeoDB,
    is_mcp: bool,
    current_task_id: Option<&crate::store::ids::TaskId>,
) -> Vec<RetrievalResult> {
    let classified_route = classify_query(query);

    if classified_route == RetrievalRoute::Abstain {
        return vec![];
    }

    let route = classified_route;

    let threshold = if is_mcp {
        scoring::mcp_threshold(route)
    } else {
        auto_threshold(route)
    };

    let lambda = mmr_lambda(route);
    let budget = if is_mcp {
        scoring::mcp_surface_budget(route)
    } else {
        scoring::auto_surface_budget(route)
    };

    // ── Step 1: MaxSim on all episode embeddings ────────────
    let mut candidates = maxsim_rank_episodes(db, query);

    // ── Step 2: BM25 on decisions, entities, tasks ──────────
    bm25_search_structured(grafeo, query, &mut candidates);

    // ── Step 3: Graph expansion on initial hits ─────────────
    if route == RetrievalRoute::HybridGraph {
        expand_graph_neighbors(grafeo, &mut candidates);
    }

    // ── Step 4: Stable sort + MMR diversity ─────────────────
    crate::rank::retrieval::stable_sort(&mut candidates);

    let proxy_vectors = load_proxy_vectors(db, &candidates);
    let diverse = apply_mmr(&candidates, budget, lambda, |a, b| {
        if a == b {
            return 1.0;
        }
        match (proxy_vectors.get(a), proxy_vectors.get(b)) {
            (Some(va), Some(vb)) => {
                crate::rank::retrieval::cosine_similarity(va, vb)
            }
            _ => 0.0,
        }
    });

    // ── Step 5: Visibility + composite scoring + threshold ──
    let candidate_ids: Vec<_> = diverse.iter().map(|c| c.id).collect();
    diverse
        .into_iter()
        .filter(|c| {
            if c.artifact_type == "summary" {
                visibility::is_episode_visible(db, &c.id)
            } else {
                true
            }
        })
        .filter(|c| {
            let now_ms = chrono::Utc::now().timestamp_millis();

            let parent_episode = match c.artifact_type.as_str() {
                "summary" => crate::store::crud::get_episode(db, &c.id).ok(),
                "decision" => crate::store::crud::get_decision(db, &c.id)
                    .ok()
                    .and_then(|d| {
                        crate::store::crud::get_episode(db, &d.episode_id.raw())
                            .ok()
                    }),
                _ => None,
            };

            let artifact_ts = parent_episode
                .as_ref()
                .map_or(0, |ep| ep.finalized_ts_utc_ms);
            let recency = crate::rank::recency::recency_score_default(
                artifact_ts,
                now_ms,
            );

            let task_ol =
                crate::rank::context::task_overlap(db, &c.id, current_task_id);

            let graph_sup = if route == RetrievalRoute::HybridGraph {
                crate::rank::context::graph_support(
                    grafeo,
                    &c.id.to_string(),
                    &candidate_ids,
                )
            } else {
                0.0
            };

            let input = ScoringInput {
                semantic: c.score,
                recency,
                task_overlap: task_ol,
                graph_support: graph_sup,
                is_decision: c.artifact_type == "decision",
                is_noisy: parent_episode.is_some_and(|ep| ep.is_noisy),
            };
            let raw = composite_score(&input, &DEFAULT_WEIGHTS, route);
            let normalized = normalize_score(raw, &DEFAULT_WEIGHTS, route);
            normalized >= threshold
        })
        .map(|c| RetrievalResult {
            episode_id: c.id,
            score: c.score,
            artifact_type: c.artifact_type,
            route,
        })
        .collect()
}

/// Score ALL episode embedding artifacts against the query via MaxSim.
///
/// This is the primary retrieval signal: ColBERT late-interaction
/// scoring on stored hierarchically-pooled embeddings. Falls back
/// to proxy-vector cosine when late-interaction bytes are absent.
fn maxsim_rank_episodes(db: &LobsterDb, query: &str) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    let Ok(rtxn) = db.env.read_txn() else {
        return candidates;
    };
    let Ok(iter) = db.embedding_artifacts.iter(&rtxn) else {
        return candidates;
    };

    // Collect all embedding artifacts keyed by their raw ID
    let mut artifacts: Vec<(RawId, crate::store::schema::EmbeddingArtifact)> =
        Vec::new();
    for entry in iter.flatten() {
        let (key, value) = entry;
        let key_bytes: [u8; 16] = match key.try_into() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(emb) = serde_json::from_slice::<
            crate::store::schema::EmbeddingArtifact,
        >(value) else {
            continue;
        };
        artifacts.push((RawId::from_bytes(key_bytes), emb));
    }
    drop(rtxn);

    if artifacts.is_empty() {
        return candidates;
    }

    // Score each artifact via rerank_score (MaxSim with fallback)
    for (raw_id, _) in &artifacts {
        let score = crate::rank::rerank::rerank_score(db, query, raw_id);
        if score > 0.0 {
            candidates.push(ScoredCandidate {
                id: *raw_id,
                score,
                artifact_type: "summary".into(),
            });
        }
    }

    // Sort by score descending and keep top-k for downstream
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(30);

    candidates
}

/// BM25 text search on decisions, entities, and tasks.
fn bm25_search_structured(
    grafeo: &GrafeoDB,
    query: &str,
    candidates: &mut Vec<ScoredCandidate>,
) {
    if grafeo.node_count() == 0 {
        return;
    }

    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::DECISION,
        "statement",
        query,
        20,
    ) {
        collect_hits(grafeo, &hits, "decision_id", "decision", candidates);
    }

    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::ENTITY,
        "canonical_name",
        query,
        20,
    ) {
        collect_hits(grafeo, &hits, "entity_id", "entity", candidates);
    }

    if let Ok(hits) =
        grafeo.text_search(crate::graph::db::labels::TASK, "title", query, 20)
    {
        collect_hits(grafeo, &hits, "task_id", "task", candidates);
    }
}

/// 1-hop graph expansion: for each initial candidate, add its
/// Grafeo neighbors as additional candidates.
fn expand_graph_neighbors(
    grafeo: &GrafeoDB,
    candidates: &mut Vec<ScoredCandidate>,
) {
    let session = grafeo.session();
    let initial_ids: Vec<String> =
        candidates.iter().map(|c| c.id.to_string()).collect();

    for id_str in &initial_ids {
        let query = format!(
            "MATCH (n)-[]->(m) WHERE \
             (n.episode_id = '{id_str}' \
              OR n.decision_id = '{id_str}' \
              OR n.entity_id = '{id_str}') \
             RETURN m.episode_id, m.decision_id, m.entity_id"
        );

        let Ok(result) = session.execute(&query) else {
            continue;
        };

        for row in result.iter() {
            let neighbor = [
                (row[0].as_str(), "summary"),
                (row[1].as_str(), "decision"),
                (row[2].as_str(), "entity"),
            ];
            for (id_val, artifact_type) in &neighbor {
                if let Some(id) = id_val {
                    if let Ok(raw_id) = id.parse() {
                        if !candidates.iter().any(|c| c.id == raw_id) {
                            candidates.push(ScoredCandidate {
                                id: raw_id,
                                score: 0.5,
                                artifact_type: (*artifact_type).into(),
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Load proxy vectors for MMR pairwise comparison.
fn load_proxy_vectors(
    db: &LobsterDb,
    candidates: &[ScoredCandidate],
) -> std::collections::HashMap<crate::store::ids::RawId, Vec<f32>> {
    use crate::store::crud;

    let mut vectors = std::collections::HashMap::new();
    for c in candidates {
        if let Ok(emb) = crud::get_embedding_artifact(db, &c.id) {
            let proxy = crate::embeddings::proxy::bytes_to_vector(
                &emb.pooled_vector_bytes,
            );
            if !proxy.is_empty() {
                vectors.insert(c.id, proxy);
            }
        }
    }
    vectors
}

/// Collect BM25 search hits from Grafeo into scored candidates.
fn collect_hits(
    grafeo: &GrafeoDB,
    hits: &[(grafeo::NodeId, f64)],
    id_property: &str,
    artifact_type: &str,
    candidates: &mut Vec<ScoredCandidate>,
) {
    for (node_id, score) in hits {
        if let Some(node) = grafeo.get_node(*node_id) {
            if let Some(id_val) = node.get_property(id_property) {
                if let Some(id_str) = id_val.as_str() {
                    if let Ok(raw_id) = id_str.parse() {
                        if let Some(existing) =
                            candidates.iter_mut().find(|c| c.id == raw_id)
                        {
                            if *score > existing.score {
                                existing.score = *score;
                            }
                        } else {
                            candidates.push(ScoredCandidate {
                                id: raw_id,
                                score: *score,
                                artifact_type: artifact_type.into(),
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graph::db as grafeo_db,
        store::{
            crud,
            db,
            ids::{EpisodeId, RepoId},
            schema::{Episode, ProcessingState},
        },
    };

    #[test]
    fn test_execute_query_returns_empty_for_no_data() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let results =
            execute_query("how does memory work", &database, &grafeo, false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_execute_query_filters_non_ready() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let ep = Episode {
            episode_id: EpisodeId::derive(b"pending"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: 1000,
            retry_count: 0,
            is_noisy: false,
        };
        crud::put_episode(&database, &ep).unwrap();

        let results = execute_query("test query", &database, &grafeo, false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_mcp_uses_lower_threshold() {
        assert!(
            scoring::mcp_threshold(RetrievalRoute::Hybrid)
                < scoring::auto_threshold(RetrievalRoute::Hybrid)
        );
    }

    #[test]
    fn test_collect_hits_dedup() {
        let grafeo = grafeo_db::new_in_memory();
        let session = grafeo.session();

        let _ = session.execute(
            "CREATE (e:Episode {episode_id: 'aaa', summary_text: 'test'})",
        );
        let _ = session.execute(
            "CREATE (e:Episode {episode_id: 'aaa', summary_text: 'test again'})",
        );
        crate::graph::indexes::ensure_indexes(&grafeo);

        let mut candidates = Vec::new();
        if let Ok(hits) = grafeo.text_search(
            crate::graph::db::labels::EPISODE,
            "summary_text",
            "test",
            20,
        ) {
            collect_hits(
                &grafeo,
                &hits,
                "episode_id",
                "summary",
                &mut candidates,
            );
        }

        let unique_ids: std::collections::HashSet<_> =
            candidates.iter().map(|c| c.id).collect();
        assert_eq!(
            candidates.len(),
            unique_ids.len(),
            "candidates should have no duplicate IDs"
        );
    }
}
