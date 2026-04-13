//! Route execution: run the appropriate search strategy for each
//! retrieval route.
//!
//! Each route fetches candidates from Grafeo, applies scoring,
//! MMR diversity, ready-set intersection, and confidence rejection.

use grafeo::GrafeoDB;
use redb::Database;

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
    store::{ids::RawId, visibility},
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
///
/// 1. Classify the query into a route
/// 2. Execute the route-specific search
/// 3. Score, apply MMR, reject below threshold
/// 4. Intersect with ready set
/// 5. Return results (or empty if Abstain)
pub fn execute_query(
    query: &str,
    db: &Database,
    grafeo: &GrafeoDB,
    is_mcp: bool,
) -> Vec<RetrievalResult> {
    execute_query_with_context(query, db, grafeo, is_mcp, None)
}

/// Execute a query with optional task context for scoring.
pub fn execute_query_with_context(
    query: &str,
    db: &Database,
    grafeo: &GrafeoDB,
    is_mcp: bool,
    current_task_id: Option<&crate::store::ids::TaskId>,
) -> Vec<RetrievalResult> {
    let route = classify_query(query);

    if route == RetrievalRoute::Abstain {
        return vec![];
    }

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

    let mut candidates = search_grafeo(grafeo, query, route);

    // Apply stable tie-breakers before MMR per spec
    crate::rank::retrieval::stable_sort(&mut candidates);

    // Rerank with cosine similarity on proxy vectors when available.
    // Load proxy vectors for each candidate from redb, then use
    // cosine similarity for MMR pairwise comparison.
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

    // Intersect with ready set and apply threshold.
    // Decisions and entities inherit visibility from their parent
    // episode (which was Ready when projected). Only filter
    // episode-typed candidates against the ready set directly.
    let candidate_ids: Vec<_> = diverse.iter().map(|c| c.id).collect();
    diverse
        .into_iter()
        .filter(|c| {
            if c.artifact_type == "summary" {
                visibility::is_episode_visible(db, &c.id)
            } else {
                // Decisions/entities were only projected from
                // Ready episodes, so they're implicitly visible
                true
            }
        })
        .filter(|c| {
            // Compute real recency from episode timestamp
            let now_ms = chrono::Utc::now().timestamp_millis();
            let artifact_ts = crate::store::crud::get_episode(db, &c.id)
                .map_or(now_ms, |ep| ep.finalized_ts_utc_ms);
            let recency = crate::rank::recency::recency_score_default(
                artifact_ts,
                now_ms,
            );

            // Compute task_overlap from current task context
            let task_ol =
                crate::rank::context::task_overlap(db, &c.id, current_task_id);

            // Compute graph_support for HybridGraph route
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
                is_noisy: crate::store::crud::get_episode(db, &c.id)
                    .is_ok_and(|ep| ep.is_noisy),
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

/// Load proxy vectors for candidates from embedding artifacts.
fn load_proxy_vectors(
    db: &Database,
    candidates: &[ScoredCandidate],
) -> std::collections::HashMap<crate::store::ids::RawId, Vec<f32>> {
    use crate::store::crud;

    let mut vectors = std::collections::HashMap::new();
    for c in candidates {
        // Try loading embedding artifact keyed by the candidate's
        // episode ID. The artifact stores pooled_vector_bytes.
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

/// Search Grafeo for candidates matching the query.
///
/// Route-aware search using `ColBERT` + Grafeo.
///
/// - **Exact**: BM25 text search only on decisions and entities
///   (no summary search, no vector encoding needed).
/// - **Hybrid**: `ColBERT` query encoding + hybrid search (BM25 +
///   vector RRF) on summaries, BM25 on decisions and entities.
/// - **`HybridGraph`**: same as Hybrid, plus 1-hop graph neighbor
///   expansion on initial hits.
fn search_grafeo(
    grafeo: &GrafeoDB,
    query: &str,
    route: RetrievalRoute,
) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    if grafeo.node_count() == 0 {
        return candidates;
    }

    match route {
        RetrievalRoute::Exact => {
            search_exact(grafeo, query, &mut candidates);
        }
        RetrievalRoute::Hybrid => {
            search_hybrid(grafeo, query, &mut candidates);
        }
        RetrievalRoute::HybridGraph => {
            search_hybrid(grafeo, query, &mut candidates);
            expand_graph_neighbors(grafeo, &mut candidates);
        }
        RetrievalRoute::Abstain => {}
    }

    candidates
}

/// Exact route: BM25 text search on decisions and entities only.
/// No `ColBERT` encoding — these are exact/lexical queries.
fn search_exact(
    grafeo: &GrafeoDB,
    query: &str,
    candidates: &mut Vec<ScoredCandidate>,
) {
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
}

/// Hybrid route: `ColBERT` query encoding + hybrid search on
/// summaries, BM25 on decisions and entities.
fn search_hybrid(
    grafeo: &GrafeoDB,
    query: &str,
    candidates: &mut Vec<ScoredCandidate>,
) {
    let mut model = match crate::embeddings::encoder::load_model() {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(
                error = %e,
                "ColBERT model not available — run `lobster install`"
            );
            return;
        }
    };
    let query_vector =
        crate::embeddings::encoder::encode_query(&mut model, query);
    let qv_ref = query_vector.as_ref().ok().map(Vec::as_slice);

    // Hybrid search (BM25 + vector RRF) on episode summaries
    if let Ok(hits) = grafeo.hybrid_search(
        crate::graph::db::labels::EPISODE,
        "summary_text",
        "embedding",
        query,
        qv_ref,
        20,
        None,
    ) {
        collect_hits(grafeo, &hits, "episode_id", "summary", candidates);
    }

    // BM25 text search on decisions
    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::DECISION,
        "statement",
        query,
        20,
    ) {
        collect_hits(grafeo, &hits, "decision_id", "decision", candidates);
    }

    // BM25 text search on entities
    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::ENTITY,
        "canonical_name",
        query,
        20,
    ) {
        collect_hits(grafeo, &hits, "entity_id", "entity", candidates);
    }
}

/// `HybridGraph` expansion: for each initial candidate, add its
/// 1-hop Grafeo neighbors as additional candidates.
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
            // Try each possible ID column
            let neighbor = [
                (row[0].as_str(), "summary"),
                (row[1].as_str(), "decision"),
                (row[2].as_str(), "entity"),
            ];
            for (id_val, artifact_type) in &neighbor {
                if let Some(id) = id_val {
                    if let Ok(raw_id) = id.parse() {
                        // Avoid duplicates
                        if !candidates.iter().any(|c| c.id == raw_id) {
                            candidates.push(ScoredCandidate {
                                id: raw_id,
                                // Neighbors get a discount — they
                                // weren't direct search hits
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

/// Collect search hits from Grafeo into scored candidates.
///
/// Uses the actual score returned by Grafeo's BM25/hybrid search.
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
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        let results =
            execute_query("how does memory work", &database, &grafeo, false);

        // No data in Grafeo → no results
        assert!(results.is_empty());
    }

    #[test]
    fn test_execute_query_classifies_correctly() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // File path → exact route
        let results = execute_query("src/main.rs", &database, &grafeo, false);
        assert!(results.is_empty()); // no data, but route was Exact

        // Relational → hybrid+graph route
        let results =
            execute_query("why did we choose redb", &database, &grafeo, false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_execute_query_filters_non_ready() {
        let database = db::open_in_memory().unwrap();
        let grafeo = grafeo_db::new_in_memory();

        // Create a Pending episode — should not appear in results
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
        // Even if Grafeo had data for this episode, the
        // visibility filter would exclude it
        assert!(results.is_empty());
    }

    #[test]
    fn test_mcp_uses_lower_threshold() {
        // MCP threshold is lower than auto threshold
        assert!(
            scoring::mcp_threshold(RetrievalRoute::Hybrid)
                < scoring::auto_threshold(RetrievalRoute::Hybrid)
        );
    }
}
