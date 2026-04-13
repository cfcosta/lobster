//! `PyLate` `ColBERT` reranking.
//!
//! Encodes the query with `ColBERT` and computes cosine similarity
//! against candidate proxy vectors for reranking.

use redb::Database;

use crate::{
    embeddings::proxy,
    rank::retrieval::cosine_similarity,
    store::{crud, ids::RawId},
};

/// Compute the reranking score for a candidate.
///
/// Encodes the query with `ColBERT` to produce a proxy vector, then
/// computes cosine similarity against the candidate's stored proxy
/// vector. Returns 0.0 if the model or embeddings are unavailable.
#[must_use]
pub fn rerank_score(
    db: &Database,
    query_text: &str,
    candidate_id: &RawId,
) -> f64 {
    // Try to load embedding artifact
    let Ok(emb) = crud::get_embedding_artifact(db, candidate_id) else {
        return 0.0;
    };

    let doc_proxy = proxy::bytes_to_vector(&emb.pooled_vector_bytes);
    if doc_proxy.is_empty() {
        return 0.0;
    }

    // Encode query with ColBERT for a real semantic proxy vector
    let Some(query_proxy) = encode_query_proxy(query_text) else {
        return 0.0;
    };

    cosine_similarity(&query_proxy, &doc_proxy)
}

/// Encode a query string into a proxy vector using `ColBERT`.
///
/// Returns `None` if the model is unavailable.
fn encode_query_proxy(query_text: &str) -> Option<Vec<f32>> {
    let mut model = crate::embeddings::encoder::load_model().ok()?;
    let texts = vec![query_text.to_string()];
    let embeddings = model.encode(&texts, true).ok()?;

    let shape = embeddings.shape();
    let dims = shape.dims();
    if dims.len() < 2 {
        return None;
    }
    let n_dims = *dims.last()?;
    let flat: Vec<f32> = embeddings
        .flatten(0, dims.len() - 2)
        .ok()?
        .to_vec2::<f32>()
        .ok()?
        .into_iter()
        .flatten()
        .collect();

    Some(proxy::mean_pool(&flat, n_dims))
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
        let database = db::open_in_memory().unwrap();
        let id = ArtifactId::derive(b"missing").raw();
        let score = rerank_score(&database, "test", &id);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn test_rerank_with_proxy_vector() {
        let database = db::open_in_memory().unwrap();

        // Create an embedding artifact with a known proxy
        let id = ArtifactId::derive(b"test");
        let proxy_vec = vec![0.5_f32; 16];
        let art = EmbeddingArtifact {
            artifact_id: id,
            revision: "test".into(),
            backend: EmbeddingBackend::Cpu,
            quantization: None,
            pooled_vector_bytes: proxy::vector_to_bytes(&proxy_vec),
            late_interaction_bytes: None,
            payload_checksum: [0; 32],
        };
        crud::put_embedding_artifact(&database, &art).unwrap();

        let score = rerank_score(&database, "test query", &id.raw());
        // When model is available: real cosine similarity in [-1, 1]
        // When model is unavailable: returns 0.0 (graceful fallback)
        assert!(
            (-1.0..=1.0).contains(&score),
            "score should be in valid range: {score}"
        );
    }

    #[test]
    fn test_rerank_with_model() {
        if crate::embeddings::encoder::load_model().is_err() {
            eprintln!("skipping test_rerank_with_model: model not installed");
            return;
        }

        let database = db::open_in_memory().unwrap();

        // Encode a real document and store its proxy
        let mut model =
            crate::embeddings::encoder::load_model().unwrap();
        let texts = vec!["Use redb for ACID storage".to_string()];
        let emb = model.encode(&texts, false).unwrap();
        let shape = emb.shape();
        let n_dims = *shape.dims().last().unwrap();
        let flat: Vec<f32> = emb
            .flatten(0, shape.dims().len() - 2)
            .unwrap()
            .to_vec2::<f32>()
            .unwrap()
            .into_iter()
            .flatten()
            .collect();
        let doc_proxy = proxy::mean_pool(&flat, n_dims);

        let id = ArtifactId::derive(b"real-test");
        let art = EmbeddingArtifact {
            artifact_id: id,
            revision: "test".into(),
            backend: EmbeddingBackend::Cpu,
            quantization: None,
            pooled_vector_bytes: proxy::vector_to_bytes(&doc_proxy),
            late_interaction_bytes: None,
            payload_checksum: [0; 32],
        };
        crud::put_embedding_artifact(&database, &art).unwrap();

        // Query about the same topic should have positive similarity
        let score =
            rerank_score(&database, "redb storage ACID", &id.raw());
        assert!(
            score > 0.0,
            "same-topic query should have positive similarity: {score}"
        );
    }
}
