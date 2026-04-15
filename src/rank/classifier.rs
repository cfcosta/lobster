//! LLM-based query route classifier.
//!
//! Classifies recall queries into retrieval routes using a structured
//! LLM call. Falls back to `Hybrid` if the LLM is unavailable.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rank::route::RetrievalRoute;

/// LLM-returned classification for a recall query.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct QueryClassification {
    /// The retrieval route to use. One of:
    /// - "Exact": query contains file paths, symbol names, error
    ///   strings, ISO dates, or entity IDs. Use property-index or
    ///   BM25 exact match.
    /// - "Hybrid": ordinary natural-language recall query. Use BM25
    ///   plus vector hybrid search.
    /// - "HybridGraph": query contains relational/causal language
    ///   ("why", "depends on", "history of"). Use hybrid search
    ///   plus 1-hop graph expansion.
    /// - "Abstain": the query is not a memory recall question (e.g.,
    ///   greetings, meta-questions about the tool itself). Return
    ///   nothing.
    pub route: String,
}

const CLASSIFIER_PREAMBLE: &str = "\
You classify developer recall queries into retrieval routes.\n\
\n\
Choose exactly one route:\n\
- \"Exact\": query references specific artifacts — file paths (src/main.rs),\n\
  qualified symbols (Store::get), error strings (error[E0277]), ISO dates\n\
  (2024-01-15), or ID prefixes (decision:, task:, entity:).\n\
- \"Hybrid\": ordinary natural-language question about past work, code, or\n\
  decisions. This is the default when nothing else fits.\n\
- \"HybridGraph\": query asks about causality, dependencies, or change\n\
  history — \"why did we\", \"what depends on\", \"what changed\", \"history of\",\n\
  \"related to\", \"caused by\".\n\
- \"Abstain\": query is not a memory recall question — greetings, commands,\n\
  meta-questions about the tool, or empty/nonsensical input.";

/// Classify a query into a retrieval route via LLM.
///
/// Falls back to `Hybrid` if the LLM call fails (no API key, timeout,
/// etc.) to ensure recall always attempts something.
#[must_use]
pub fn classify_query(query: &str) -> RetrievalRoute {
    if query.trim().is_empty() {
        return RetrievalRoute::Abstain;
    }

    let result = tokio::runtime::Handle::try_current().ok().and_then(|h| {
        // block_in_place requires a multi-threaded runtime.
        // Fall back to Hybrid on single-threaded runtimes.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tokio::task::block_in_place(|| {
                h.block_on(async {
                    crate::app::llm::extract::<QueryClassification>(
                        CLASSIFIER_PREAMBLE,
                        query,
                    )
                    .await
                    .ok()
                })
            })
        }))
        .ok()
        .flatten()
    });

    result.map_or(RetrievalRoute::Hybrid, |c| match c.route.as_str() {
        "Exact" => RetrievalRoute::Exact,
        "HybridGraph" => RetrievalRoute::HybridGraph,
        "Abstain" => RetrievalRoute::Abstain,
        _ => RetrievalRoute::Hybrid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_abstains() {
        assert_eq!(classify_query(""), RetrievalRoute::Abstain);
        assert_eq!(classify_query("   "), RetrievalRoute::Abstain);
    }

    #[test]
    fn test_classification_schema() {
        let schema = schemars::schema_for!(QueryClassification);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("route"));
    }

    #[test]
    fn test_fallback_without_tokio_runtime() {
        // Outside a tokio runtime, classify_query falls back to Hybrid
        let route = classify_query("some query");
        assert_eq!(route, RetrievalRoute::Hybrid);
    }
}
