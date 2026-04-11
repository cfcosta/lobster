# Lobster architecture spec

## Summary

Lobster is a local, deterministic, per-repo memory system for Claude Code.

Its architecture is **layered memory**:

1. **`redb`** is the canonical source of truth for raw events, episodes, tasks, decisions, jobs, and provenance.
2. **`Grafeo`** is the semantic serving layer for graph facts, hybrid retrieval, and graph-backed expansion.
3. **`pylate-rs`** provides local embedding inference for distilled memory artifacts.
4. **Hooks** provide lightweight automatic recall and ingestion triggers.
5. **MCP tools** provide explicit, deeper recall.
6. **Background dreaming** performs maintenance and consolidation, not speculative autonomy.

The core product bet is:

> Store broad local evidence deterministically, distill it into trusted episode-level artifacts, retrieve candidates semantically, and use graph structure plus provenance to refine and expand recall.

---

## Goals

- Local-only, offline-first operation
- Single-user, per-repo default scope
- Deterministic ingestion and retrieval behavior
- Very low-latency automatic recall
- Strong evidence and provenance for surfaced memory
- Small automatic memory payloads, deeper recall via MCP
- Broad history retention with distilled higher-value artifacts
- Graph as the semantic model, not the only retrieval primitive

## Non-goals

- Cloud sync or hosted backends
- Multi-user memory sharing
- Freeform autonomous reflection or speculative reminders
- Large prompt injections by default
- Hidden model downloads during hook execution
- LLM-generated freeform database queries

## Superseding architecture decisions

These decisions intentionally supersede earlier questionnaire wording where the two conflict.

- **Raw events in `redb` are the canonical truth layer.** Episodes remain first-class durable artifacts, but they are derived from the raw event stream rather than replacing it.
- **Typed extraction output supersedes model-generated freeform Grafeo queries.** Models may propose facts, but Lobster alone compiles those facts into fixed graph updates.
- **Explicit local model install supersedes hidden first-use downloads.** Hook execution must not surprise users with network access or long setup work.

---

## Design principles

### 1. Deterministic truth, derived serving layers

The canonical state must be replayable from durable local records. Search indexes and graph projections can be rebuilt.

### 2. Persist derived artifacts, not just source text

Summaries, accepted extraction outputs, embedding artifacts, and projection metadata are themselves durable records. Rebuilds should prefer persisted artifacts over re-running models whenever possible.

### 3. Evidence before abstraction

Every promoted decision, entity relation, and graph edge should trace back to one or more concrete source episodes or spans.

### 4. Distill before recall

Automatic recall should search over distilled artifacts first: decisions, summaries, active tasks, and durable constraints.

### 5. Embeddings retrieve, graphs explain

Candidate generation is embedding-led; graph links, temporal context, and heuristics rerank and expand.

### 6. Small by default

Automatic recall should be tiny and high confidence. Uncertain or bulky history stays behind explicit MCP tools.

### 7. Dreaming means maintenance

Background work should summarize, retry, merge, enrich, and compress. It should not invent TODOs or speculative insights.

---

## System overview

```text
Claude Code hooks
    |
    v
raw event capture ------------------------------+
    |                                           |
    v                                           |
redb canonical store                            |
    |                                           |
    +--> episode builder                        |
    |       |
    |       v
    |   finalized episode
    |       |
    |       +--> summary + decision detection
    |       |
    |       +--> embedding jobs (pylate-rs)
    |       |
    |       +--> graph extraction jobs (Extractor)
    |               |
    |               v
    |          typed extraction output
    |               |
    |               v
    |          deterministic compiler
    |               |
    |               v
    +-----------> Grafeo semantic serving layer
                    |
                    +--> hybrid retrieval
                    +--> graph navigation
                    +--> graph-backed reranking

Hooks pre-response path --> tiny automatic recall
MCP tools                --> deep explicit recall
Dreaming / idle worker   --> retries, merges, consolidation
```

---

## Runtime shape

Lobster runs as a **single local binary** with internal components:

- hook ingestion handlers
- MCP server handlers
- redb storage layer
- episode segmentation logic
- summarization worker
- embedding worker
- extraction worker
- dreaming / retry worker
- Grafeo projection and query layer

There is no required external daemon, but the binary has two explicit runtime modes:

- `lobster hook ...` for one-shot hook execution
- `lobster mcp` for long-lived MCP service execution

### Process ownership

Background maintenance must not be assumed to run inside short-lived hook invocations.

- Same-turn recall belongs to hook execution.
- Deep explicit recall belongs to the long-lived MCP process.
- Maintenance, retries, and dreaming belong to the long-lived process when available, or run opportunistically under a repo-local lease with a strict time budget.

This keeps lifecycle ownership explicit without requiring a separate daemon architecture.

---

## Storage architecture

## 1. Canonical store: `redb`

`redb` is the source of truth for deterministic data.

It stores:

- raw hook events
- episode records
- summary artifacts
- task records
- decision records
- accepted extraction artifacts
- embedding artifacts
- entity canonicalization metadata
- provenance/evidence refs
- processing jobs and retry state
- summarizer and extractor version metadata
- embedding model/backend/quantization metadata
- projection version and applied-at metadata
- retrieval statistics and surfacing telemetry
- repo config, ignore rules, and operational status

### Why `redb`

- embedded, local, Rust-native
- ACID and crash-safe
- deterministic replay-friendly persistence
- suitable for append-heavy event capture and typed records

### Write-coordinator policy

`redb` is concurrent-reader / single-writer. Only one `WriteTransaction` can be active at a time (`begin_write` blocks until the previous one commits or aborts). Since Lobster wants raw-event appends, episode finalization, artifact persistence, retry bookkeeping, and status updates all in one binary, writes must be carefully coordinated:

1. **Keep write transactions very short.** Never hold a `redb` write transaction across model inference, Grafeo projection, or network I/O.
2. **Funnel nontrivial writes through a dedicated writer task** in the long-lived MCP process. Other components (hook handlers, embedding workers, extraction workers) send write requests to this task via a channel.
3. **Batch related writes into a single transaction** where possible (e.g., episode finalization: persist episode shell + summary artifact + decision records in one transaction, then release the lock before starting Grafeo projection).
4. **Short-lived hook invocations** should only append raw events. They must not attempt long write transactions that could block other hooks.

If the writer task is not running (e.g., no long-lived MCP process), short-lived hook invocations acquire the write lock briefly for event appends only, under the same repo-local lease and wall-clock budget defined for background maintenance.

### Durability policy

- **`Durability::Immediate`** for: raw events, episode records, accepted artifacts, visibility-state flips. These must survive crashes.
- **`Durability::None`** only for: clearly rebuildable batches like telemetry bursts, projection metadata updates, and statistics refreshes. A subsequent `Immediate` commit flushes these.

### Canonical rule

If Grafeo or any retrieval index is lost, Lobster must be able to rebuild semantic state from `redb`.

That rebuild should not depend on re-running summarization or extraction by default. The canonical artifact layer in `redb` should contain enough accepted summary, extraction, and embedding data to replay projection deterministically and audit what happened.

### Durable artifact layer

At minimum, persist these versioned artifacts in `redb`:

- summary text + summarizer version/revision
- accepted `ExtractionOutput` + extractor version/revision
- embedding artifact metadata: model revision, backend, quantization, checksum
- canonical serialized embedding representation or vector bytes
- projection state: projection version, applied-at timestamp, projection checksum/state

This makes Grafeo a rebuildable serving projection rather than the only keeper of semantic state.

---

## 2. Semantic serving layer: `Grafeo`

`Grafeo` is not just the graph layer. It acts as a **materialized semantic index** for:

- nodes: episodes, tasks, decisions, entities
- evidence-backed edges with temporal validity
- searchable distilled artifacts (via explicit text and vector indexes)
- hybrid retrieval: HNSW vector search, BM25, filtered search, MMR diversity
- graph neighborhood traversal
- graph-backed context assembly and reranking support

### Why `Grafeo`

It already matches the serving needs better than forcing `redb` to become a graph/vector/text retrieval engine. It provides HNSW, BM25, hybrid search, MMR, sessions, transactions, and an embeddable Rust API.

### Grafeo as materialized index, not second truth

Grafeo is a **rebuildable projection** of the canonical state in `redb`. It is not a second source of truth, and it is not a string-query target for generated writes.

What belongs in Grafeo:

- promoted semantic facts
- searchable summaries and decisions
- graph relationships with evidence back-links and temporal validity metadata
- graph-time adjacency and semantic neighborhood data
- explicit text indexes on summary/decision text fields
- explicit vector indexes on pooled proxy vectors

What does not belong in Grafeo (stays in `redb`):

- raw events
- job queues
- retry state
- canonical ingestion state
- accepted summary/extraction/embedding artifacts
- audit completeness guarantees

### Temporal edge metadata

Edges in Grafeo carry temporal validity, not just decisions. Every projected edge should include:

- `valid_from_ts_utc_ms`: when the relationship became true
- `valid_to_ts_utc_ms`: when the relationship became invalid (null if still valid)

This preserves changing relationships over time (e.g., "component X depends on Y" may become invalid after a refactor). Graph traversal and `memory_neighbors` must filter on temporal validity by default.

### Index requirements

Lobster must create explicit indexes in Grafeo for its retrieval paths:

- **Vector index**: HNSW on pooled proxy vectors for decision, summary, and task artifact nodes. Cosine metric, dimensions matching the PyLate proxy vector.
- **Text index**: BM25 on summary text and decision statement fields.
- **Property index**: on `artifact_type`, `repo_id`, `task_id` for fast filtering.

### Embedding integration rule

Lobster should not rely on Grafeo's built-in embedding generation path. Embeddings are produced by Lobster's own runtime and projected into Grafeo explicitly, which avoids hidden model downloads and keeps model provenance under Lobster's control.

---

## 3. Embedding runtime: `pylate-rs`

`pylate-rs` is the local embedding inference component.

It should embed mainly:

- decisions
- decision + rationale + compact task context
- episode summaries
- active task summaries
- durable constraints/components when promoted

### Important constraint

`pylate-rs` is an inference runtime, not a full retrieval architecture by itself.

This is acceptable because Lobster retrieves over distilled artifacts, not raw full-history spans. A dedicated late-interaction code index can be added later as an optional subsystem.

### Model ownership rule

`ColBERT::encode()` takes `&mut self`, so a single global model instance cannot serve concurrent ingestion-time embedding and retrieval-time reranking. Lobster must define explicit model ownership:

- one model instance per embedding/rerank worker, or a bounded pool keyed by backend/device
- the long-lived MCP process owns the pool; short-lived hook invocations do not load models
- CPU is the canonical fixture/determinism baseline; GPU backends are opt-in performance modes

### Proxy-vector reduction rule

The architecture requires a "pooled single-vector proxy" for each distilled artifact, but the reduction from PyLate per-token embeddings to a single vector must be explicitly defined and frozen for determinism:

- reduction: mean-pool all non-padding token embeddings from the PyLate encoder output into a single vector
- the reduction function is versioned alongside the embedding model revision
- the reduced vector dimensions must match what Grafeo's HNSW index expects
- fixture tests must verify that the same input produces the same proxy vector per Lobster release

### Artifact-specific pooling policy

Not all artifact classes benefit equally from full late-interaction reranking. Pooling is a storage/quality trade-off tuned per class:

| Artifact class             | `late_interaction_bytes` | Pooling | Rationale                                        |
| -------------------------- | ------------------------ | ------- | ------------------------------------------------ |
| Decisions                  | full (no pooling)        | none    | High-value, short text, most critical for recall |
| Active task summaries      | light (pool_factor=2)    | light   | Medium-value, moderate length                    |
| Durable constraints        | full (no pooling)        | none    | High-value, rarely change                        |
| Episode summaries (recent) | light (pool_factor=2)    | light   | Moderate value, moderate length                  |
| Episode summaries (old)    | none (proxy only)        | heavy   | Bulk, lower recall priority                      |

Artifact classes without `late_interaction_bytes` use pooled-vector reranking only and must be labeled clearly in code and tests.

### Explicit retrieval contract

Because PyLate-style late interaction and Grafeo's vector search are not identical retrieval models, Lobster must define a bridging contract explicitly:

1. persist a pooled single-vector proxy for each distilled artifact
2. persist `late_interaction_bytes` per the artifact-specific pooling policy above
3. project the pooled proxy vector into Grafeo
4. use Grafeo hybrid search to fetch top-K candidates
5. rerank those candidates in-process with exact PyLate similarity when `late_interaction_bytes` are available, otherwise pooled-vector reranking
6. apply graph support, task overlap, recency heuristics, and MMR diversity for final ordering
7. reject candidates below the confidence threshold (see retrieval routing spec)

This gives Lobster a practical path without requiring a second dedicated multi-vector index on day one.

---

## Data model

Lobster uses a strong typed schema.

## Core durable records

```rust
struct RawEvent {
    seq: u64,
    repo_id: RepoId,
    ts_utc_ms: i64,
    event_kind: EventKind,
    payload_hash: [u8; 32],
    payload_bytes: Vec<u8>,
}

struct Episode {
    episode_id: EpisodeId,
    repo_id: RepoId,
    start_seq: u64,
    end_seq: u64,
    task_id: Option<TaskId>,
    processing_state: ProcessingState,
    finalized_ts_utc_ms: i64,
}

struct Decision {
    decision_id: DecisionId,
    repo_id: RepoId,
    episode_id: EpisodeId,
    task_id: Option<TaskId>,
    statement: String,
    rationale: String,
    confidence: Confidence,
    valid_from_ts_utc_ms: i64,
    valid_to_ts_utc_ms: Option<i64>,
    evidence: Vec<EvidenceRef>,
}

struct Task {
    task_id: TaskId,
    repo_id: RepoId,
    title: String,
    status: TaskStatus,
    opened_in: EpisodeId,
    last_seen_in: EpisodeId,
}

struct Entity {
    entity_id: EntityId,
    repo_id: RepoId,
    kind: EntityKind,
    canonical_name: String,
}

struct SummaryArtifact {
    episode_id: EpisodeId,
    revision: String,
    summary_text: String,
    payload_checksum: [u8; 32],
}

struct ExtractionArtifact {
    episode_id: EpisodeId,
    revision: String,
    output_json: Vec<u8>,
    payload_checksum: [u8; 32],
}

struct EmbeddingArtifact {
    artifact_id: ArtifactId,
    revision: String,
    backend: EmbeddingBackend,
    quantization: Option<String>,
    pooled_vector_bytes: Vec<u8>,
    late_interaction_bytes: Option<Vec<u8>>,
    payload_checksum: [u8; 32],
}
```

## Processing state

```rust
enum ProcessingState {
    Pending,
    Ready,
    RetryQueued,
    FailedFinal,
}
```

### Notes

- **Raw events are durable truth**.
- **Episodes are durable derived artifacts**, not the only truth layer.
- **Summaries, extraction outputs, and embeddings are also durable artifacts**.
- **The accepted summary lives in `SummaryArtifact`, not on `Episode` itself**.
- **Decisions must include evidence**.
- **Temporal validity matters** for decision timelines and changing constraints.
- **Tasks and decisions use deterministic canonicalization**.
- **General entities may merge later during dreaming** if evidence supports it.

---

## Identity and canonicalization

### Strict canonicalization

Deterministic canonicalization should be strong for:

- repositories
- tasks
- decisions
- file references when exact path identity exists

### Conservative canonicalization

General semantic entities may be inserted conservatively and merged later if the merge is evidence-backed and deterministic.

This preserves trust while avoiding brittle early over-merging.

---

## Ingestion pipeline

## Event capture

Every relevant hook/tool/conversation event is appended to `redb` immediately.

High-value event classes:

- user/assistant turns
- tool execution and outcomes
- file reads/writes/edits
- tests and failures
- plans and task transitions

### Redaction and ignore rules

Even in a local-only system, Lobster should not persist obvious secrets or useless large blobs by default.

Lobster should include a deterministic filter layer for:

- ignored paths and file patterns
- obvious secret/token-like strings
- `.env`-style sensitive content
- large binary payloads

Filtering decisions should be deterministic, logged, and repo-configurable.

## Episode segmentation

Episodes are built from raw events using deterministic rules:

- idle gaps
- repo transitions
- task-intent changes
- significant hook boundaries

Tie-breaks must be stable and testable.

## Finalization flow

When an episode closes:

1. persist finalized episode shell in `redb` as `Pending`
2. produce a versioned `SummaryArtifact`
3. persist the accepted summary artifact in `redb`
4. detect/promote decisions using heuristics + confidence buckets
5. persist decisions/tasks/artifacts in `redb`
6. enqueue embedding and graph extraction jobs
7. persist accepted `EmbeddingArtifact` and `ExtractionArtifact` outputs in `redb`
8. project the ready semantic view into Grafeo
9. mark episode `Ready` only after projection succeeds
10. if extraction fails twice, mark `RetryQueued` and keep it out of normal recall

### Why this flow

It preserves:

- durable truth first
- near-real-time serving
- explicit pending states
- deterministic recovery behavior

### Latency target

Target sub-second total processing for common finalized episodes, but never at the expense of corrupt or opaque state transitions.

## Summarization contract

Summarization should use the same architectural discipline as extraction.

Use a swappable async interface:

```rust
trait Summarizer: Send + Sync {
    async fn summarize(&self, input: SummaryInput) -> Result<SummaryArtifact, SummaryError>;
}
```

### Why async

The primary summarizer implementation wraps `rig-core`, which is Tokio-based. Synchronous trait signatures would force `block_on` calls into hook handling and MCP request paths. Making the trait async from the start avoids that.

`rig-core` is used **only** as an adapter for LLM calls (summarization and extraction). It does not own retrieval, vector stores, or the MCP surface. Those remain with Lobster's own Grafeo, PyLate, and `memory_*` contracts.

Persist the accepted `SummaryArtifact` in `redb` with:

- summary text
- summarizer version/revision
- model/runtime identity when applicable
- deterministic checksum of the output payload

The summary is not just a transient pre-processing step. It is a first-class durable artifact used for rebuilds, auditability, and retrieval.

---

## Decision detection

Decision detection is **heuristics-first**.

Signals may include:

- explicit choice language
- plan approval
- implementation commitment
- change/fix confirmation
- test outcome tied to a selected path
- stated constraints or non-goals

### Canonical ownership rule

Canonical `Decision` records are created only by the decision-detection pipeline.

The extractor may reference existing decisions and emit graph relations around them, but it does not create new canonical `Decision` records. If Lobster later experiments with model-suggested decisions, they should be stored as a separate non-canonical proposal artifact until explicitly promoted.

### Promotion policy

- high-confidence decisions may be auto-promoted
- confidence is stored as low/medium/high
- every promoted decision must include evidence refs
- decisions without usable evidence are rejected or kept provisional

---

## Graph extraction architecture

## Extractor contract

Lobster should not hard-code a specific model backend into the architecture.

Use a swappable async interface:

```rust
trait Extractor: Send + Sync {
    async fn extract(&self, input: ExtractionInput) -> Result<ExtractionOutput, ExtractionError>;
}
```

Possible implementations:

- deterministic heuristic extractor
- `rig-core`-backed LLM extractor (using Rig's `Extractor` API for structured output)
- Candle-backed local model extractor
- future Qwen-compatible extractor

### Rig-core scope constraint

`rig-core` is an adapter for the model call only. It provides the prompt-to-structured-output path. Lobster does not use Rig's agents, dynamic context, vector stores, tools, or MCP features. Those capabilities overlap with Lobster's own stack and would dilute the redb/Grafeo/PyLate architecture.

## Required output shape

The extractor must emit **typed structured facts**, not freeform Grafeo queries.

Extractor output may reference already-created canonical decisions, but it does not create new ones.

Example:

```json
{
  "task_refs": ["task:build-memory-search"],
  "decision_refs": ["decision:9d2"],
  "entities": [
    { "kind": "component", "name": "Grafeo" },
    { "kind": "constraint", "name": "offline-first" }
  ],
  "relations": [
    { "type": "task_decision", "from": "task:build-memory-search", "to": "decision:9d2" },
    { "type": "decision_entity", "from": "decision:9d2", "to": "entity:component:grafeo" }
  ]
}
```

## Deterministic compiler

Lobster compiles extractor output into typed graph mutations executed through **Grafeo's programmatic CRUD API** (`create_node`, `set_node_property`, `create_edge`, etc.), not through GQL query strings.

### Why programmatic, not GQL

Grafeo exposes a complete direct API for node/edge/property creation, sessions, and transactions. Since the extractor already emits typed facts, the cleanest deterministic compiler converts those facts into typed graph operations. This removes a whole class of string-assembly errors, makes the projection layer easier to fixture-test, and keeps the serialization format under Lobster's control.

GQL is reserved for debugging, admin inspection, and read-side queries (e.g., `memory_neighbors` traversal). It is not used for projection writes.

### Validation

Validation must include:

- schema validation
- evidence validation
- duplicate checks
- canonical ID resolution where applicable

Invalid output never writes directly to Grafeo.

## Model handling

- no hidden auto-download during hook execution
- model install should be explicit setup/bootstrap
- quantized local defaults are acceptable
- extractor behavior should be versioned and fixture-tested

---

## Graph schema

## First-class nodes

- episode
- task
- decision
- entity

### Initial entity kinds

- concept
- constraint
- component
- file-lite
- repo

## Required edge families

### Provenance / timeline edges

- episode -> task
- episode -> decision
- episode -> entity

### Semantic work edges

- task -> decision
- task -> entity
- decision -> entity
- entity -> entity

### Edge rule

Every persisted edge must be evidence-backed and temporally annotated.

- `valid_from_ts_utc_ms`: when the relationship became true
- `valid_to_ts_utc_ms`: when the relationship became invalid (null if still valid)
- `evidence`: one or more `EvidenceRef` back-links

This keeps graph navigation explainable, supports `memory_neighbors` safely, and prevents stale relationships from corrupting retrieval.

---

## Retrieval architecture

Lobster has **query-routed** retrieval, not one-size-fits-all. The retrieval path is selected by a deterministic route classifier before any search runs. See `docs/RETRIEVAL_ROUTING.md` for the full routing spec, thresholds, candidate budgets, and eval matrix.

### Why query routing

GraphRAG underperforms plain RAG on simple tasks. Always invoking graph expansion or always trusting a dense retriever wastes budget and reduces precision. A deterministic router selects the cheapest path that satisfies the query class.

## 1. Automatic recall path

Purpose: tiny, high-confidence, low-latency hints.

### Search corpus

Automatic recall should search only over distilled ready artifacts:

- decisions
- active task summaries
- recent high-value episode summaries
- durable constraints/components

### Candidate generation contract

Candidate retrieval uses the route selected by the classifier:

1. **classify the query** into a retrieval route (exact, hybrid, hybrid+graph, or abstain)
2. execute the route-specific search against Grafeo
3. rerank the candidate set in-process with exact PyLate similarity when `late_interaction_bytes` are available, otherwise pooled-vector reranking
4. apply graph/task/recency heuristics and MMR diversity control
5. **reject candidates below the confidence threshold** (hard "say nothing" rule)
6. intersect the result set with the `redb` ready set before anything is surfaced
7. **expand evidence windows** for surfaced decisions: include local rationale and supporting evidence, not detached snippets

This keeps the hot path fast while preserving a strict visibility gate and ensuring surfaced results carry enough context to be useful.

### Ranking model

A deterministic composite score:

```text
score = semantic + recency + task_overlap + graph_support + decision_bonus - noise_penalty
```

Stable tie-breakers:

1. artifact priority
2. timestamp
3. stable ID

### Rejection rule

If no candidate exceeds the route-specific confidence threshold after reranking, automatic recall returns **nothing**. Surfacing weak results is worse than silence. The threshold is defined per route in the retrieval routing spec.

### Diversity control

Use Grafeo's MMR (Maximal Marginal Relevance) support to avoid near-duplicate items in automatic recall. The diversity parameter is tunable per route.

### Evidence-window expansion

When a decision or summary is surfaced, automatic recall must expand the result to include its local evidence window:

- for decisions: the rationale, supporting evidence refs, and compact task context
- for summaries: the summary text plus the decision(s) it supports, if any

This prevents surfacing detached snippets that lack the context to be actionable.

### Output budget

- default: 1-3 short high-confidence items
- usually a tiny hint block
- larger structured payload only when confidence is high
- uncertain memory stays behind explicit tools

## 2. Explicit MCP retrieval path

Purpose: richer recall and graph exploration.

This path may search more broadly, use the full set of retrieval routes, and return larger structured payloads. It still respects the same visibility rule: only artifacts marked `Ready` in `redb` are eligible for normal retrieval results. It uses the same routing classifier but with wider candidate budgets and lower rejection thresholds.

---

## MCP tool contracts

## `memory_recent`

Return newest ready artifacts filtered by repo/task/type.

## `memory_search`

Return mixed ranked hits across decisions, summaries, tasks, and entities.

Each hit should include:

- snippet/text
- artifact type
- repo/task refs
- confidence
- provenance pointer
- lightweight graph context

## `memory_decisions`

Return a decision timeline for a repo/task/component, including rationale and supporting provenance.

## `memory_neighbors`

Return evidence-backed graph neighbors only.

## `memory_context`

Assemble a compact task-oriented context bundle from top-ranked mixed hits.

### MCP principle

MCP is the deep recall surface. Hooks should not attempt to expose full history inline.

---

## Hook integration

## Pre-response hooks

Use only for tiny same-turn recall hints.

Primary home: `UserPromptSubmit`-style entry points before response generation.

## Post-tool / milestone hooks

Use for lightweight reminders after:

- heavy edits
- failures
- test results
- major task transitions

Primary homes: `PostToolUse`, `PostToolUseFailure`, and task lifecycle events.

## Async/background hooks

Use for non-blocking maintenance only:

- summarization jobs
- graph retries
- merge passes
- statistics refresh

Async hooks are not the mechanism for same-turn context injection.

When no long-lived MCP process is active, async/background work may run opportunistically under a repo-local lease and a fixed wall-clock budget so that multiple short-lived hook invocations do not fight over maintenance ownership.

### MCP logging constraint

The MCP server uses stdio for JSON-RPC transport. All Lobster logging must go to **stderr or files only**. Writing to stdout corrupts JSON-RPC traffic and breaks the MCP protocol.

---

## Dreaming / background synthesis

Dreaming means maintenance and consolidation.

Allowed jobs:

- retry failed graph extraction
- entity merge proposals/execution
- summary pyramids over older episodes
- task timeline maintenance
- graph link backfill from accepted evidence
- surfacing stats recalculation

Not allowed by default:

- speculative reminders
- autonomous TODO generation
- freeform reflective essays

---

## Processing states and diagnostics

Memories that are not fully processed should not silently disappear.

## Cross-store visibility protocol

Because `redb` and Grafeo are separate stores, retrieval visibility is controlled by `redb`, not by whatever semantic data happens to exist in Grafeo.

The required protocol is:

1. persist episode and derived artifacts in `redb` as `Pending`
2. persist accepted summary/extraction/embedding artifacts in `redb`
3. apply Grafeo projection
4. record projection metadata in `redb`
5. flip the episode/artifacts to `Ready`
6. intersect all Grafeo retrieval candidates with the `redb` ready set before returning them

This prevents ghost graph state, half-projected artifacts, and recall skew after crashes or retries.

### Visibility scope

Readiness is **episode-scoped**. Derived summaries, decisions, entities, and graph relations inherit the visibility state of their parent episode for automatic and normal explicit recall.

### State transitions

- `Pending -> Ready` only after accepted artifacts are in `redb`, Grafeo projection succeeds, and projection metadata is recorded
- `Pending -> RetryQueued` on failed embedding, extraction, or projection when retry budget remains
- `RetryQueued -> Ready` on successful retry and projection
- `RetryQueued -> FailedFinal` when retry budget is exhausted
- `FailedFinal` items remain out of normal recall until repaired by an explicit maintenance path

### States

- `Pending`: persisted but not fully processed
- `Ready`: eligible for normal recall
- `RetryQueued`: waiting for retry
- `FailedFinal`: failed after retry budget exhausted

### Diagnostics requirement

Lobster should expose internal visibility for degraded state, for example:

- warning on failure/open-fail mode
- status summary in logs or CLI
- optional pending-aware admin/debug query paths
- `lobster status`
- `lobster reset --repo`
- optional `memory_status` debug/admin MCP surface

This avoids opaque behavior when graph extraction lags or fails.

---

## Performance targets

### Automatic recall

- extremely low overhead
- no heavy graph traversal on the hot path
- recall should usually operate on distilled artifacts only

### Episode processing

- common case target under 1 second end-to-end
- graceful pending/retry path if full processing misses budget

### Background work

- must yield to interactive work
- should prefer idle moments and session wrap-up windows

---

## Trust and determinism requirements

Lobster must prove two things in tests:

1. same inputs produce the same durable state
2. same memory/query context produces the same ranking outputs

### Determinism boundary

Determinism is promised per:

- Lobster release
- summarizer/extractor/embedding model revision
- backend/runtime choice

CPU-backed execution should be the canonical fixture/golden-test mode. Faster GPU or platform-specific backends may exist, but they should be treated as performance modes rather than the canonical determinism baseline.

## Required test layers

- fixture-driven event ingestion tests
- episode segmentation tests
- decision detection tests
- extraction compiler tests
- Grafeo projection tests
- retrieval ranking determinism tests
- end-to-end decision recall scenarios

### Versioned contracts

The following should be versioned:

- summary artifact schema
- summarizer behavior contract
- extraction schema
- prompt/extractor behavior contract
- embedding artifact schema
- graph template set
- ranking feature set where practical

Behavioral tests are necessary, but they should not be the only compatibility mechanism.

### Telemetry rule

Retrieval and surfacing telemetry is observational only unless a telemetry-derived signal is explicitly promoted into the versioned ranking feature set. This keeps fixture-based determinism intact.

---

## Failure behavior

If Lobster is degraded:

- Claude Code continues normally
- Lobster fails open with warning
- new raw events still persist if possible
- retrieval may omit non-ready items
- retries run later when possible

The memory system must never block normal coding flow.

---

## Suggested crate layout

```text
src/
  main.rs
  app/
    mod.rs
    config.rs
    status.rs
  hooks/
    mod.rs
    capture.rs
    surfacing.rs
  mcp/
    mod.rs
    search.rs
    recent.rs
    decisions.rs
    neighbors.rs
    context.rs
  store/
    mod.rs
    redb.rs
    schema.rs
    ids.rs
  episodes/
    mod.rs
    segmenter.rs
    summary.rs
    summarizer.rs
    decisions.rs
  embeddings/
    mod.rs
    pylate.rs
  extract/
    mod.rs
    traits.rs
    heuristic.rs
    candle.rs
    compiler.rs
    validate.rs
  graph/
    mod.rs
    grafeo.rs
    projection.rs
    query.rs
  dream/
    mod.rs
    retry.rs
    merge.rs
    compress.rs
  rank/
    mod.rs
    scoring.rs
  tests/
    fixtures.rs
```

This keeps truth, serving, extraction, and retrieval concerns separate.

---

## Future extensions

- optional dedicated late-interaction code index
- cross-repo opt-in linking
- richer export/inspection tools
- stronger symbol-aware memory links
- better merge heuristics for entities
- richer temporal graph reasoning

These are intentionally deferred.

---

## Final architecture statement

Lobster is a **deterministic layered memory system** for Claude Code:

- `redb` stores durable truth, coordinated through a single-writer task
- `Grafeo` serves semantic memory, projected via its programmatic CRUD API
- `pylate-rs` powers local semantic retrieval over distilled artifacts, with pooled model ownership and a frozen proxy-vector reduction rule
- `rig-core` adapts LLM calls for summarization and extraction only, behind async trait interfaces
- extractors emit typed facts, not raw queries
- hooks surface tiny high-confidence recall
- MCP tools expose deep memory exploration
- dreaming consolidates and repairs memory in the background

That is the architecture most aligned with Lobster's product goals and with the current best pattern for local agent memory systems.
