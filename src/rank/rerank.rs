//! `PyLate` `ColBERT` reranking.
//!
//! Encodes the query with `ColBERT` and computes cosine similarity
//! against candidate proxy vectors for reranking. Assumes the
//! `ColBERT` model is installed.

use crate::{
    embeddings::{encoder, proxy},
    rank::retrieval::cosine_similarity,
    store::{crud, db::LobsterDb, ids::RawId},
};

/// Compute the reranking score for a candidate.
///
/// Encodes the query with `ColBERT` to produce a proxy vector, then
/// computes cosine similarity against the candidate's stored proxy
/// vector. Returns 0.0 if the candidate has no embedding artifact.
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

    let Ok(mut model) = encoder::load_model() else {
        return 0.0;
    };
    let Ok(query_proxy) = encoder::encode_query(&mut model, query_text) else {
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
        let mut model = match encoder::load_model() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipping test_rerank_with_colbert: {e}");
                return;
            }
        };

        let (database, _dir) = db::open_in_memory().unwrap();

        // Encode a real document and store its proxy
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
        let score = rerank_score(&database, "redb storage ACID", &id.raw());
        assert!(
            score > 0.0,
            "same-topic query should have positive similarity: {score}"
        );
    }
}
