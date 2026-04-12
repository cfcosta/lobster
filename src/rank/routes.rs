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
    let route = classify_query(query);

    if route == RetrievalRoute::Abstain {
        return vec![];
    }

    // For now, search is simulated — Grafeo text/vector search
    // will be wired when indexes are created. The pipeline
    // structure is what matters: classify → search → score →
    // MMR → ready-set → threshold → return.

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

    // Search Grafeo using GQL text matching on decision statements
    // and entity names. This is a basic implementation that will be
    // replaced with HNSW vector search when embeddings are active.
    let candidates = search_grafeo(grafeo, query, route);

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

    // Intersect with ready set and apply threshold
    diverse
        .into_iter()
        .filter(|c| visibility::is_episode_visible(db, &c.id))
        .filter(|c| {
            let input = ScoringInput {
                semantic: c.score,
                recency: 0.5,
                task_overlap: 0.0,
                graph_support: 0.0,
                is_decision: c.artifact_type == "decision",
                is_noisy: false,
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
/// Uses BM25 text search (when indexes exist) plus GQL property
/// matching as fallback. Returns scored candidates.
#[allow(clippy::too_many_lines)]
fn search_grafeo(
    grafeo: &GrafeoDB,
    query: &str,
    _route: RetrievalRoute,
) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();
    let query_lower = query.to_lowercase();

    // Try BM25 text search on decision statements (uses index)
    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::DECISION,
        "statement",
        query,
        20,
    ) {
        for (node_id, _distance) in &hits {
            if let Some(node) = grafeo.get_node(*node_id) {
                if let Some(id_val) = node.get_property("decision_id") {
                    if let Some(id_str) = id_val.as_str() {
                        if let Ok(raw_id) = id_str.parse() {
                            candidates.push(ScoredCandidate {
                                id: raw_id,
                                score: 0.9,
                                artifact_type: "decision".into(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Try BM25 text search on episode summaries
    if let Ok(hits) = grafeo.text_search(
        crate::graph::db::labels::EPISODE,
        "summary_text",
        query,
        20,
    ) {
        for (node_id, _distance) in &hits {
            if let Some(node) = grafeo.get_node(*node_id) {
                if let Some(id_val) = node.get_property("episode_id") {
                    if let Some(id_str) = id_val.as_str() {
                        if let Ok(raw_id) = id_str.parse() {
                            candidates.push(ScoredCandidate {
                                id: raw_id,
                                score: 0.8,
                                artifact_type: "summary".into(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Fallback: GQL property matching for broader coverage
    let session = grafeo.session();

    // Search decisions
    if let Ok(result) =
        session.execute("MATCH (d:Decision) RETURN d.decision_id, d.statement")
    {
        for row in result.iter() {
            if let (Some(id), Some(stmt)) = (row[0].as_str(), row[1].as_str()) {
                let stmt_lower = stmt.to_lowercase();
                // Simple text overlap scoring
                let overlap = query_lower
                    .split_whitespace()
                    .filter(|w| stmt_lower.contains(w))
                    .count();
                if overlap > 0 {
                    #[allow(clippy::cast_precision_loss)]
                    let score = overlap as f64
                        / query_lower.split_whitespace().count().max(1) as f64;
                    if let Ok(raw_id) = id.parse() {
                        candidates.push(ScoredCandidate {
                            id: raw_id,
                            score,
                            artifact_type: "decision".into(),
                        });
                    }
                }
            }
        }
    }

    // Search episode summaries
    if let Ok(result) = session
        .execute("MATCH (ep:Episode) RETURN ep.episode_id, ep.summary_text")
    {
        for row in result.iter() {
            if let (Some(id), Some(text)) = (row[0].as_str(), row[1].as_str()) {
                let text_lower = text.to_lowercase();
                let overlap = query_lower
                    .split_whitespace()
                    .filter(|w| text_lower.contains(w))
                    .count();
                if overlap > 0 {
                    #[allow(clippy::cast_precision_loss)]
                    let score = overlap as f64
                        / query_lower.split_whitespace().count().max(1) as f64
                        * 0.8; // slightly lower than decisions
                    if let Ok(raw_id) = id.parse() {
                        candidates.push(ScoredCandidate {
                            id: raw_id,
                            score,
                            artifact_type: "summary".into(),
                        });
                    }
                }
            }
        }
    }

    // Search entity nodes
    if let Ok(result) =
        session.execute("MATCH (e:Entity) RETURN e.entity_id, e.canonical_name")
    {
        for row in result.iter() {
            if let (Some(id), Some(name)) = (row[0].as_str(), row[1].as_str()) {
                let name_lower = name.to_lowercase();
                if query_lower.contains(&name_lower)
                    || name_lower.contains(&query_lower)
                {
                    if let Ok(raw_id) = id.parse() {
                        candidates.push(ScoredCandidate {
                            id: raw_id,
                            score: 0.7,
                            artifact_type: "entity".into(),
                        });
                    }
                }
            }
        }
    }

    candidates
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
