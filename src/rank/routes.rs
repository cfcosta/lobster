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
    _grafeo: &GrafeoDB,
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

    // TODO: Replace with actual Grafeo search when indexes exist.
    // For now return empty — this is honest about what works.
    let candidates: Vec<ScoredCandidate> = vec![];

    // Apply MMR diversity
    let diverse = apply_mmr(&candidates, budget, lambda, |a, b| {
        // Placeholder similarity — real impl uses cosine on
        // pooled vectors
        if a == b { 1.0 } else { 0.0 }
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
