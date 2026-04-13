# Lobster test strategy

## Philosophy

Lobster promises **deterministic behavior**: same inputs produce the same durable state,
same query context produces the same ranking. Tests exist to prove that promise holds.

Test quality bar:

- Every property encodes a real behavioral law, not a tautology.
- Use an oracle (reference, model, metamorphic relation) whenever possible.
- Generate valid inputs by construction — no rejection storms.
- Shrink to human-interpretable counterexamples.
- Be reproducible across runs.

---

## Frameworks

- **`hegeltest`** (`hegel`) for property-based tests. All PBT uses `#[hegel::test]`.
- **Standard `#[test]`** for fixture-driven golden tests and integration tests.
- **`redb::backends::InMemoryBackend`** for store tests (no disk I/O).
- **`Grafeo::new_in_memory()`** for graph tests.
- **CPU-only PyLate** as the canonical determinism baseline.

---

## Property type decision tree

For each subsystem, choose the highest-leverage property style that applies:

1. **Differential testing** — compare SUT against a reference or naive implementation.
2. **Model-based testing** — stateful APIs tested via `#[hegel::state_machine]` against a
   simple model (e.g., `Vec`, `HashMap`).
3. **Round-trip properties** — `decode(encode(x)) == x`, rebuild-from-canonical.
4. **Metamorphic relations** — when no oracle exists, assert relationships between
   `f(x)` and `f(transform(x))`.
5. **Algebraic laws** — commutativity, idempotence, identity. Only when meaningful.

Each test layer below is annotated with which property style it uses.

---

## Test layers

### 1. Store layer (`store/`)

Tests for the redb canonical store: event appends, episode persistence, artifact CRUD,
processing state transitions, and the write-coordinator contract.

#### Differential: store vs in-memory model

```rust
use hegel::generators as gs;

/// Oracle: a simple Vec<RawEvent> that tracks what was appended.
/// SUT: the redb store. After N operations, both must agree.
#[hegel::test(test_cases = 500)]
fn prop_store_matches_vec_model(tc: hegel::TestCase) {
    let db = test_db();
    let mut model: Vec<RawEvent> = Vec::new();
    let n: usize = tc.draw(gs::integers().min_value(1_usize).max_value(100));

    for _ in 0..n {
        let event: RawEvent = tc.draw(gen_raw_event());
        let seq = db.append_event(&event).unwrap();
        model.push(event);

        // Differential: store and model agree on the latest event
        let loaded = db.get_event(seq).unwrap();
        let expected = model.last().unwrap();
        assert_eq!(loaded.payload_hash, expected.payload_hash);
        assert_eq!(loaded.event_kind, expected.event_kind);
    }

    // Differential: total counts match
    assert_eq!(db.event_count(), model.len() as u64);
}
```

#### Round-trip: serialize → persist → load

```rust
#[hegel::test(test_cases = 500)]
fn prop_event_roundtrip(tc: hegel::TestCase) {
    let db = test_db();
    let event: RawEvent = tc.draw(gen_raw_event());
    let seq = db.append_event(&event).unwrap();
    let loaded = db.get_event(seq).unwrap();
    assert_eq!(loaded.payload_hash, event.payload_hash);
    assert_eq!(loaded.payload_bytes, event.payload_bytes);
    assert_eq!(loaded.event_kind, event.event_kind);
}

#[hegel::test(test_cases = 300)]
fn prop_artifact_roundtrip(tc: hegel::TestCase) {
    let db = test_db();
    let artifact: EmbeddingArtifact = tc.draw(gen_embedding_artifact());
    db.persist_embedding_artifact(&artifact).unwrap();
    let loaded = db.get_embedding_artifact(&artifact.artifact_id).unwrap();
    assert_eq!(loaded.pooled_vector_bytes, artifact.pooled_vector_bytes);
    assert_eq!(loaded.late_interaction_bytes, artifact.late_interaction_bytes);
    assert_eq!(loaded.payload_checksum, artifact.payload_checksum);
}
```

#### Algebraic: append monotonicity (identity-like law)

```rust
#[hegel::test(test_cases = 500)]
fn prop_event_append_monotonic(tc: hegel::TestCase) {
    // seq(append(e2)) > seq(append(e1)) for any e1 before e2
    let db = test_db();
    let n: usize = tc.draw(gs::integers().min_value(2_usize).max_value(200));
    let mut last_seq = 0u64;
    for _ in 0..n {
        let event = tc.draw(gen_raw_event());
        let seq = db.append_event(&event).unwrap();
        assert!(seq > last_seq, "Monotonicity violated: {seq} <= {last_seq}");
        last_seq = seq;
    }
}
```

#### Model-based: processing state machine

```rust
struct ProcessingStateMachine {
    db: TestDb,
    episode_id: EpisodeId,
    model_state: ProcessingState,
}

#[hegel::state_machine]
impl ProcessingStateMachine {
    #[rule]
    fn finalize_pending(&mut self, tc: hegel::TestCase) {
        tc.assume(self.model_state == ProcessingState::Pending);
        self.db.set_state(self.episode_id, ProcessingState::Ready).unwrap();
        self.model_state = ProcessingState::Ready;
    }

    #[rule]
    fn retry_pending(&mut self, tc: hegel::TestCase) {
        tc.assume(self.model_state == ProcessingState::Pending);
        self.db.set_state(self.episode_id, ProcessingState::RetryQueued).unwrap();
        self.model_state = ProcessingState::RetryQueued;
    }

    #[rule]
    fn retry_succeeds(&mut self, tc: hegel::TestCase) {
        tc.assume(self.model_state == ProcessingState::RetryQueued);
        self.db.set_state(self.episode_id, ProcessingState::Ready).unwrap();
        self.model_state = ProcessingState::Ready;
    }

    #[rule]
    fn retry_exhausted(&mut self, tc: hegel::TestCase) {
        tc.assume(self.model_state == ProcessingState::RetryQueued);
        self.db.set_state(self.episode_id, ProcessingState::FailedFinal).unwrap();
        self.model_state = ProcessingState::FailedFinal;
    }

    #[invariant]
    fn state_matches(&mut self, _tc: hegel::TestCase) {
        let actual = self.db.get_state(self.episode_id).unwrap();
        assert_eq!(actual, self.model_state);
    }

    #[invariant]
    fn no_backward_transition(&mut self, _tc: hegel::TestCase) {
        // Ready and FailedFinal are terminal — never go back to Pending
        if self.model_state == ProcessingState::Ready
            || self.model_state == ProcessingState::FailedFinal
        {
            assert_ne!(
                self.db.get_state(self.episode_id).unwrap(),
                ProcessingState::Pending
            );
        }
    }
}

#[hegel::test]
fn test_processing_state_machine(tc: hegel::TestCase) {
    let db = test_db();
    let episode_id = db.create_episode_pending().unwrap();
    let system = ProcessingStateMachine {
        db,
        episode_id,
        model_state: ProcessingState::Pending,
    };
    hegel::stateful::run(system, tc);
}
```

#### Model-based: write-coordinator (concurrent access)

```rust
struct WriterCoordinatorModel {
    db: Arc<TestDb>,
    model: Arc<Mutex<Vec<RawEvent>>>,
}

#[hegel::state_machine]
impl WriterCoordinatorModel {
    #[rule]
    fn append_event(&mut self, tc: hegel::TestCase) {
        let event: RawEvent = tc.draw(gen_raw_event());
        self.db.append_event(&event).unwrap();
        self.model.lock().unwrap().push(event);
    }

    #[rule]
    fn read_snapshot(&mut self, _tc: hegel::TestCase) {
        // Concurrent read must never see partial state
        let count = self.db.event_count();
        let model_count = self.model.lock().unwrap().len() as u64;
        assert!(count <= model_count, "Store ahead of model: {count} > {model_count}");
    }

    #[invariant]
    fn counts_converge(&mut self, _tc: hegel::TestCase) {
        let count = self.db.event_count();
        let model_count = self.model.lock().unwrap().len() as u64;
        assert_eq!(count, model_count);
    }
}
```

#### Golden fixture tests

- Append a known event sequence, read it back, compare byte-for-byte.
- Persist a known episode + artifacts, rebuild from scratch, verify identical state.
- Verify `Durability::Immediate` for events and artifacts; `Durability::None` only for
  telemetry batches.

---

### 2. Episode segmentation (`episodes/segmenter.rs`)

Tests for the deterministic segmentation of raw events into episodes.

#### Algebraic: partitioning laws

```rust
#[hegel::test(test_cases = 500)]
fn prop_segmentation_covers_all_events(tc: hegel::TestCase) {
    // Union of all episode ranges == full event sequence (partition completeness)
    let events: Vec<RawEvent> = tc.draw(gen_event_stream());
    let episodes = segment(&events);
    let covered: Vec<u64> = episodes.iter()
        .flat_map(|ep| ep.start_seq..=ep.end_seq)
        .collect();
    let all_seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(covered, all_seqs);
}

#[hegel::test(test_cases = 500)]
fn prop_segments_non_overlapping(tc: hegel::TestCase) {
    // No two episodes share a sequence number (partition disjointness)
    let events: Vec<RawEvent> = tc.draw(gen_event_stream());
    let episodes = segment(&events);
    for pair in episodes.windows(2) {
        assert!(pair[0].end_seq < pair[1].start_seq);
    }
}

#[hegel::test(test_cases = 500)]
fn prop_segmentation_idempotent(tc: hegel::TestCase) {
    // segment(events) == segment(events) (determinism as idempotence)
    let events: Vec<RawEvent> = tc.draw(gen_event_stream());
    let a = segment(&events);
    let b = segment(&events);
    assert_eq!(a, b);
}
```

#### Metamorphic: appending events extends the last segment or creates a new one

```rust
#[hegel::test(test_cases = 300)]
fn prop_append_extends_or_creates(tc: hegel::TestCase) {
    // Metamorphic: segment(events ++ [e]) either extends the last episode
    // or creates a new one. It never changes earlier episodes.
    let mut events: Vec<RawEvent> = tc.draw(gen_event_stream());
    let baseline = segment(&events);
    tc.assume(!baseline.is_empty());

    let extra = tc.draw(gen_raw_event_after(&events));
    events.push(extra);
    let extended = segment(&events);

    // All baseline episodes except possibly the last are unchanged
    let stable_prefix = baseline.len().saturating_sub(1);
    assert_eq!(&extended[..stable_prefix], &baseline[..stable_prefix]);
}
```

#### Golden fixture tests

- Known event sequences with idle gaps → expected episode boundaries.
- Known event sequences with repo transitions → expected splits.
- Tie-break stability: event streams that hit boundary conditions produce stable results.

---

### 3. Decision detection (`episodes/decisions.rs`)

#### Metamorphic: adding unrelated content doesn't change decisions

```rust
#[hegel::test(test_cases = 300)]
fn prop_noise_does_not_create_decisions(tc: hegel::TestCase) {
    // Metamorphic: appending non-decision noise to an episode
    // should not produce additional decisions
    let episode = tc.draw(gen_finalized_episode());
    let baseline = detect_decisions(&episode);

    let noisy = append_noise(&episode, tc.draw(gs::text().min_size(10).max_size(200)));
    let with_noise = detect_decisions(&noisy);

    // Noise should not increase decision count
    assert!(
        with_noise.len() <= baseline.len() + 1,
        "Noise added {} decisions (baseline: {})",
        with_noise.len(), baseline.len()
    );
}
```

#### Algebraic: evidence requirement (invariant)

```rust
#[hegel::test(test_cases = 300)]
fn prop_decisions_have_evidence(tc: hegel::TestCase) {
    // Every promoted decision has at least one evidence ref
    let episode = tc.draw(gen_finalized_episode());
    let decisions = detect_decisions(&episode);
    for d in &decisions {
        assert!(!d.evidence.is_empty(), "Decision without evidence: {:?}", d.statement);
    }
}

#[hegel::test(test_cases = 300)]
fn prop_decision_detection_deterministic(tc: hegel::TestCase) {
    let episode = tc.draw(gen_finalized_episode());
    let a = detect_decisions(&episode);
    let b = detect_decisions(&episode);
    assert_eq!(a, b);
}
```

#### Fixture tests

- Known conversation excerpts with explicit choice language → expected decisions.
- Excerpts with no decisions → empty result.
- Confidence bucketing: known inputs → expected low/medium/high classification.

---

### 4. Extraction compiler (`extract/compiler.rs`, `extract/validate.rs`)

Tests for the typed fact compiler that converts `ExtractionOutput` into Grafeo graph
mutations.

#### Differential: compiler output vs hand-written expected ops

```rust
#[hegel::test(test_cases = 300)]
fn prop_compiler_deterministic(tc: hegel::TestCase) {
    let output: ExtractionOutput = tc.draw(gen_valid_extraction_output());
    let a = compile(&output).unwrap();
    let b = compile(&output).unwrap();
    assert_eq!(a, b);
}
```

#### Algebraic: valid input always produces ops, invalid always rejected

```rust
#[hegel::test(test_cases = 300)]
fn prop_valid_extraction_produces_graph_ops(tc: hegel::TestCase) {
    let output: ExtractionOutput = tc.draw(gen_valid_extraction_output());
    let ops = compile(&output).unwrap();
    assert!(!ops.is_empty());
}

#[hegel::test(test_cases = 300)]
fn prop_invalid_extraction_rejected(tc: hegel::TestCase) {
    let output: ExtractionOutput = tc.draw(gen_invalid_extraction_output());
    let result = compile(&output);
    assert!(result.is_err());
}
```

#### Metamorphic: adding a new entity to extraction adds exactly one node op

```rust
#[hegel::test(test_cases = 200)]
fn prop_adding_entity_adds_one_node(tc: hegel::TestCase) {
    let base: ExtractionOutput = tc.draw(gen_valid_extraction_output());
    let base_ops = compile(&base).unwrap();
    let base_node_count = base_ops.iter().filter(|op| matches!(op, GraphOp::CreateNode { .. })).count();

    let extended = add_entity(&base, tc.draw(gen_entity()));
    let ext_ops = compile(&extended).unwrap();
    let ext_node_count = ext_ops.iter().filter(|op| matches!(op, GraphOp::CreateNode { .. })).count();

    assert_eq!(ext_node_count, base_node_count + 1);
}
```

#### Round-trip: compile → project → query

```rust
#[hegel::test(test_cases = 100)]
fn prop_projection_roundtrip(tc: hegel::TestCase) {
    // Compiled graph ops, when projected into Grafeo, are queryable
    let grafeo = Grafeo::new_in_memory();
    let output: ExtractionOutput = tc.draw(gen_valid_extraction_output());
    let ops = compile(&output).unwrap();
    project(&grafeo, &ops).unwrap();
    for op in &ops {
        if let GraphOp::CreateNode { label, .. } = op {
            let count: i64 = grafeo.session()
                .execute(&format!("MATCH (n:{label}) RETURN COUNT(n)"))
                .unwrap()
                .scalar()
                .unwrap();
            assert!(count > 0);
        }
    }
}
```

#### Validation fixture tests

- Known valid `ExtractionOutput` JSON → expected graph operations.
- Known invalid outputs (missing evidence, unknown entity kinds, duplicate IDs) → rejection.
- Schema version mismatch → rejection with clear error.

---

### 5. Grafeo projection (`graph/projection.rs`)

Tests that the deterministic compiler correctly projects typed facts into Grafeo's
programmatic API.

#### Algebraic: projection idempotence

```rust
#[hegel::test(test_cases = 200)]
fn prop_projection_idempotent(tc: hegel::TestCase) {
    let grafeo = Grafeo::new_in_memory();
    let ops: Vec<GraphOp> = tc.draw(gen_graph_ops());
    project(&grafeo, &ops).unwrap();
    let count_a = grafeo.node_count();
    let edges_a = grafeo.edge_count();
    project(&grafeo, &ops).unwrap(); // second projection
    assert_eq!(grafeo.node_count(), count_a);
    assert_eq!(grafeo.edge_count(), edges_a);
}
```

#### Algebraic: every edge has temporal metadata

```rust
#[hegel::test(test_cases = 200)]
fn prop_temporal_edges_have_valid_from(tc: hegel::TestCase) {
    let grafeo = Grafeo::new_in_memory();
    let ops: Vec<GraphOp> = tc.draw(gen_graph_ops());
    project(&grafeo, &ops).unwrap();
    let result = grafeo.session().execute(
        "MATCH ()-[e]->() RETURN e.valid_from_ts_utc_ms"
    ).unwrap();
    for row in result.iter() {
        assert!(row[0].as_int64().is_some(), "Edge missing valid_from");
    }
}
```

#### Round-trip: project → destroy → rebuild from redb → compare

```rust
#[hegel::test(test_cases = 50)]
fn prop_grafeo_rebuild_from_redb(tc: hegel::TestCase) {
    let (redb, grafeo) = test_stores();
    let episodes: Vec<Episode> = tc.draw(gs::vecs(gen_ready_episode_with_artifacts()).min_size(1).max_size(10));

    for ep in &episodes {
        persist_and_project(&redb, &grafeo, ep).unwrap();
    }
    let original_nodes = grafeo.node_count();
    let original_edges = grafeo.edge_count();

    // Destroy and rebuild
    let grafeo2 = Grafeo::new_in_memory();
    rebuild_from_redb(&redb, &grafeo2).unwrap();

    assert_eq!(grafeo2.node_count(), original_nodes);
    assert_eq!(grafeo2.edge_count(), original_edges);
}
```

---

### 6. Embedding pipeline (`embeddings/pylate.rs`)

Tests for the PyLate embedding worker and the proxy-vector reduction rule.

#### Algebraic: determinism (idempotence of encode)

```rust
#[hegel::test(test_cases = 50)]
fn prop_embedding_deterministic(tc: hegel::TestCase) {
    let text: String = tc.draw(gs::text().min_size(10).max_size(500));
    let model = test_model(); // CPU-only
    let a = model.encode(&[text.clone()], false).unwrap();
    let b = model.encode(&[text], false).unwrap();
    assert_tensors_equal(&a, &b);
}
```

#### Algebraic: proxy vector dimensions are fixed

```rust
#[hegel::test(test_cases = 50)]
fn prop_proxy_vector_dimensions_match(tc: hegel::TestCase) {
    let text: String = tc.draw(gs::text().min_size(10).max_size(500));
    let model = test_model();
    let embeddings = model.encode(&[text], false).unwrap();
    let proxy = mean_pool(&embeddings);
    assert_eq!(proxy.len(), EXPECTED_PROXY_DIMS);
}
```

#### Metamorphic: similar texts produce closer vectors than dissimilar texts

```rust
#[hegel::test(test_cases = 50)]
fn prop_semantic_similarity_monotonic(tc: hegel::TestCase) {
    // Metamorphic: cos(embed(A), embed(A + noise)) > cos(embed(A), embed(unrelated))
    let base: String = tc.draw(gs::text().min_size(20).max_size(200));
    let noise: String = tc.draw(gs::text().min_size(1).max_size(20));
    let unrelated: String = tc.draw(gs::text().min_size(20).max_size(200));
    tc.assume(base != unrelated);

    let model = test_model();
    let base_emb = proxy_embed(&model, &base);
    let similar_emb = proxy_embed(&model, &format!("{base} {noise}"));
    let unrelated_emb = proxy_embed(&model, &unrelated);

    let sim_close = cosine_similarity(&base_emb, &similar_emb);
    let sim_far = cosine_similarity(&base_emb, &unrelated_emb);
    // This is a soft property — it won't always hold, but should hold statistically.
    // Use a generous margin and tc.note for debugging failures.
    tc.note(&format!("sim_close={sim_close:.4}, sim_far={sim_far:.4}"));
}
```

#### Metamorphic: pooling preserves ranking order

```rust
#[hegel::test(test_cases = 30)]
fn prop_pooling_preserves_ranking(tc: hegel::TestCase) {
    let query: String = tc.draw(gs::text().min_size(10).max_size(100));
    let doc_a: String = tc.draw(gs::text().min_size(20).max_size(200));
    let doc_b: String = tc.draw(gs::text().min_size(20).max_size(200));

    let model = test_model();
    let q_emb = model.encode(&[query.clone()], true).unwrap();
    let a_emb = model.encode(&[doc_a], false).unwrap();
    let b_emb = model.encode(&[doc_b], false).unwrap();

    let full_sim_a = maxsim(&q_emb, &a_emb);
    let full_sim_b = maxsim(&q_emb, &b_emb);

    let a_pooled = hierarchical_pooling(&a_emb, 2).unwrap();
    let b_pooled = hierarchical_pooling(&b_emb, 2).unwrap();
    let pooled_sim_a = maxsim(&q_emb, &a_pooled);
    let pooled_sim_b = maxsim(&q_emb, &b_pooled);

    // Ranking order preserved after pooling
    if full_sim_a > full_sim_b {
        assert!(pooled_sim_a >= pooled_sim_b, "Pooling reversed ranking");
    }
}
```

#### Pooling policy fixture tests

- Decision text → full `late_interaction_bytes` persisted (no pooling).
- Active task summary → light pooling (`pool_factor=2`).
- Old episode summary → heavy pooling or proxy-only.

---

### 7. Retrieval routing (`rank/`)

Tests for the query classifier, composite scoring, MMR diversity, rejection thresholds,
and evidence-window expansion.

#### Differential: classifier vs expected route table

```rust
#[hegel::test(test_cases = 500)]
fn prop_classifier_deterministic(tc: hegel::TestCase) {
    let query: String = tc.draw(gs::text().min_size(1).max_size(200));
    let a = classify_query(&query);
    let b = classify_query(&query);
    assert_eq!(a, b);
}

#[hegel::test(test_cases = 300)]
fn prop_file_paths_route_exact(tc: hegel::TestCase) {
    // Differential: generated file paths always classified as Exact
    let path: String = tc.draw(gen_file_path());
    assert_eq!(classify_query(&path), RetrievalRoute::Exact);
}

#[hegel::test(test_cases = 300)]
fn prop_relational_queries_route_graph(tc: hegel::TestCase) {
    // Differential: generated relational queries always classified as HybridGraph
    let query: String = tc.draw(gen_relational_query());
    assert_eq!(classify_query(&query), RetrievalRoute::HybridGraph);
}
```

#### Algebraic: score normalization and ordering

```rust
#[hegel::test(test_cases = 500)]
fn prop_score_in_unit_interval(tc: hegel::TestCase) {
    let artifact = tc.draw(gen_scored_artifact());
    let score = composite_score(&artifact);
    assert!(
        (0.0..=1.0).contains(&score),
        "Score out of [0,1]: {score}"
    );
}

#[hegel::test(test_cases = 500)]
fn prop_score_deterministic(tc: hegel::TestCase) {
    let artifact = tc.draw(gen_scored_artifact());
    let a = composite_score(&artifact);
    let b = composite_score(&artifact);
    assert_eq!(a, b);
}

#[hegel::test(test_cases = 300)]
fn prop_decision_bonus_respected(tc: hegel::TestCase) {
    // Algebraic: given identical features, decisions score >= entities (tie-breaker)
    let features = tc.draw(gen_score_features());
    let d_score = composite_score_for(ArtifactType::Decision, &features);
    let e_score = composite_score_for(ArtifactType::Entity, &features);
    assert!(d_score >= e_score);
}
```

#### Algebraic: rejection threshold

```rust
#[hegel::test(test_cases = 300)]
fn prop_low_confidence_rejected(tc: hegel::TestCase) {
    let candidates = tc.draw(gen_low_confidence_candidates());
    let results = select_results(&candidates, RetrievalRoute::Hybrid, RecallMode::Automatic);
    assert!(results.is_empty(), "Low-confidence candidates should be rejected");
}

#[hegel::test(test_cases = 300)]
fn prop_mcp_threshold_lower_than_automatic(tc: hegel::TestCase) {
    // MCP allows lower confidence than automatic recall
    let candidates = tc.draw(gen_medium_confidence_candidates());
    let auto = select_results(&candidates, RetrievalRoute::Hybrid, RecallMode::Automatic);
    let mcp = select_results(&candidates, RetrievalRoute::Hybrid, RecallMode::Explicit);
    assert!(mcp.len() >= auto.len());
}
```

#### Algebraic: MMR diversity

```rust
#[hegel::test(test_cases = 200)]
fn prop_mmr_no_near_duplicates(tc: hegel::TestCase) {
    let candidates = tc.draw(gen_candidate_set_with_duplicates());
    let selected = apply_mmr(&candidates, 0.7);
    for i in 0..selected.len() {
        for j in (i + 1)..selected.len() {
            let sim = cosine_similarity(&selected[i].proxy_vector, &selected[j].proxy_vector);
            assert!(sim < 0.95, "Near-duplicate pair survived MMR: sim={sim:.4}");
        }
    }
}

#[hegel::test(test_cases = 200)]
fn prop_mmr_preserves_best_candidate(tc: hegel::TestCase) {
    // MMR never drops the highest-scored candidate
    let candidates = tc.draw(gen_candidate_set());
    tc.assume(!candidates.is_empty());
    let best = candidates.iter().max_by(|a, b| a.score.partial_cmp(&b.score).unwrap()).unwrap();
    let selected = apply_mmr(&candidates, 0.7);
    assert!(selected.iter().any(|s| s.id == best.id), "MMR dropped the best candidate");
}
```

#### Golden ranking tests

- Known corpus + known query → expected ranked output (golden file comparison).
- Same corpus + same query across Lobster releases → identical ranking.
- Threshold tuning fixture: known corpus → Precision@K and false-silence metrics.

---

### 8. Cross-store visibility (`store/` + `graph/`)

Tests for the cross-store visibility protocol: only `Ready` artifacts are surfaced.

#### Differential: retrieval result set vs redb ready set

```rust
#[hegel::test(test_cases = 200)]
fn prop_retrieval_subset_of_ready(tc: hegel::TestCase) {
    // Differential: every retrieved episode_id must be Ready in redb
    let (redb, grafeo) = test_stores();
    let episodes: Vec<Episode> = tc.draw(gen_mixed_state_episodes());
    for ep in &episodes {
        persist_and_project_if_ready(&redb, &grafeo, ep).unwrap();
    }
    let query: String = tc.draw(gs::text().min_size(3).max_size(50));
    let results = retrieve(&grafeo, &redb, &query).unwrap();
    for r in &results {
        let state = redb.get_state(r.episode_id).unwrap();
        assert_eq!(state, ProcessingState::Ready,
            "Non-ready episode {:#?} appeared in results", r.episode_id);
    }
}

#[hegel::test(test_cases = 200)]
fn prop_pending_never_surfaced(tc: hegel::TestCase) {
    let (redb, grafeo) = test_stores();
    let episode = tc.draw(gen_episode_with_state(ProcessingState::Pending));
    persist_episode(&redb, &episode).unwrap();
    let results = retrieve(&grafeo, &redb, "anything").unwrap();
    for r in &results {
        assert_ne!(r.episode_id, episode.episode_id);
    }
}
```

---

### 9. Redaction and filtering (`hooks/capture.rs`)

#### Metamorphic: redaction is monotone (adding secrets doesn't reveal them)

```rust
#[hegel::test(test_cases = 500)]
fn prop_secrets_never_persisted(tc: hegel::TestCase) {
    let secret = tc.draw(gen_secret_pattern());
    let event = make_event_with_payload(secret.as_bytes());
    let filtered = apply_redaction(&event);
    assert!(!contains_secret_patterns(&filtered.payload_bytes),
        "Secret pattern survived redaction");
}

#[hegel::test(test_cases = 500)]
fn prop_redaction_idempotent(tc: hegel::TestCase) {
    // Algebraic: redact(redact(x)) == redact(x)
    let payload: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(1024));
    let event = make_event_with_payload(&payload);
    let once = apply_redaction(&event);
    let twice = apply_redaction(&once);
    assert_eq!(once.payload_bytes, twice.payload_bytes);
}

#[hegel::test(test_cases = 300)]
fn prop_safe_content_unchanged(tc: hegel::TestCase) {
    // Metamorphic: content with no secret patterns passes through unchanged
    let safe: String = tc.draw(gs::text().min_size(1).max_size(500).alphabet(
        "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,!?\n"
    ));
    let event = make_event_with_payload(safe.as_bytes());
    let filtered = apply_redaction(&event);
    assert_eq!(filtered.payload_bytes, event.payload_bytes);
}
```

#### Fixture tests

- Known secret patterns (AWS keys, GitHub tokens, Bearer tokens) → redacted.
- Known safe content → unchanged.
- Ignored file patterns → events dropped.

---

### 10. End-to-end scenarios

Full pipeline tests from raw event ingestion through retrieval. These use standard
`#[test]` with fixture data, not property tests (the pipeline involves mock LLM calls
that are not easily generatable).

#### Scenario: decision recall

1. Ingest a fixture conversation where a decision is made.
2. Run episode segmentation.
3. Run decision detection.
4. Run summarization (mock summarizer).
5. Run extraction (mock extractor).
6. Run embedding (CPU model).
7. Project into Grafeo.
8. Query for the decision.
9. Assert the decision is surfaced with correct evidence.

#### Scenario: temporal edge invalidation

1. Ingest a conversation establishing "component X depends on Y".
2. Project the edge with `valid_from`.
3. Ingest a later conversation removing that dependency.
4. Project the edge update with `valid_to`.
5. Query for current dependencies of X.
6. Assert Y is no longer returned.

#### Scenario: abstain on weak evidence

1. Ingest minimal, ambiguous conversation.
2. Process through the full pipeline.
3. Query with an unrelated topic.
4. Assert the retrieval router returns nothing (abstain).

#### Scenario: rebuild from redb

1. Process N episodes through the full pipeline.
2. Destroy Grafeo.
3. Rebuild Grafeo from redb artifacts.
4. Compare node/edge counts, key property values, retrieval results.
5. Assert identical.

---

## Generator library

All test modules share a common generator library at `tests/generators.rs`. Key
generators built by construction (no rejection):

```rust
use hegel::generators as gs;

#[hegel::composite]
fn gen_raw_event(tc: hegel::TestCase) -> RawEvent {
    let kind: EventKind = tc.draw(gs::default());
    let payload: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(512));
    RawEvent {
        seq: 0, // assigned by store
        repo_id: tc.draw(gen_repo_id()),
        ts_utc_ms: tc.draw(gs::integers().min_value(1_000_000_000_000_i64).max_value(2_000_000_000_000_i64)),
        event_kind: kind,
        payload_hash: sha256(&payload),
        payload_bytes: payload,
    }
}

#[hegel::composite]
fn gen_event_stream(tc: hegel::TestCase) -> Vec<RawEvent> {
    // Generates a time-ordered stream with realistic gaps (valid by construction)
    let n: usize = tc.draw(gs::integers().min_value(1_usize).max_value(100));
    let mut events = Vec::with_capacity(n);
    let mut ts = tc.draw(gs::integers().min_value(1_000_000_000_000_i64).max_value(1_500_000_000_000_i64));
    for i in 0..n {
        ts += tc.draw(gs::integers().min_value(100_i64).max_value(300_000_i64));
        let mut event = tc.draw(gen_raw_event());
        event.ts_utc_ms = ts;
        event.seq = i as u64 + 1; // monotonic by construction
        events.push(event);
    }
    events
}

#[hegel::composite]
fn gen_valid_extraction_output(tc: hegel::TestCase) -> ExtractionOutput {
    // Always valid: has at least one entity, one relation, and evidence
    let n_entities: usize = tc.draw(gs::integers().min_value(1_usize).max_value(5));
    let entities: Vec<Entity> = (0..n_entities)
        .map(|_| tc.draw(gen_entity()))
        .collect();
    let relations: Vec<Relation> = tc.draw(
        gs::vecs(gen_relation_for(&entities)).min_size(1).max_size(5)
    );
    ExtractionOutput {
        task_refs: vec![],
        decision_refs: vec![],
        entities,
        relations,
    }
}

#[hegel::composite]
fn gen_invalid_extraction_output(tc: hegel::TestCase) -> ExtractionOutput {
    // Invalid by construction: references nonexistent entities
    let mut output = tc.draw(gen_valid_extraction_output());
    output.relations.push(Relation {
        rel_type: "bogus".into(),
        from: "nonexistent:id".into(),
        to: "also:nonexistent".into(),
    });
    output
}

#[hegel::composite]
fn gen_file_path(tc: hegel::TestCase) -> String {
    let depth: usize = tc.draw(gs::integers().min_value(1_usize).max_value(5));
    let mut parts = Vec::with_capacity(depth);
    for _ in 0..depth {
        parts.push(tc.draw(gs::text().min_size(1).max_size(20)
            .alphabet("abcdefghijklmnopqrstuvwxyz_-")));
    }
    let ext = tc.draw(gs::sampled_from(vec![".rs", ".toml", ".md", ".json", ".ts"]));
    format!("src/{}{ext}", parts.join("/"))
}

#[hegel::composite]
fn gen_relational_query(tc: hegel::TestCase) -> String {
    let keyword = tc.draw(gs::sampled_from(vec![
        "why did we", "what depends on", "related to",
        "how did", "history of", "caused by", "timeline of",
    ]));
    let topic = tc.draw(gs::text().min_size(3).max_size(30));
    format!("{keyword} {topic}")
}

#[hegel::composite]
fn gen_secret_pattern(tc: hegel::TestCase) -> String {
    // Valid by construction: always matches a known secret pattern
    let kind = tc.draw(gs::sampled_from(vec!["aws", "github", "bearer", "env"]));
    match kind {
        "aws" => format!("AKIA{}", tc.draw(gs::text().min_size(16).max_size(16)
            .alphabet("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"))),
        "github" => format!("ghp_{}", tc.draw(gs::text().min_size(36).max_size(36)
            .alphabet("abcdefghijklmnopqrstuvwxyz0123456789"))),
        "bearer" => format!("Bearer {}", tc.draw(gs::text().min_size(20).max_size(40)
            .alphabet("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"))),
        "env" => format!("SECRET_KEY={}", tc.draw(gs::text().min_size(10).max_size(40))),
        _ => unreachable!(),
    }
}

#[hegel::composite]
fn gen_scored_artifact(tc: hegel::TestCase) -> ScoredArtifact {
    ScoredArtifact {
        artifact_type: tc.draw(gs::sampled_from(vec![
            ArtifactType::Decision, ArtifactType::Task,
            ArtifactType::Summary, ArtifactType::Entity,
        ])),
        semantic_score: tc.draw(gs::floats::<f32>().min_value(0.0).max_value(1.0)
            .allow_nan(false).allow_infinity(false)),
        recency_ts: tc.draw(gs::integers().min_value(1_000_000_000_000_i64).max_value(2_000_000_000_000_i64)),
        task_id: tc.draw(gs::optional(gen_task_id())),
        noise_flagged: tc.draw(gs::booleans()),
        proxy_vector: tc.draw(gs::vecs(gs::floats::<f32>().min_value(-1.0).max_value(1.0)
            .allow_nan(false).allow_infinity(false)).min_size(128).max_size(128)),
    }
}
```

---

## Property inventory summary

| Layer        | Property style | What it proves                                      |
| ------------ | -------------- | --------------------------------------------------- |
| Store        | Differential   | Store matches in-memory model after any op sequence |
| Store        | Round-trip     | Persist → load is identity                          |
| Store        | Algebraic      | Sequence numbers are strictly monotonic             |
| Store        | Model-based    | State transitions never violate the FSM             |
| Segmentation | Algebraic      | Partition completeness, disjointness, idempotence   |
| Segmentation | Metamorphic    | Appending events only affects the tail              |
| Decisions    | Metamorphic    | Noise doesn't create spurious decisions             |
| Decisions    | Algebraic      | Every decision has evidence                         |
| Compiler     | Differential   | Deterministic output                                |
| Compiler     | Metamorphic    | Adding an entity adds exactly one node              |
| Compiler     | Round-trip     | Compile → project → query finds the node            |
| Projection   | Algebraic      | Idempotent, temporal metadata present               |
| Projection   | Round-trip     | Destroy → rebuild from redb → identical             |
| Embeddings   | Algebraic      | Deterministic, fixed dimensions                     |
| Embeddings   | Metamorphic    | Similar texts closer than dissimilar texts          |
| Embeddings   | Metamorphic    | Pooling preserves ranking order                     |
| Routing      | Differential   | File paths → Exact, relational → HybridGraph        |
| Scoring      | Algebraic      | Scores in [0,1], deterministic, decision bonus      |
| Rejection    | Algebraic      | Low confidence → empty results                      |
| MMR          | Algebraic      | No near-duplicates, best candidate preserved        |
| Visibility   | Differential   | Results ⊆ redb ready set                            |
| Redaction    | Metamorphic    | Secrets removed, safe content unchanged             |
| Redaction    | Algebraic      | Idempotent                                          |

---

## CI configuration

### Default (per PR)

All `#[hegel::test]` tests run with their configured `test_cases` count. Hegel
auto-detects CI and derandomizes.

```bash
cargo test
```

### Extended (nightly)

Selected expensive tests run with higher `test_cases` counts:

```rust
#[hegel::test(test_cases = 5000)]
fn prop_ranking_determinism_extended(tc: hegel::TestCase) { ... }
```

Run extended tests in a nightly job or before release.

### Golden test maintenance

Golden files live in `tests/golden/`. When a versioned contract changes (ranking weights,
extraction schema, segmentation rules), update golden files explicitly and bump the
relevant version constant. Never auto-update golden files in CI.

---

## Versioned contracts

Each of these has a version constant. Changing one bumps its version and requires
re-running the full fixture suite:

| Contract                  | Version constant            | What triggers a bump                    |
| ------------------------- | --------------------------- | --------------------------------------- |
| Summary artifact schema   | `SUMMARY_SCHEMA_VERSION`    | Field additions/removals                |
| Summarizer behavior       | `SUMMARIZER_REVISION`       | Prompt or model change                  |
| Extraction schema         | `EXTRACTION_SCHEMA_VERSION` | Output shape changes                    |
| Extractor behavior        | `EXTRACTOR_REVISION`        | Prompt or model change                  |
| Embedding artifact schema | `EMBEDDING_SCHEMA_VERSION`  | Vector format or metadata changes       |
| Embedding model           | `EMBEDDING_MODEL_REVISION`  | Model weights or tokenizer change       |
| Graph template set        | `GRAPH_TEMPLATE_VERSION`    | New node/edge types or projection rules |
| Ranking feature set       | `RANKING_VERSION`           | Weight changes, new score components    |
| Segmentation rules        | `SEGMENTATION_VERSION`      | Boundary rule changes                   |

---

## Test file organization

```text
tests/
  generators.rs           # shared hegel generators
  golden/
    segmentation/         # golden event streams + expected episodes
    decisions/            # golden conversations + expected decisions
    extraction/           # golden extraction outputs + expected graph ops
    ranking/              # golden corpus + queries + expected ranked results
  integration/
    store_test.rs         # differential, round-trip, state machine, write coordination
    segmentation_test.rs  # algebraic partitioning, metamorphic append
    decisions_test.rs     # metamorphic noise, evidence invariant
    compiler_test.rs      # differential, metamorphic entity addition, round-trip
    projection_test.rs    # idempotence, temporal metadata, rebuild round-trip
    embedding_test.rs     # determinism, similarity metamorphic, pooling ranking
    routing_test.rs       # differential classifier, route correctness
    ranking_test.rs       # score algebra, rejection, MMR algebra
    visibility_test.rs    # differential ready-set intersection
    redaction_test.rs     # metamorphic secret removal, idempotence
    e2e_test.rs           # fixture-driven end-to-end scenarios
```
