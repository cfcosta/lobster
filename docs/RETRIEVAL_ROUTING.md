# Retrieval routing spec

Lobster uses **deterministic query routing** to select the cheapest retrieval path that satisfies each query class. The router runs before any search, not after.

## Why routing

GraphRAG underperforms plain RAG on simple tasks. Always invoking graph expansion wastes latency and can dilute precision. Always trusting dense retrieval misses relational structure. A deterministic classifier selects the right path per query.

Based on: EA-GraphRAG (routes simple vs complex), SelRoute (lexical/semantic/hybrid/enriched by type), Mem0 (vector as ordering backbone, graph alongside), MemX (multi-factor reranking + rejection).

---

## Routes

### 1. Exact / lexical

**Triggers**: query contains file paths, symbol names, error strings, ISO dates, named decision IDs, task IDs, or entity canonical names.

**Detection**: regex/heuristic match on structured patterns. No embedding required.

**Execution**:

1. Grafeo property-index lookup or BM25 text search (exact match mode)
2. Intersect with `redb` ready set
3. No reranking needed (exact matches are self-evident)

**Candidate budget**: up to 5 results.

**Confidence threshold**: exact matches always pass (score = 1.0).

### 2. Hybrid (BM25 + vector)

**Triggers**: ordinary natural-language recall query that does not match exact patterns and does not contain relational keywords.

**Detection**: default route when exact/lexical does not match and relational signals are absent.

**Execution**:

1. Grafeo hybrid search: BM25 over text indexes + HNSW over pooled proxy vectors
2. Fetch top-K candidates (K = route-specific budget)
3. Rerank with PyLate MaxSim when `late_interaction_bytes` available, otherwise pooled-vector cosine
4. Apply composite scoring: `semantic + recency + task_overlap + decision_bonus - noise_penalty`
5. Apply MMR diversity (lambda = 0.7) to deduplicate near-identical results
6. Intersect with `redb` ready set
7. Reject below confidence threshold

**Candidate budget**: top-K = 20 candidates, surface up to 3 after reranking.

**Confidence threshold**: reject if best reranked score < 0.4 (normalized composite).

### 3. Hybrid + graph expansion

**Triggers**: query contains relational/causal language: "why", "how did", "what depends on", "related to", "what changed", "history of", "timeline", "because", "led to", "caused by", or explicit graph-traversal keywords.

**Detection**: keyword/pattern match on relational signals.

**Execution**:

1. Run hybrid search (same as route 2) to get initial candidates
2. For top candidates, expand via Grafeo graph neighborhood traversal:
   - 1-hop evidence-backed edges from matched nodes
   - filter expanded neighbors by temporal validity (`valid_to` is null or > now)
   - include edge type, evidence refs, and temporal metadata
3. Merge expanded context into candidate set
4. Rerank the full set (original + expanded) with composite scoring
5. Apply MMR diversity (lambda = 0.6, slightly more diverse for relational queries)
6. Intersect with `redb` ready set
7. Reject below confidence threshold

**Candidate budget**: top-K = 30 candidates (before expansion), surface up to 5 after reranking.

**Confidence threshold**: reject if best reranked score < 0.35 (slightly lower, since graph context adds value even at moderate confidence).

### 4. Abstain

**Triggers**: the route classifier fires but no route produces results above its confidence threshold.

**Execution**: return nothing. Silence is preferable to weak evidence.

For automatic recall (hooks), abstain is the default posture. For explicit MCP recall, abstain may return a structured "no confident results" response with metadata about what was searched.

---

## Route classifier

The classifier is a deterministic function, not an LLM call. It runs in-process, synchronously, before any retrieval.

```rust
enum RetrievalRoute {
    Exact,
    Hybrid,
    HybridGraph,
    Abstain,
}

fn classify_query(query: &str) -> RetrievalRoute {
    if matches_exact_patterns(query) {
        return RetrievalRoute::Exact;
    }
    if matches_relational_signals(query) {
        return RetrievalRoute::HybridGraph;
    }
    RetrievalRoute::Hybrid
}
```

### Exact patterns (regex/heuristic)

- File paths: `/`, `src/`, `.rs`, `.toml`, `.md`, etc.
- Symbol names: `CamelCase`, `snake_case` identifiers with `::` or `.` qualification
- Error strings: `error`, `panic`, `failed`, `E0`, stack trace fragments
- ISO dates: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM`
- Decision/task IDs: `decision:`, `task:`, `episode:` prefixed identifiers
- Entity canonical names: exact match against known entity names from `redb`

### Relational signals (keyword match)

- Causal: "why", "because", "caused by", "led to", "resulted in"
- Dependency: "depends on", "related to", "connected to", "linked to"
- Change: "what changed", "how did", "history of", "timeline"
- Traversal: "neighbors", "adjacent", "surrounding", "context of"

---

## Confidence thresholds

| Route       | Automatic recall  | Explicit MCP recall |
| ----------- | ----------------- | ------------------- |
| Exact       | 1.0 (always pass) | 1.0 (always pass)   |
| Hybrid      | 0.4               | 0.25                |
| HybridGraph | 0.35              | 0.20                |

Thresholds are on the normalized composite score (0.0 to 1.0). MCP thresholds are lower because the user explicitly asked for recall and can tolerate lower confidence.

---

## Candidate budgets

| Route       | Fetch K | Surface (auto) | Surface (MCP) |
| ----------- | ------- | -------------- | ------------- |
| Exact       | 5       | 3              | 5             |
| Hybrid      | 20      | 3              | 10            |
| HybridGraph | 30      | 5              | 15            |

"Fetch K" is how many candidates are retrieved from Grafeo before reranking. "Surface" is the maximum returned after reranking, rejection, and diversity filtering.

---

## Composite scoring

```text
score = w_sem * semantic_score
      + w_rec * recency_score
      + w_task * task_overlap_score
      + w_graph * graph_support_score
      + w_dec * decision_bonus
      - w_noise * noise_penalty
```

### Score components

- **semantic_score** (0.0–1.0): PyLate MaxSim reranking score (normalized), or cosine similarity for proxy-only artifacts.
- **recency_score** (0.0–1.0): exponential decay from artifact timestamp. Half-life tunable per artifact class.
- **task_overlap_score** (0.0 or 1.0): 1.0 if the artifact shares a `task_id` with the current context, 0.0 otherwise.
- **graph_support_score** (0.0–1.0): fraction of the artifact's graph neighbors that also appear in the candidate set. Only computed for HybridGraph route.
- **decision_bonus** (0.0 or fixed bonus): added for decision artifacts, which are higher-value recall targets.
- **noise_penalty** (0.0–1.0): penalty for artifacts flagged as low-signal during dreaming/maintenance.

### Default weights

| Weight  | Value | Notes                               |
| ------- | ----- | ----------------------------------- |
| w_sem   | 0.40  | Dominant signal                     |
| w_rec   | 0.20  | Recent artifacts preferred          |
| w_task  | 0.15  | Same-task context is strong signal  |
| w_graph | 0.10  | Only active for HybridGraph route   |
| w_dec   | 0.10  | Decisions get a fixed bonus         |
| w_noise | 0.05  | Light penalty for flagged artifacts |

Weights are versioned and fixture-tested. Changes to weights constitute a ranking version bump.

### Normalization

The composite score is normalized to [0.0, 1.0] by dividing by the maximum achievable score (sum of positive weights + decision bonus for the best case).

### Stable tie-breakers

When composite scores are equal:

1. artifact priority (decision > task > summary > entity)
2. timestamp (newer first)
3. stable artifact ID (lexicographic)

---

## Diversity control (MMR)

After scoring and before final selection, apply Maximal Marginal Relevance to remove near-duplicates:

```text
MMR(d) = lambda * score(d) - (1 - lambda) * max_sim(d, selected)
```

| Route       | Lambda | Effect                                     |
| ----------- | ------ | ------------------------------------------ |
| Exact       | 1.0    | No diversity penalty (exact matches)       |
| Hybrid      | 0.7    | Moderate diversity                         |
| HybridGraph | 0.6    | More diversity (graph expansion adds bulk) |

`max_sim(d, selected)` is the maximum PyLate or cosine similarity between candidate `d` and any already-selected result.

---

## Evidence-window expansion

After selecting final results, expand each surfaced item with its evidence window:

### Decisions

```
{
  statement: "...",
  rationale: "...",
  confidence: "high",
  evidence: [{ episode_id, span_summary }],
  task_context: { task_id, title, status }
}
```

### Summaries

```
{
  summary_text: "...",
  episode_id: "...",
  decisions_supported: [{ decision_id, statement }],
  task_context: { task_id, title }
}
```

### Entities

```
{
  canonical_name: "...",
  kind: "component",
  related_decisions: [{ decision_id, statement }],
  related_tasks: [{ task_id, title }]
}
```

Evidence expansion is bounded: at most 3 evidence refs per decision, at most 2 supported decisions per summary.

---

## Eval matrix

Retrieval quality is evaluated per route on fixture datasets.

### Metrics per route

| Metric          | What it measures                                                   |
| --------------- | ------------------------------------------------------------------ |
| Precision@K     | Fraction of surfaced results that are relevant                     |
| Recall@K        | Fraction of relevant results in the corpus that were surfaced      |
| Rejection rate  | Fraction of queries where the router correctly abstained           |
| False silence   | Fraction of queries where the router abstained but should not have |
| Latency p50/p99 | End-to-end retrieval latency (classifier + search + rerank)        |
| Diversity score | 1 - mean pairwise similarity among surfaced results                |

### Fixture dataset requirements

Each route needs a dedicated fixture set:

- **Exact**: 50+ queries with known exact-match targets (file paths, decision IDs, symbols)
- **Hybrid**: 50+ natural-language queries with labeled relevant artifacts
- **HybridGraph**: 30+ relational queries ("why did we choose X", "what depends on Y") with labeled relevant subgraphs
- **Abstain**: 20+ queries with no relevant artifacts in the corpus (should produce silence)

### Determinism requirement

Same query + same corpus state + same Lobster release = same routing decision, same candidate set, same ranking, same output. This is verified by golden-test comparison in CI.

### Threshold tuning

Confidence thresholds and weights are tuned on the fixture datasets. The tuning process:

1. Run all fixture queries through the pipeline
2. Measure Precision@K and false-silence rate per route
3. Adjust thresholds to maximize Precision@K while keeping false-silence below 5%
4. Freeze thresholds as versioned constants
5. Re-run golden tests to verify determinism

Threshold changes constitute a retrieval version bump and require re-running the full fixture suite.
