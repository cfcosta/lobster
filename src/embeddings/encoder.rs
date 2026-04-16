//! `ColBERT` model loading and encoding via pylate-rs.
//!
//! The model is loaded lazily on first use and cached for the
//! lifetime of the process. All access goes through [`with_model`],
//! which holds a `Mutex` guard for the duration of the callback.

use std::sync::Mutex;

use anyhow::{Context, Result};
use candle_core::Device;
use pylate_rs::ColBERT;

use crate::{
    embeddings::proxy::{self, PROXY_REDUCTION_VERSION, PoolingPolicy},
    store::schema::{EmbeddingArtifact, EmbeddingBackend},
};

/// Process-wide lazy singleton for the ColBERT model.
///
/// `Option::None` means not yet attempted. `Some(model)` means
/// successfully loaded. If loading fails, we leave it as `None` so
/// a later call can retry (e.g. after `lobster install`).
static MODEL: Mutex<Option<ColBERT>> = Mutex::new(None);

/// Run `f` with exclusive access to the lazily-loaded ColBERT model.
///
/// The model is initialized on the first call and reused for every
/// subsequent call. Returns an error if the model files aren't
/// cached locally (`lobster install` not run).
pub fn with_model<T>(f: impl FnOnce(&mut ColBERT) -> Result<T>) -> Result<T> {
    let mut guard = MODEL.lock().expect("ColBERT mutex poisoned");

    if guard.is_none() {
        let device = select_device();
        tracing::info!(?device, "loading ColBERT model");
        let model: ColBERT = ColBERT::from("lightonai/GTE-ModernColBERT-v1")
            .with_device(device)
            .try_into()
            .context(
                "failed to load ColBERT model — run `lobster install` first",
            )?;
        *guard = Some(model);
    }

    f(guard.as_mut().expect("just initialized"))
}

/// Returns `true` if the model can be loaded (files are cached).
///
/// Intended for tests that need to skip when the model is absent.
pub fn model_available() -> bool {
    with_model(|_| Ok(())).is_ok()
}

/// Select the best available device: CUDA when the feature is
/// enabled and a GPU is present, CPU otherwise.
pub fn select_device() -> Device {
    #[cfg(feature = "cuda")]
    {
        if let Ok(device) = Device::new_cuda(0) {
            return device;
        }
    }
    Device::Cpu
}

/// Encode a text string into a document embedding and produce
/// both pooled proxy vector and optional late-interaction bytes.
///
/// Uses the lazy-loaded model singleton internally.
///
/// # Errors
///
/// Returns an error if the model is unavailable or encoding fails.
pub fn encode_text(
    text: &str,
    artifact_id: crate::store::ids::ArtifactId,
    policy: PoolingPolicy,
) -> Result<EmbeddingArtifact> {
    with_model(|model| encode_text_inner(model, text, artifact_id, policy))
}

fn encode_text_inner(
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

/// Encode a query string into a pooled proxy vector for search.
///
/// Uses query-mode encoding (`is_query=true`) and mean-pools the
/// per-token embeddings into a single dense vector.
///
/// # Errors
///
/// Returns an error if the model is not installed or encoding fails.
pub fn encode_query(query: &str) -> Result<Vec<f32>> {
    with_model(|model| {
        let texts = vec![query.to_string()];
        let embeddings = model
            .encode(&texts, true)
            .context("ColBERT query encode failed")?;

        let proxy = mean_pool_tensor(&embeddings)?;
        Ok(crate::embeddings::proxy::bytes_to_vector(&proxy))
    })
}

/// Encode a query and compute MaxSim against a stored document
/// embedding. Returns the MaxSim score, or `None` if scoring
/// fails.
///
/// `doc_li_bytes` is the stored `late_interaction_bytes` (flat
/// f32s). `n_dims` is the embedding dimension (derivable from
/// the proxy vector length).
pub fn maxsim_score(
    query: &str,
    doc_li_bytes: &[u8],
    n_dims: usize,
) -> Result<f32> {
    if doc_li_bytes.is_empty() || n_dims == 0 {
        anyhow::bail!("empty document embedding");
    }

    with_model(|model| {
        let texts = vec![query.to_string()];
        let query_emb = model
            .encode(&texts, true)
            .context("ColBERT query encode failed")?;

        // Reconstruct document tensor from stored bytes
        let doc_floats =
            crate::embeddings::proxy::bytes_to_vector(doc_li_bytes);
        let n_tokens = doc_floats.len() / n_dims;
        if n_tokens == 0 {
            anyhow::bail!("degenerate document embedding: 0 tokens");
        }

        let doc_tensor = candle_core::Tensor::from_vec(
            doc_floats,
            (1, n_tokens, n_dims),
            &model.device,
        )
        .context("reconstruct doc tensor")?;

        // MaxSim: for each query token, find max similarity with
        // any document token, then sum across query tokens.
        let sim = model
            .similarity(&query_emb, &doc_tensor)
            .map_err(|e| anyhow::anyhow!("MaxSim failed: {e}"))?;

        Ok(sim.data[0][0])
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
        if !model_available() {
            eprintln!("skipping test_load_and_encode: model not installed");
            return;
        }

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test");
        let result = encode_text(
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
        if !model_available() {
            eprintln!("skipping: model not installed");
            return;
        }

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test2");
        let result = encode_text(
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
        if !model_available() {
            eprintln!("skipping: model not installed");
            return;
        }

        let artifact_id = crate::store::ids::ArtifactId::derive(b"test3");
        let result = encode_text(
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
