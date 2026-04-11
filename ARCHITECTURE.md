# Lobster v1 architecture spec

## Summary

Lobster is a local, deterministic, per-repo memory system for Claude Code.

Its v1 architecture is **layered memory**:

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

## Non-goals for v1

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

Background work in v1 should summarize, retry, merge, enrich, and compress. It should not invent TODOs or speculative insights.

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

Lobster v1 runs as a **single local binary** with internal components:

- hook ingestion handlers
- MCP server handlers
- redb storage layer
- episode segmentation logic
- summarization worker
- embedding worker
- extraction worker
- dreaming / retry worker
- Grafeo projection and query layer

There is no required external daemon in v1, but the binary has two explicit runtime modes:

- `lobster hook ...` for one-shot hook execution
- `lobster mcp` for long-lived MCP service execution

### Process ownership

Background maintenance must not be assumed to run inside short-lived hook invocations.

- Same-turn recall belongs to hook execution.
- Deep explicit recall belongs to the long-lived MCP process.
- Maintenance, retries, and dreaming belong to the long-lived process when available, or run opportunistically under a repo-local lease with a strict time budget.

This keeps lifecycle ownership explicit without requiring a separate daemon architecture in v1.

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

`Grafeo` is not just the graph layer. In v1 it acts as the **semantic serving layer** for:

- nodes: episodes, tasks, decisions, entities
- evidence-backed edges
- searchable distilled artifacts
- hybrid retrieval support
- graph neighborhood traversal
- graph-backed context assembly

### Why `Grafeo`

It already matches the serving needs better than forcing `redb` to become a graph/vector/text retrieval engine.

### What belongs in Grafeo

- promoted semantic facts
- searchable summaries and decisions
- graph relationships with evidence back-links
- graph-time adjacency and semantic neighborhood data

### What does not make Grafeo the source of truth

- raw events
- job queues
- retry state
- canonical ingestion state
- accepted summary/extraction/embedding artifacts
- audit completeness guarantees

Those stay in `redb`.

### Embedding integration rule

Lobster v1 should not rely on Grafeo's built-in embedding generation path. Embeddings are produced by Lobster's own runtime and projected into Grafeo explicitly, which avoids hidden model downloads and keeps model provenance under Lobster's control.

---

## 3. Embedding runtime: `pylate-rs`

`pylate-rs` is the local embedding inference component.

In v1 it should embed mainly:

- decisions
- decision + rationale + compact task context
- episode summaries
- active task summaries
- durable constraints/components when promoted

### Important constraint

`pylate-rs` is an inference runtime, not a full retrieval architecture by itself.

For v1, that is acceptable because Lobster retrieves over distilled artifacts, not raw full-history spans. A dedicated late-interaction code index can be added later as an optional subsystem.

### Explicit v1 retrieval contract

Because PyLate-style late interaction and Grafeo's vector search are not identical retrieval models, Lobster must define a bridging contract explicitly:

1. persist a pooled single-vector proxy for each distilled artifact
2. persist `late_interaction_bytes` for artifact classes that participate in exact PyLate reranking
3. project the pooled proxy vector into Grafeo
4. use Grafeo hybrid search to fetch top-K candidates
5. rerank those candidates in-process with exact PyLate similarity derived from the persisted late-interaction representation
6. apply graph support, task overlap, and recency heuristics for final ordering

If an artifact class does not persist late-interaction state in v1, Lobster must treat it as pooled-vector reranking only and label that path clearly in code and tests.

This gives Lobster a practical v1 path without requiring a second dedicated multi-vector index on day one.

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

### Strict in v1

Deterministic canonicalization should be strong for:

- repositories
- tasks
- decisions
- file references when exact path identity exists

### Conservative in v1

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

v1 should include a deterministic filter layer for:

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

Use a swappable interface:

```rust
trait Summarizer {
    fn summarize(&self, input: SummaryInput) -> Result<SummaryArtifact, SummaryError>;
}
```

Persist the accepted `SummaryArtifact` in `redb` with:

- summary text
- summarizer version/revision
- model/runtime identity when applicable
- deterministic checksum of the output payload

The summary is not just a transient pre-processing step. It is a first-class durable artifact used for rebuilds, auditability, and retrieval.

---

## Decision detection

Decision detection is **heuristics-first** in v1.

Signals may include:

- explicit choice language
- plan approval
- implementation commitment
- change/fix confirmation
- test outcome tied to a selected path
- stated constraints or non-goals

### Canonical ownership rule

In v1, canonical `Decision` records are created only by the decision-detection pipeline.

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

Use a swappable interface:

```rust
trait Extractor {
    fn extract(&self, input: ExtractionInput) -> Result<ExtractionOutput, ExtractionError>;
}
```

Possible implementations:

- deterministic heuristic extractor
- Candle-backed local model extractor
- future Qwen-compatible extractor

## Required output shape

The extractor must emit **typed structured facts**, not freeform Grafeo queries.

In v1, extractor output may reference already-created canonical decisions, but it does not create new ones.

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

Lobster compiles extractor output into a **tiny fixed insert/update template set** for Grafeo.

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

Every persisted edge must be evidence-backed.

This keeps graph navigation explainable and supports `memory_neighbors` safely.

---

## Retrieval architecture

Lobster has two retrieval paths.

## 1. Automatic recall path

Purpose: tiny, high-confidence, low-latency hints.

### Search corpus

Automatic recall should search only over distilled ready artifacts:

- decisions
- active task summaries
- recent high-value episode summaries
- durable constraints/components

### Candidate generation contract

For v1, candidate retrieval should work like this:

1. query Grafeo hybrid search over pooled single-vector proxies + BM25 text
2. fetch a small top-K candidate set
3. rerank that set in-process with exact PyLate similarity when `late_interaction_bytes` are available, otherwise same-model pooled-vector reranking
4. apply graph/task/recency heuristics
5. intersect the result set with the `redb` ready set before anything is surfaced

This keeps the hot path fast while preserving a strict visibility gate.

### Ranking model

A deterministic composite score:

```text
score = semantic + recency + task_overlap + graph_support + decision_bonus - noise_penalty
```

Stable tie-breakers:

1. artifact priority
2. timestamp
3. stable ID

### Output budget

- default: 1-3 short high-confidence items
- usually a tiny hint block
- larger structured payload only when confidence is high
- uncertain memory stays behind explicit tools

## 2. Explicit MCP retrieval path

Purpose: richer recall and graph exploration.

This path may search more broadly and return larger structured payloads, but it still respects the same visibility rule: only artifacts marked `Ready` in `redb` are eligible for normal retrieval results.

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

---

## Dreaming / background synthesis

In v1, dreaming means maintenance and consolidation.

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

In v1, readiness is **episode-scoped**. Derived summaries, decisions, entities, and graph relations inherit the visibility state of their parent episode for automatic and normal explicit recall.

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

CPU-backed execution should be the canonical fixture/golden-test mode in v1. Faster GPU or platform-specific backends may exist, but they should be treated as performance modes rather than the canonical determinism baseline.

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

Retrieval and surfacing telemetry is observational only in v1 unless a telemetry-derived signal is explicitly promoted into the versioned ranking feature set. This keeps fixture-based determinism intact.

---

## Failure behavior

If Lobster is degraded:

- Claude Code continues normally
- Lobster fails open with warning
- new raw events still persist if possible
- retrieval may omit non-ready items
- retries run later when possible

The memory system must never block normal coding flow in v1.

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

## Future extensions after v1

- optional dedicated late-interaction code index
- cross-repo opt-in linking
- richer export/inspection tools
- stronger symbol-aware memory links
- better merge heuristics for entities
- richer temporal graph reasoning

These are intentionally deferred.

---

## Final architecture statement

Lobster v1 is a **deterministic layered memory system** for Claude Code:

- `redb` stores durable truth
- `Grafeo` serves semantic memory
- `pylate-rs` powers local semantic retrieval over distilled artifacts
- extractors emit typed facts, not raw queries
- hooks surface tiny high-confidence recall
- MCP tools expose deep memory exploration
- dreaming consolidates and repairs memory in the background

That is the architecture most aligned with Lobster's product goals and with the current best pattern for local agent memory systems.
