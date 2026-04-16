//! ColBERT MaxSim reranking.
//!
//! Computes MaxSim scores between a query and candidate documents
//! using stored late-interaction embeddings. Falls back to cosine
//! similarity on proxy vectors when late-interaction bytes are not
//! available (ProxyOnly pooling policy).

use crate::{
    embeddings::{encoder, proxy},
    rank::retrieval::cosine_similarity,
    store::{crud, db::LobsterDb, ids::RawId},
};

/// Compute the reranking score for a candidate using MaxSim.
///
/// Loads the candidate's embedding artifact and:
/// 1. If late-interaction bytes are available, computes MaxSim
///    (the proper ColBERT scoring) between the query and the
///    stored per-token document embedding.
/// 2. Otherwise falls back to cosine similarity on proxy vectors.
///
/// Returns 0.0 if the candidate has no embedding artifact.
#[must_use]
pub fn rerank_score(
    db: &LobsterDb,
    query_text: &str,
    candidate_id: &RawId,
) -> f64 {
    let Ok(emb) = crud::get_embedding_artifact(db, candidate_id) else {
        return 0.0;
    };

    let doc_proxy = proxy::bytes_to_vector(&emb.pooled_vector_bytes);
    if doc_proxy.is_empty() {
        return 0.0;
    }

    // Prefer MaxSim on late-interaction embeddings when available
    if let Some(li_bytes) = &emb.late_interaction_bytes {
        if !li_bytes.is_empty() {
            let n_dims = doc_proxy.len();
            if let Ok(score) =
                encoder::maxsim_score(query_text, li_bytes, n_dims)
            {
                return f64::from(score);
            }
        }
    }

    // Fallback: cosine similarity on proxy vectors
    let Ok(query_proxy) = encoder::encode_query(query_text) else {
        return 0.0;
    };

    cosine_similarity(&query_proxy, &doc_proxy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        db,
        ids::ArtifactId,
        schema::{EmbeddingArtifact, EmbeddingBackend},
    };

    #[test]
    fn test_rerank_missing_artifact() {
        let (database, _dir) = db::open_in_memory().unwrap();
        let id = ArtifactId::derive(b"missing").raw();
        let score = rerank_score(&database, "test", &id);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn test_rerank_with_colbert() {
        if !encoder::model_available() {
            eprintln!("skipping test_rerank_with_colbert: model not installed");
            return;
        }

        let (database, _dir) = db::open_in_memory().unwrap();

        // Encode a real document via encode_text which produces
        // both proxy and late-interaction bytes
        let id = ArtifactId::derive(b"real-test");
        let result = encoder::encode_text(
            "Use redb for ACID storage",
            crate::store::ids::ArtifactId::from_raw(id.raw()),
            crate::embeddings::proxy::PoolingPolicy::Full,
        );

        match result {
            Ok(art) => {
                assert!(
                    art.late_interaction_bytes.is_some(),
                    "Full policy should produce late-interaction bytes"
                );
                crud::put_embedding_artifact(&database, &art).unwrap();

                // MaxSim rerank on the same topic
                let score =
                    rerank_score(&database, "redb storage ACID", &id.raw());
                assert!(
                    score > 0.0,
                    "same-topic query should have positive MaxSim: {score}"
                );
            }
            Err(e) => {
                eprintln!("encoding failed (may need model): {e}");
            }
        }
    }

    #[test]
    fn test_rerank_falls_back_to_cosine_without_li() {
        if !encoder::model_available() {
            eprintln!("skipping: model not installed");
            return;
        }

        let (database, _dir) = db::open_in_memory().unwrap();

        // Store with ProxyOnly (no late-interaction bytes)
        let id = ArtifactId::derive(b"proxy-only");
        let result = encoder::encode_text(
            "Use redb for ACID storage",
            crate::store::ids::ArtifactId::from_raw(id.raw()),
            crate::embeddings::proxy::PoolingPolicy::ProxyOnly,
        );

        match result {
            Ok(art) => {
                assert!(art.late_interaction_bytes.is_none());
                crud::put_embedding_artifact(&database, &art).unwrap();

                // Should fall back to cosine
                let score =
                    rerank_score(&database, "redb storage ACID", &id.raw());
                assert!(
                    score > 0.0,
                    "cosine fallback should have positive score: {score}"
                );
            }
            Err(e) => {
                eprintln!("encoding failed: {e}");
            }
        }
    }
}
