//! Query-routed retrieval: route classifier and route types.
//!
//! The router runs before any search, selecting the cheapest
//! retrieval path that satisfies each query class.

use serde::{Deserialize, Serialize};

/// The retrieval path selected by the deterministic route
/// classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RetrievalRoute {
    /// Query contains file paths, symbol names, error strings,
    /// ISO dates, or entity IDs. Property-index or BM25 exact
    /// match.
    Exact,

    /// Ordinary natural-language recall query. BM25 + HNSW
    /// hybrid search with pylate-rs reranking.
    Hybrid,

    /// Query contains relational/causal language: "why", "depends
    /// on", "history of", etc. Hybrid search plus 1-hop graph
    /// expansion.
    HybridGraph,

    /// No route produced results above its confidence threshold.
    /// Return nothing — silence is preferable to weak evidence.
    Abstain,
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: RetrievalRoute serde round-trip --
    #[hegel::test(test_cases = 200)]
    fn prop_route_serde_roundtrip(tc: TestCase) {
        let route: RetrievalRoute = tc.draw(gs::sampled_from(vec![
            RetrievalRoute::Exact,
            RetrievalRoute::Hybrid,
            RetrievalRoute::HybridGraph,
            RetrievalRoute::Abstain,
        ]));
        let json = serde_json::to_string(&route).unwrap();
        let parsed: RetrievalRoute = serde_json::from_str(&json).unwrap();
        assert_eq!(route, parsed);
    }

    #[test]
    fn test_all_variants_distinct() {
        let variants = [
            RetrievalRoute::Exact,
            RetrievalRoute::Hybrid,
            RetrievalRoute::HybridGraph,
            RetrievalRoute::Abstain,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
