# pylate-rs

A high-performance inference engine for [PyLate](https://github.com/lightonai/pylate)
late-interaction (ColBERT-style) models, written in Rust on top of Hugging Face's
[Candle](https://github.com/huggingface/candle) ML framework.

- **Repository (fork)**: <https://github.com/cfcosta/pylate-rs>
- **Upstream**: <https://github.com/lightonai/pylate-rs>
- **docs.rs**: <https://docs.rs/pylate-rs/latest/pylate_rs/>
- **Compatible models**: <https://huggingface.co/collections/lightonai/pylate-6862b571946fe88330d65264>

## What it does

Late-interaction models like ColBERT produce **per-token embeddings** for queries and
documents, then compute similarity via a **MaxSim** operator: for each query token, take
the maximum dot product across all document tokens, then sum the results. This gives
richer matching than single-vector dot products while staying efficient at retrieval time.

pylate-rs provides:

- Model loading from Hugging Face Hub or local directories
- Query and document encoding (with proper prefix/padding/expansion handling)
- MaxSim similarity computation
- Hierarchical pooling for compressing document representations
- Support for ModernBERT and classic BERT architectures

Performance over PyTorch-based PyLate: ~97% faster model loading on CPU, ~81% faster
query throughput on CUDA.

## Adding to your project

```toml
[dependencies]
pylate-rs = "1.0.4"

# You will also need these peer dependencies:
anyhow = "1"
candle-core = "0.10.2"
```

### Feature flags

| Feature      | What it enables                                            |
| ------------ | ---------------------------------------------------------- |
| `default`    | CPU inference, tokenizers/onig, hf-hub for model downloads |
| `cuda`       | NVIDIA GPU via candle CUDA backend + flash attention       |
| `metal`      | Apple GPU (M1/M2/M3) via candle Metal backend              |
| `accelerate` | Apple CPU via Accelerate framework                         |
| `mkl`        | Intel CPU via MKL                                          |
| `wasm`       | WebAssembly target (wasm-bindgen, disables hf-hub)         |
| `python`     | Python bindings via PyO3 + numpy + ndarray                 |

For CUDA support:

```toml
[dependencies]
pylate-rs = { version = "1.0.4", features = ["cuda"] }
```

From the Lobster fork with CUDA:

```toml
[dependencies]
pylate-rs = { git = "https://github.com/cfcosta/pylate-rs.git", features = ["cuda"] }
```

## Key types

### `ColBERT`

The main model struct. Holds the transformer model, linear projection layer, tokenizer,
and all configuration. Created via the builder pattern.

### `ColbertBuilder`

Builder for constructing a `ColBERT` from a Hugging Face repo ID or local path.
Supports overriding batch size, query/document lengths, prefixes, and device.

### `Similarities`

Output of `model.similarity()`. Contains `data: Vec<Vec<f32>>` where `data[i][j]` is
the similarity score between query `i` and document `j`.

### `BaseModel`

Enum abstracting over the underlying transformer architecture:

- `BaseModel::ModernBert(ModernBert)` for ModernBERT models
- `BaseModel::Bert(BertModel)` for classic BERT models

### `ColbertError`

Unified error type covering Candle, tokenizer, JSON, HF Hub, I/O, and operation errors.

### `normalize_l2`

Public utility function that L2-normalizes a tensor along its last dimension.

## Basic usage

**Important**: `encode()` takes `&mut self`, so the model must be declared `let mut`.
`hierarchical_pooling()` returns `anyhow::Result<Tensor>` (not `ColbertError`).

### Load a model and compute similarity

```rust
use anyhow::Result;
use candle_core::Device;
use pylate_rs::ColBERT;

fn main() -> Result<()> {
    let device = Device::Cpu;
    // For CUDA: let device = Device::new_cuda(0)?;

    // Load from Hugging Face Hub. The builder downloads model files automatically.
    let mut model: ColBERT = ColBERT::from("lightonai/GTE-ModernColBERT-v1")
        .with_device(device)
        .try_into()?;

    // Encode queries (is_query=true pads to query_length with [MASK] tokens)
    let queries = vec!["What is the capital of France?".to_string()];
    let query_embeddings = model.encode(&queries, true)?;

    // Encode documents (is_query=false uses batch-longest padding)
    let documents = vec![
        "Paris is the capital of France.".to_string(),
        "Berlin is the capital of Germany.".to_string(),
    ];
    let document_embeddings = model.encode(&documents, false)?;

    // Compute similarity: queries x documents
    let similarities = model.similarity(&query_embeddings, &document_embeddings)?;

    for (doc_idx, score) in similarities.data[0].iter().enumerate() {
        println!("Query 0 vs Document {}: {:.4}", doc_idx, score);
    }

    Ok(())
}
```

### Load from a local directory

If you have model files saved locally in PyLate format, pass the path directly:

```rust
let mut model: ColBERT = ColBERT::from("/path/to/local/model")
    .with_device(Device::Cpu)
    .try_into()?;
```

The directory must contain: `tokenizer.json`, `model.safetensors`, `config.json`,
`config_sentence_transformers.json`, `special_tokens_map.json`, plus `1_Dense/config.json`
and `1_Dense/model.safetensors`.

### Hierarchical pooling

Reduces the number of token embeddings per document by a given factor using Ward
hierarchical clustering. This compresses document representations for faster downstream
search while preserving ranking quality.

```rust
use pylate_rs::{hierarchical_pooling, ColBERT};

let document_embeddings = model.encode(&documents, false)?;

// pool_factor=2 halves the token count per document
let pooled = hierarchical_pooling(&document_embeddings, 2)?;

// Original: [2, 14, 128] -> Pooled: [2, 7, 128]
let similarities = model.similarity(&query_embeddings, &pooled)?;
```

## Advanced usage

### Builder configuration

```rust
let mut model: ColBERT = ColBERT::from("lightonai/GTE-ModernColBERT-v1")
    .with_device(Device::Cpu)
    .with_batch_size(64)                // fallback default: 32
    .with_query_length(64)              // fallback default: 32 (model config may override)
    .with_document_length(256)          // fallback default: 180 (model config may override)
    .with_query_prefix("[Q]".into())
    .with_document_prefix("[D]".into())
    .with_mask_token("[MASK]".into())
    .with_do_query_expansion(true)
    .with_attend_to_expansion_tokens(false)
    .try_into()?;
```

### Raw similarity matrix

For token-level interaction analysis (e.g., visualization), use `raw_similarity`:

```rust
// Returns a 4D tensor: [num_queries, num_documents, query_tokens, doc_tokens]
let raw_sim = model.raw_similarity(&query_embeddings, &document_embeddings)?;
```

### Supported model architectures

The library auto-detects the architecture from `config.json`:

- **`ModernBertModel`**: e.g., `lightonai/GTE-ModernColBERT-v1`
- **`BertForMaskedLM`** / **`BertModel`**: e.g., `lightonai/colbertv2.0`,
  `lightonai/answerai-colbert-small-v1`

## How encoding works internally

- **Query encoding**: Text is prefixed with `query_prefix` (fallback `[Q]`, may be
  overridden by model config), truncated to `query_length` (fallback 32), and padded with
  `[MASK]` tokens (query expansion). If `do_query_expansion` is true, all token embeddings
  including masks are kept. If `attend_to_expansion_tokens` is true, the attention mask
  becomes all-ones so the model attends to MASK expansion tokens.

- **Document encoding**: Text is prefixed with `document_prefix` (fallback `[D]`),
  truncated to `document_length` (fallback 180), padded to batch-longest. Padding token
  embeddings are zeroed out after L2 normalization. For large batches on GPU, documents
  are sorted by token length for more efficient batching.

- **MaxSim**: `queries.unsqueeze(1) @ documents.transpose(1,2).unsqueeze(0)` produces a
  `[Q, D, q_tokens, d_tokens]` tensor. Take max over `d_tokens`, then sum over
  `q_tokens` for the final score.

- **Hierarchical pooling**: Uses Ward linkage clustering (via the `kodama` crate). The
  first token embedding (CLS/prefix) is excluded from clustering and preserved separately.
  On CPU, batch processing is parallelized using Rayon.

## Role in Lobster

In Lobster, pylate-rs is the **local embedding inference component**. It embeds distilled
memory artifacts (decisions, episode summaries, task summaries, constraints). Lobster
controls model provenance directly rather than relying on any upstream library's built-in
embedding path.

The retrieval contract is:

1. Persist a **pooled single-vector proxy** for each distilled artifact
2. Persist **`late_interaction_bytes`** for artifact classes that participate in exact
   PyLate reranking
3. Project the pooled proxy into Grafeo for hybrid search candidate generation
4. Rerank candidates in-process with exact PyLate MaxSim from the persisted
   late-interaction representation
