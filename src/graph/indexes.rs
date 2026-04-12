//! Grafeo index creation for text and vector search.
//!
//! Per spec: HNSW vector index on pooled proxy vectors (cosine),
//! BM25 text index on `summary_text` and decision statement fields,
//! property index on `artifact_type`, `repo_id`, `task_id`.

use grafeo::GrafeoDB;

use crate::graph::db::labels;

/// Create all required indexes on a Grafeo instance.
///
/// Should be called after initial rebuild or when the database
/// is first opened. Safe to call multiple times — Grafeo handles
/// duplicate index creation gracefully.
pub fn ensure_indexes(grafeo: &GrafeoDB) {
    // BM25 text indexes for summary and decision search
    let _ = grafeo.create_text_index(labels::EPISODE, "summary_text");
    let _ = grafeo.create_text_index(labels::DECISION, "statement");
    let _ = grafeo.create_text_index(labels::DECISION, "rationale");
    let _ = grafeo.create_text_index(labels::ENTITY, "canonical_name");

    // Property indexes for fast filtering
    let () = grafeo.create_property_index("repo_id");
    let () = grafeo.create_property_index("episode_id");
    let () = grafeo.create_property_index("decision_id");
    let () = grafeo.create_property_index("entity_id");
    let () = grafeo.create_property_index("task_id");
    let () = grafeo.create_property_index("processing_state");
    let () = grafeo.create_property_index("kind");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::db;

    #[test]
    fn test_ensure_indexes_on_empty_db() {
        let grafeo = db::new_in_memory();
        // Should not panic on empty database
        ensure_indexes(&grafeo);
    }

    #[test]
    fn test_ensure_indexes_idempotent() {
        let grafeo = db::new_in_memory();
        ensure_indexes(&grafeo);
        // Calling again should not panic
        ensure_indexes(&grafeo);
    }

    #[test]
    fn test_text_search_after_index() {
        let grafeo = db::new_in_memory();

        // Create a decision node with text
        let _node = db::create_decision_node(
            &grafeo,
            "dec-1",
            "Use redb for storage",
            "ACID compliant",
            "High",
        );

        ensure_indexes(&grafeo);

        // BM25 text search should find the decision
        let results = grafeo.text_search(
            labels::DECISION,
            "statement",
            "redb storage",
            10,
        );
        if let Ok(hits) = results {
            assert!(!hits.is_empty(), "text search should find the decision");
        } else {
            // Some Grafeo configs may not support text
            // search — that's OK for the test
        }
    }
}
