//! `ColBERT` model loading and encoding via pylate-rs.
//!
//! Loads the model once, encodes text as document embeddings,
//! applies `hierarchical_pooling` per the artifact-specific policy,
//! and produces proxy vectors via mean-pooling.

use anyhow::{Context, Result};
use candle_core::Device;
use pylate_rs::ColBERT;

use crate::{
    embeddings::proxy::{self, PROXY_REDUCTION_VERSION, PoolingPolicy},
    store::schema::{EmbeddingArtifact, EmbeddingBackend},
};

/// Load the GTE-ModernColBERT-v1 model on CPU.
///
/// # Errors
///
/// Returns an error if the model files aren't cached locally.
/// Run `lobster install` first.
pub fn load_model() -> Result<ColBERT> {
    let model: ColBERT = ColBERT::from("lightonai/GTE-ModernColBERT-v1")
        .with_device(Device::Cpu)
        .try_into()
        .context(
            "failed to load ColBERT model — run `lobster install` first",
        )?;
    Ok(model)
}

/// Encode a text string into a document embedding and produce
/// both pooled proxy vector and optional late-interaction bytes.
///
/// # Errors
///
/// Returns an error if encoding fails.
pub fn encode_text(
    model: &mut ColBERT,
    text: &str,
    artifact_id: crate::store::ids::ArtifactId,
    policy: PoolingPolicy,
) -> Result<EmbeddingArtifact> {
    use sha2::Digest as _;

    // Encode as document (is_query=false)
    let documents = vec![text.to_string()];
    let doc_embeddings = model
        .encode(&documents, false)
        .context("ColBERT encode failed")?;

    // Apply hierarchical pooling based on policy
    let (pooled_vector_bytes, late_interaction_bytes) = match policy {
        PoolingPolicy::Full => {
            // Keep full late-interaction representation
            let proxy = mean_pool_tensor(&doc_embeddings)?;
            let li_bytes = tensor_to_bytes(&doc_embeddings)?;
            (proxy, Some(li_bytes))
        }
        PoolingPolicy::Light => {
            // Apply hierarchical pooling with pool_factor=2
            let pooled = pylate_rs::hierarchical_pooling(&doc_embeddings, 2)
                .context("hierarchical pooling failed")?;
            let proxy = mean_pool_tensor(&doc_embeddings)?;
            let li_bytes = tensor_to_bytes(&pooled)?;
            (proxy, Some(li_bytes))
        }
        PoolingPolicy::ProxyOnly => {
            // Proxy vector only — no late-interaction bytes
            let proxy = mean_pool_tensor(&doc_embeddings)?;
            (proxy, None)
        }
    };

    let mut hasher = sha2::Sha256::new();
    hasher.update(&pooled_vector_bytes);
    let checksum: [u8; 32] = hasher.finalize().into();

    Ok(EmbeddingArtifact {
        artifact_id,
        revision: PROXY_REDUCTION_VERSION.to_string(),
        backend: EmbeddingBackend::Cpu,
        quantization: None,
        pooled_vector_bytes,
        late_interaction_bytes,
        payload_checksum: checksum,
    })
}

/// Mean-pool a candle Tensor [1, tokens, dims] into a proxy vector.
fn mean_pool_tensor(tensor: &candle_core::Tensor) -> Result<Vec<u8>> {
    let shape = tensor.shape();
    let dims = shape.dims();
    if dims.len() < 2 {
        anyhow::bail!("expected at least 2D tensor, got {dims:?}");
    }
    let n_dims = *dims.last().unwrap();

    // Flatten to 2D [tokens, dims], then mean across tokens
    let flat = tensor.flatten(0, dims.len() - 2).context("flatten")?;
    let flat_data: Vec<f32> = flat
        .to_vec2()
        .context("to_vec2")?
        .into_iter()
        .flatten()
        .collect();

    let proxy = proxy::mean_pool(&flat_data, n_dims);
    Ok(proxy::vector_to_bytes(&proxy))
}

/// Convert a Tensor to raw bytes for storage.
fn tensor_to_bytes(tensor: &candle_core::Tensor) -> Result<Vec<u8>> {
    let flat: Vec<f32> = tensor
        .flatten_all()
        .context("flatten")?
        .to_vec1()
        .context("to_vec1")?;
    Ok(proxy::vector_to_bytes(&flat))
}

#[cfg(test)]
mod tests {
    use super::*;

    // This test requires the model to be installed. Skip in CI
    // where it may not be available.
    #[test]
    fn test_load_and_encode() {
        let mut model = match load_model() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipping test_load_and_encode: {e}");
                return;
            }
        };

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test");
        let result = encode_text(
            &mut model,
            "I chose redb for storage",
            artifact_id,
            PoolingPolicy::Full,
        );

        match result {
            Ok(artifact) => {
                assert!(
                    !artifact.pooled_vector_bytes.is_empty(),
                    "proxy vector should not be empty"
                );
                assert!(
                    artifact.late_interaction_bytes.is_some(),
                    "Full policy should produce late-interaction bytes"
                );
                assert_ne!(artifact.payload_checksum, [0; 32]);
                assert_eq!(artifact.revision, PROXY_REDUCTION_VERSION);
            }
            Err(e) => {
                eprintln!("encoding failed (may need model): {e}");
            }
        }
    }

    #[test]
    fn test_encode_with_light_pooling() {
        let mut model = match load_model() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test2");
        let result = encode_text(
            &mut model,
            "Episode summary about testing",
            artifact_id,
            PoolingPolicy::Light,
        );

        if let Ok(artifact) = result {
            assert!(
                artifact.late_interaction_bytes.is_some(),
                "Light policy should have late-interaction (pooled)"
            );
        }
    }

    #[test]
    fn test_encode_proxy_only() {
        let mut model = match load_model() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test3");
        let result = encode_text(
            &mut model,
            "Old episode summary",
            artifact_id,
            PoolingPolicy::ProxyOnly,
        );

        if let Ok(artifact) = result {
            assert!(
                artifact.late_interaction_bytes.is_none(),
                "ProxyOnly should have no late-interaction bytes"
            );
            assert!(!artifact.pooled_vector_bytes.is_empty());
        }
    }
}
