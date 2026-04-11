//! Deterministic query route classifier.
//!
//! Runs before any search to select the cheapest retrieval path.
//! This is a deterministic function, not an LLM call.

use crate::rank::route::RetrievalRoute;

/// Classify a query into a retrieval route.
///
/// Order:
/// 1. Check for exact patterns (file paths, symbols, IDs, dates)
/// 2. Check for relational signals (causal, dependency, change)
/// 3. Default to Hybrid
#[must_use]
pub fn classify_query(query: &str) -> RetrievalRoute {
    if matches_exact_patterns(query) {
        return RetrievalRoute::Exact;
    }
    if matches_relational_signals(query) {
        return RetrievalRoute::HybridGraph;
    }
    RetrievalRoute::Hybrid
}

/// Detect exact/lexical patterns in a query.
fn matches_exact_patterns(query: &str) -> bool {
    // File paths: contains / or common extensions
    if query.contains('/')
        || query.contains(".rs")
        || query.contains(".toml")
        || query.contains(".md")
        || query.contains(".json")
        || query.contains(".ts")
        || query.contains(".py")
    {
        return true;
    }

    // Error strings
    let lower = query.to_lowercase();
    if lower.contains("error[e")
        || lower.contains("panic at")
        || lower.contains("stack trace")
    {
        return true;
    }

    // Decision/task/episode ID prefixes
    if query.starts_with("decision:")
        || query.starts_with("task:")
        || query.starts_with("episode:")
        || query.starts_with("entity:")
    {
        return true;
    }

    // ISO dates
    if has_iso_date(query) {
        return true;
    }

    // Qualified symbol names (Foo::bar, foo.bar_baz)
    if has_qualified_symbol(query) {
        return true;
    }

    false
}

/// Detect relational/causal signals.
fn matches_relational_signals(query: &str) -> bool {
    let lower = query.to_lowercase();
    let relational_keywords = [
        // Causal
        "why ",
        "because",
        "caused by",
        "led to",
        "resulted in",
        // Dependency
        "depends on",
        "related to",
        "connected to",
        "linked to",
        // Change
        "what changed",
        "how did",
        "history of",
        "timeline",
        // Traversal
        "neighbors",
        "adjacent",
        "surrounding",
        "context of",
    ];
    relational_keywords.iter().any(|kw| lower.contains(kw))
}

fn has_iso_date(query: &str) -> bool {
    // Simple pattern: YYYY-MM-DD
    let bytes = query.as_bytes();
    if bytes.len() < 10 {
        return false;
    }
    for window in bytes.windows(10) {
        if window[4] == b'-'
            && window[7] == b'-'
            && window[..4].iter().all(u8::is_ascii_digit)
            && window[5..7].iter().all(u8::is_ascii_digit)
            && window[8..10].iter().all(u8::is_ascii_digit)
        {
            return true;
        }
    }
    false
}

fn has_qualified_symbol(query: &str) -> bool {
    // Foo::bar or foo.bar with at least one letter on each side
    query.contains("::")
        || query
            .split('.')
            .filter(|p| {
                !p.is_empty()
                    && p.chars().next().is_some_and(char::is_alphabetic)
            })
            .count()
            >= 2
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: classify_query is a total function --
    // Never panics, always returns a valid route.
    #[hegel::test(test_cases = 500)]
    fn prop_classify_never_panics(tc: TestCase) {
        let query: String = tc.draw(gs::text().max_size(500));
        let route = classify_query(&query);
        // Must be one of the known variants
        assert!(matches!(
            route,
            RetrievalRoute::Exact
                | RetrievalRoute::Hybrid
                | RetrievalRoute::HybridGraph
        ));
        // Never returns Abstain (that's a post-search decision)
        assert_ne!(route, RetrievalRoute::Abstain);
    }

    // -- Property: classify_query is deterministic --
    #[hegel::test(test_cases = 500)]
    fn prop_classify_deterministic(tc: TestCase) {
        let query: String = tc.draw(gs::text().max_size(200));
        let r1 = classify_query(&query);
        let r2 = classify_query(&query);
        assert_eq!(r1, r2, "same query must return same route");
    }

    // -- Unit tests: exact patterns --
    #[test]
    fn test_file_paths_are_exact() {
        assert_eq!(classify_query("src/main.rs"), RetrievalRoute::Exact);
        assert_eq!(classify_query("Cargo.toml"), RetrievalRoute::Exact);
        assert_eq!(
            classify_query("docs/ARCHITECTURE.md"),
            RetrievalRoute::Exact
        );
    }

    #[test]
    fn test_error_strings_are_exact() {
        assert_eq!(
            classify_query("error[E0277]: trait bound"),
            RetrievalRoute::Exact
        );
        assert_eq!(
            classify_query("panic at 'index out of bounds'"),
            RetrievalRoute::Exact
        );
    }

    #[test]
    fn test_id_prefixes_are_exact() {
        assert_eq!(classify_query("decision:abc123"), RetrievalRoute::Exact);
        assert_eq!(classify_query("task:build-memory"), RetrievalRoute::Exact);
    }

    #[test]
    fn test_iso_dates_are_exact() {
        assert_eq!(classify_query("2024-01-15"), RetrievalRoute::Exact);
    }

    #[test]
    fn test_qualified_symbols_are_exact() {
        assert_eq!(classify_query("Store::get_episode"), RetrievalRoute::Exact);
    }

    // -- Unit tests: relational signals --
    #[test]
    fn test_causal_queries_are_hybrid_graph() {
        assert_eq!(
            classify_query("why did we choose redb?"),
            RetrievalRoute::HybridGraph
        );
        assert_eq!(
            classify_query("what was caused by the test change"),
            RetrievalRoute::HybridGraph
        );
    }

    #[test]
    fn test_dependency_queries_are_hybrid_graph() {
        assert_eq!(
            classify_query("what depends on Grafeo?"),
            RetrievalRoute::HybridGraph
        );
        assert_eq!(
            classify_query("components related to retrieval"),
            RetrievalRoute::HybridGraph
        );
    }

    #[test]
    fn test_change_queries_are_hybrid_graph() {
        assert_eq!(
            classify_query("what changed in the storage layer"),
            RetrievalRoute::HybridGraph
        );
        assert_eq!(
            classify_query("history of this decision"),
            RetrievalRoute::HybridGraph
        );
    }

    // -- Unit tests: default hybrid --
    #[test]
    fn test_plain_queries_are_hybrid() {
        assert_eq!(
            classify_query("memory search implementation"),
            RetrievalRoute::Hybrid
        );
        assert_eq!(
            classify_query("how to use the MCP tools"),
            RetrievalRoute::Hybrid
        );
    }

    #[test]
    fn test_empty_query_is_hybrid() {
        assert_eq!(classify_query(""), RetrievalRoute::Hybrid);
    }
}
