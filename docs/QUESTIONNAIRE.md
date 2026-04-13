# Lobster questionnaire

Based on `README.md` and follow-up clarification rounds.

## Round 1

### What is Lobster primarily supposed to do for a Claude Code session?

- Long-term memory layer — Persist facts, decisions, files, and context across sessions so Claude can recall them later.
- Session analytics/observability — Record everything mainly for inspection, debugging, and audit rather than active recall.
- **Autonomous knowledge builder** — Continuously organize session data into a graph and generate new insights or summaries from it. ← chosen
- All three — It should persist memory, expose observability, and actively synthesize new knowledge.

### How deeply should Lobster integrate with Claude Code hooks?

- Passive hook listener — Only observe hook events and store them without changing behavior.
- **Memory-aware assistant** — Observe hooks and feed relevant recalled memory back into Claude workflows. ← chosen
- Active workflow orchestrator — Use hooks to trigger recalls, summaries, graph updates, and automated actions.

### What kinds of data should Lobster remember as first-class memory?

- Only conversation content — Store prompts, responses, and summaries of the session.
- Conversation + tool usage — Also store commands, file edits, reads, and execution outcomes.
- **Everything meaningful** — Conversation, tool usage, code changes, decisions, entities, relationships, and derived embeddings/graph structure. ← chosen

### What does "Dreaming AI by default" mean in this project?

- Background summarization — Periodically compress or summarize stored memory into higher-level notes.
- Knowledge graph enrichment — Infer entities, relationships, tags, and links from stored events.
- Generative reflection — Produce hypotheses, reminders, TODOs, or insights from past sessions.
- **Combination of all of the above** — Run summarization, enrichment, and reflection automatically in the background. ← chosen

## Round 2

### Who is the primary user of Lobster at first launch?

- **Solo developer** — A single developer using Claude Code locally on their own machine and repos. ← chosen
- Small team — A few developers sharing conventions and possibly shared memory artifacts.
- Many projects/workspaces — One user or org managing memory across lots of repos and contexts.

### When should Lobster surface remembered context back into the workflow?

- Only on explicit request — Recall happens when the user or agent asks for memory.
- Automatically on relevant events — Use hooks to inject or suggest relevant memories during normal work.
- **Hybrid** — Mostly automatic relevance-based recall, with explicit memory queries also supported. ← chosen

### What is the intended role of the graph knowledge layer relative to embeddings and key/value storage?

- Graph as optional metadata — redb is the main store, embeddings power similarity, and the graph adds light relationships.
- **Graph as core semantic model** — The graph is central, with redb and embeddings supporting retrieval and persistence. ← chosen
- Equal peers — Use all three together: redb for storage, embeddings for similarity, graph for structured meaning.

### What should Lobster produce for Claude Code besides raw storage?

- **Relevant memory snippets** — Return retrieved memories and summaries when they seem useful. ← chosen
- Context packs — Assemble structured bundles of relevant history, entities, decisions, and files for the current task.
- Insights and prompts — Also generate reminders, hypotheses, TODOs, and reflection-style outputs.
- All of the above — It should retrieve snippets, build context packs, and generate insights.

## Round 3

### What should Lobster explicitly avoid doing in its first usable version?

Chosen by user via custom response: **1 and 3**

- **No cloud/service backend** — Keep everything local and offline-first; no hosted sync or external dependency. ← chosen
- No aggressive autonomy — Avoid taking actions or changing workflows beyond storing and surfacing memory.
- **No multi-user complexity** — Skip permissions, collaboration, and shared-memory features at first. ← chosen
- All three limits — Keep v1 local, non-invasive, and single-user focused.

### What does "deterministic behavior" mean operationally for Lobster?

- Repeatable storage/indexing — The same input events should always produce the same persisted state and graph updates.
- Predictable retrieval ranking — Given the same memory and query context, recall should be stable and reproducible.
- **Both storage and retrieval** — Ingestion and recall behavior should both be as repeatable as practical. ← chosen

### When should background "dreaming" run?

- After important hooks — Trigger summarization/enrichment right after significant session events.
- Idle-time/background — Run when Claude Code is idle or at low-pressure moments.
- End-of-session — Mainly synthesize memories when a work session wraps up.
- **Mix of all three** — Use event-driven, idle-time, and end-of-session synthesis. ← chosen

### What matters most when Lobster decides which memory to surface automatically?

- Task relevance — Prioritize memories that best match the current file/task/problem.
- Decision importance — Favor prior decisions, constraints, and commitments over generic similarity.
- **Recency + relevance** — Balance semantic similarity with what happened recently. ← chosen
- Rich context balance — Blend relevance, importance, recency, and graph relationships.

## Round 4

### How should Lobster scope memory by default?

- **Per repository** — Keep memory mainly tied to the current repo/workspace, with little cross-project recall. ← chosen
- Per user, cross-repo — Build one personal memory that can connect ideas across repositories.
- Hybrid scoped memory — Keep repo-local memory first, but allow selective cross-repo linking and recall.

### What counts as a "session" for Lobster's memory model?

- Single Claude Code run — A session starts when Claude Code launches and ends when it exits.
- Task thread — A session is a coherent work thread that may span multiple launches.
- **Continuous personal timeline** — Sessions are just segments of one ongoing memory stream. ← chosen

### How should automatically surfaced memory appear to the user or agent?

- Quiet inline hints — Small relevant reminders with minimal interruption.
- Structured memory block — A dedicated contextual payload inserted into the prompt/workflow.
- **Tiered output** — Usually brief hints, but expand into a structured block when confidence is high. ← chosen

### How aggressively should Lobster retain history?

- Store nearly everything — Persist almost all meaningful events and rely on summarization to manage scale.
- Selective storage — Filter heavily at ingestion and keep only high-value memory candidates.
- **Raw + distilled** — Keep broad raw history plus progressively distilled summaries/graph abstractions. ← chosen

## Round 5

### Which Claude Code events should Lobster treat as the highest-value triggers in v1?

- Prompts and responses — Focus on user requests, assistant replies, and conversational turns as the main memory stream.
- Tool activity and file changes — Prioritize commands, reads, edits, writes, test runs, and code diffs as the richest signals.
- Decision points — Focus on plans, approvals, errors, fixes, and conclusions more than raw event volume.
- **Blend of all three** — Capture all of them, but rank prompts, tools, and decisions differently during indexing. ← chosen

### What should be the core atomic memory unit inside Lobster?

- Raw event records — Store each hook/tool/conversation event as its own canonical memory item.
- **Derived episodes** — Group related low-level events into higher-level episodes before most retrieval happens. ← chosen
- Both layers — Keep raw events for fidelity and build episode-level abstractions for retrieval and dreaming.

### When Lobster auto-surfaces memory, what is the best default payload size?

Chosen by user via custom response: **Option 3, but allow expanding with an MCP tool only, such as `memory_search`.**

- 1-3 short snippets — Keep it very light so it rarely distracts from the main task.
- Small structured bundle — Return a compact pack with snippets, related files, and one or two key decisions.
- **Expandable layered recall** — Start tiny, but include a path to expand into richer context when confidence is high. ← chosen (with expansion exposed via MCP tool)

### What would make Lobster feel clearly successful in its first real version?

- It reliably reminds Claude of past decisions — The biggest win is reducing repeated explanations and forgotten constraints.
- It reconstructs project context fast — The biggest win is helping Claude regain situational awareness after time away.
- It generates useful synthesized insights — The biggest win is that dreaming produces genuinely helpful summaries and ideas.
- **All three, in that order** — Decision recall first, context reconstruction second, synthesis third. ← chosen

## Round 6

### How should Lobster expose memory access in Claude Code day to day?

- Read-only MCP tools — Expose tools like memory_search, memory_get, and memory_recent, while hook-based recall stays automatic.
- Prompt injection only — Do not expose explicit tools at first; only surface memory through hooks and context injection.
- **Both hooks and MCP tools** — Use automatic hook-based recall plus explicit MCP tools for deeper inspection and expansion. ← chosen

### What should the graph's first-class nodes and edges represent in v1?

- People, files, tasks, decisions — Model practical work entities first, with edges like worked_on, decided, mentioned_in, and depends_on.
- Events and episodes only — Keep the graph close to the raw memory timeline and infer everything else later.
- **Mixed semantic graph** — Represent episodes, tasks, files, decisions, entities, and relationships together from the start. ← chosen

### What should dreaming be allowed to write back into memory automatically?

- Only summaries — It may create distilled summaries, but not speculative insights or reminders.
- **Summaries plus derived links** — It may add summaries, tags, relationships, and graph enrichments, but avoid speculative content. ← chosen
- Summaries, links, and reflections — It may also add hypotheses, reminders, TODO candidates, and reflective notes, clearly marked as derived.

### How much user control should exist over what Lobster stores or recalls in v1?

- **Minimal controls** — Keep defaults simple; maybe just on/off and basic verbosity. ← chosen
- Practical controls — Allow repo-level enablement, memory clearing, recall verbosity, and maybe ignore rules.
- Fine-grained controls — Support detailed filters, categories, retention policies, and per-hook behavior from the start.

## Round 7

### How should Lobster identify and connect the same concept over time in v1?

- Mostly by repo-local names — Use file paths, task labels, branch context, and explicit names, with limited entity resolution.
- **Deterministic canonicalization** — Normalize files, tasks, decisions, and entities into stable IDs whenever possible. ← chosen
- Aggressive semantic merging — Use embeddings and graph inference to merge similar concepts even when names differ.

### Where should automatic memory recall enter the Claude workflow by default?

- Before assistant response generation — Provide memory as context before Claude starts answering.
- After tool-heavy milestones — Inject memory mainly after edits, tests, failures, or notable task transitions.
- **Both, but lightly** — Use small pre-response hints and milestone-based reminders without overwhelming the prompt. ← chosen

### How should you judge whether Lobster's automatic recall is good enough?

- Low annoyance — It rarely surfaces irrelevant memory or distracts from the task.
- High usefulness — When it appears, it often helps Claude make a better decision or avoid repetition.
- **Both usefulness and restraint** — Measure success as helpful recall with low interruption and low irrelevance. ← chosen

### What kinds of memory should Lobster avoid surfacing automatically unless explicitly queried?

- Stale low-confidence memories — Old or weakly related items should stay hidden unless asked for.
- Raw noisy event trails — Low-level logs and event spam should stay behind MCP tools, not appear inline.
- **Both of those** — Keep stale or noisy memories out of automatic recall unless the user drills down. ← chosen

## Round 8

### How should a developer start using Lobster in v1?

- **Per-repo install/config** — Enable it intentionally inside each repository and keep memory local to that project. ← chosen
- Global personal daemon — Run one local service for the user, with repos registering themselves automatically.
- Hook plugin with local store — Install Lobster once as a Claude Code hook integration, while it manages repo-local memory stores under the hood.

### What explicit MCP queries should feel essential in v1?

- Search and recent — `memory_search` and `memory_recent` are enough for the first cut.
- Search, recent, decision lookup — Also support asking for prior decisions or constraints tied to a repo/task/file.
- **Search, recent, graph navigation** — Also expose related entities, episodes, and graph neighbors when drilling down. ← chosen

### If Lobster is unsure whether a memory is relevant, what should it do by default?

- Stay silent — Do not surface it automatically; keep uncertain memories behind explicit queries.
- Surface with low-confidence label — Show it briefly but make the uncertainty visible.
- **Put it behind expand-only recall** — Mention that more history exists, but require MCP/tool expansion to see uncertain memories. ← chosen

### How important is it that Lobster's stored memory can be exported or inspected outside the app?

- Very important — The data should be inspectable/exportable for trust, backup, and future migration.
- Nice to have — Useful eventually, but not necessary for the first version.
- **Low priority** — Internal correctness and usefulness matter more than exportability at first. ← chosen

## Round 9

### What matters more for Lobster's automatic recall path in v1?

- **Very low latency** — Recall should be almost invisible in cost, even if that means simpler retrieval. ← chosen
- Balanced speed and depth — Keep it fast, but allow a bit more work when it materially improves relevance.
- Best recall quality — Accept noticeable extra work if it significantly improves memory usefulness.

### How transparent should Lobster be about why it surfaced a memory?

- **Minimal explanation** — Just show the memory; avoid extra explanation unless asked. ← chosen
- Short rationale — Briefly indicate why it matched, such as related file, task, or recent decision.
- Full provenance — Show confidence, source episode, graph path, and match reasons by default.

### How visible should background dreaming artifacts be to the user in v1?

- **Mostly invisible** — Dreaming improves retrieval behind the scenes and only shows up when queried or strongly relevant. ← chosen
- Visible as summaries — Expose distilled summaries and links when they become important, but keep internals quiet.
- Actively surfaced — Regularly present dream-generated summaries or insights even without a direct task trigger.

### What is the strongest first demo for Lobster?

- **Remember prior decisions** — Claude returns to a repo and immediately recalls constraints, choices, and why they were made. ← chosen
- Rebuild context after time away — Claude quickly regains project understanding from prior episodes, files, and tasks.
- Navigate memory graph — A user drills through related episodes, files, and decisions via MCP tools and sees the graph pay off.
- All three in sequence — Show decision recall, context reconstruction, and graph/MCP exploration together.

## Round 10

### What should be the canonical persisted representation of memory in v1?

- **Episode-first records** — Persist higher-level episodes as the main durable unit, with raw hook events only as supporting detail. ← chosen
- Event-first log — Persist an append-only event log as the source of truth, and derive episodes/graph views from it.
- Dual persistence — Persist both raw events and derived episodes/graph state as first-class durable data.

### When should Lobster update its graph in v1?

- **Inline on ingestion** — Update graph state immediately as events arrive so recall always sees fresh structure. ← chosen
- Asynchronous near-real-time — Write raw memory immediately, then build or refine graph state shortly after in the background.
- Mostly dreaming-time — Do minimal structure at ingestion and let background passes build the graph later.

### How should Lobster recognize that something is an important decision worth remembering?

- Explicit markers only — Only store decisions when hooks or the user clearly mark them as decisions.
- Deterministic heuristics — Use rule-based signals from language, edits, plans, approvals, and outcomes to detect decisions.
- **Heuristics plus confirmation** — Detect likely decisions automatically, but allow them to be confirmed or promoted later. ← chosen

### How tightly should memory items be linked to code artifacts in v1?

- File-level links — Attach memories to repositories and file paths, without deeper code structure awareness.
- File plus symbol awareness — Also connect memories to functions, types, modules, or symbols when deterministically available.
- **Task-centric only** — Focus on task and episode relationships first; keep file links lightweight. ← chosen

## Round 11

### What should cause Lobster to start or end an episode in v1?

- Hook-based boundaries — Use deterministic hook events, idle gaps, and repo/task transitions to segment episodes.
- Task-intent boundaries — Start/end episodes mainly when the user's goal or task intent changes.
- **Hybrid segmentation** — Use hook timing and task-intent signals together, with deterministic tie-breaks. ← chosen

### How should confirmation of an automatically detected decision work in practice?

- **Silent auto-promotion** — Promote high-confidence decisions automatically with no explicit user step. ← chosen
- Lightweight confirmation hook — Show a tiny prompt or hook-based chance to confirm, reject, or ignore the detected decision.
- MCP-only promotion — Keep detections provisional until a user or agent explicitly promotes them through a tool.

### What role should embeddings play in v1 relative to the graph and heuristics?

- Fallback retrieval aid — Use embeddings mainly for similarity search when graph links or deterministic signals are insufficient.
- **Primary retrieval engine** — Let embeddings do most of the retrieval work, with graph and heuristics as secondary support. ← chosen
- Ingestion enrichment only — Use embeddings mostly to tag or cluster memories at write time, not for the fast recall path.

### If a user wants memory removed in v1, what granularity matters most?

- **Whole-repo reset** — The main need is clearing all memory for a repository. ← chosen
- Episode-level deletion — Users should be able to forget specific work episodes without wiping the repo.
- Decision/item-level deletion — Users should be able to remove particular remembered facts, decisions, or nodes.

## Round 12

### What should Lobster look like operationally in v1 inside a repo?

- **Single local binary** — A Rust binary handles hooks, storage, graph updates, and MCP-facing queries with minimal moving parts. ← chosen
- Binary plus background daemon — Use a small resident local service for indexing and retrieval while hooks talk to it.
- Library-first integration — Make Lobster mainly a library/crate used by Claude hook glue and MCP wrappers.

### Which explicit memory tool feels most essential beyond search and recent?

Chosen by user via custom response: **all three of them**

- **memory_decisions** — A focused tool to retrieve prior decisions, constraints, and rationale. ← chosen
- **memory_neighbors** — A graph-navigation tool to walk related episodes, entities, and links. ← chosen
- **memory_context** — A tool that assembles a compact task-oriented context bundle for the current work. ← chosen

### What should Lobster prove in tests to earn trust in v1?

- Deterministic ingestion and retrieval — Same inputs produce the same stored state and the same ranked recall results.
- Useful decision recall — End-to-end fixtures show it reliably surfaces the right prior decisions in realistic workflows.
- **Both, with determinism first** — Prove deterministic behavior first, then validate decision recall quality on realistic traces. ← chosen

### If the memory system is unavailable or errors during a session, how should Claude Code behave?

- Fail open quietly — Normal coding continues; Lobster simply stops contributing memory until it recovers.
- **Fail open with warning** — Do not block work, but surface a visible warning that memory is degraded. ← chosen
- Fail closed for hooks — Treat memory as critical enough that hook failures should stop or block affected flows.

## Round 13

### How strict should Lobster's internal memory schema be in v1?

- **Strong typed schema** — Use explicit Rust types and deterministic fields for episodes, decisions, entities, and links. ← chosen
- Mostly typed with flexible metadata — Keep core records strongly typed, but allow extension fields for evolving memory annotations.
- Loose document model — Favor flexible JSON-like records first and harden schema later.

### Which kind of Claude Code hook output matters most for high-value memory extraction?

- Assistant/user conversation turns — Natural-language exchanges carry the most important durable intent and decisions.
- Tool executions and outcomes — Commands, edits, reads, tests, and failures are the richest signals of real work.
- Plans and task transitions — Shifts in goals, plans, and milestones matter more than the raw actions themselves.
- **Need all three, differently weighted** — Conversation, tools, and plans all matter, but with different extraction rules and ranking weights. ← chosen

### What should a remembered decision record contain at minimum in v1?

- **Decision and rationale** — Just the core choice and why it was made. ← chosen
- Decision, rationale, and scope — Also record what repo/task/files or entities the decision applies to.
- Decision, rationale, scope, and evidence — Also keep supporting events such as relevant prompts, edits, tests, or outcomes.

### How should Lobster be configured per repo in v1?

- **Convention over configuration** — Almost no config; sensible defaults and maybe one enable file. ← chosen
- Small explicit config file — A simple repo-local config defines storage path, recall level, and a few toggles.
- CLI-driven setup only — Prefer installation commands and generated defaults over a hand-edited config file.

## Round 14

### How should Pylate be used in Lobster v1?

- **Primary retrieval embeddings** — Pylate is the main embedding engine for recall, similarity search, and ranking candidate memories. ← chosen
- Ingestion-time enrichment — Pylate mainly helps tag, cluster, or link memories during write-time, not the fast recall path.
- Both retrieval and enrichment — Use Pylate both for fast retrieval and for enrichment tasks like clustering or semantic linking.

### What exactly should Qwen/Qwen3.5-2B via Candle generate for the graph layer?

- **Entity and edge extraction** — Use the model to identify nodes and relationships from episodes, then write structured graph updates. ← chosen
- Summaries that heuristics convert — Use the model to summarize episodes, and let deterministic rules convert those summaries into graph structure.
- Full graph proposals — Let the model propose graph nodes, links, tags, and decision candidates directly, then post-process them.

### Given the goal of deterministic behavior, how should the Qwen/Candle graph-generation step be handled?

- **Strictly constrained decoding** — Use deterministic prompts, fixed parameters, and structured outputs so the same input yields the same graph proposal. ← chosen
- Best-effort deterministic pipeline — Keep prompts and settings stable, but accept some small variation if the graph remains useful.
- Offline enrichment only — Use the model only in dreaming/background passes so any non-determinism stays out of the main recall path.

### You said there were three things to ask about, but listed two; what should the third topic be?

Chosen by user via custom response: **nothing, you can ignore**

- Storage/infrastructure choice — The third topic is about which store or graph backend should own the structured memory.
- Hook/MCP integration shape — The third topic is about how embeddings and graph generation connect to hooks and MCP tools.
- Model/runtime constraints — The third topic is about performance, latency, quantization, and local hardware/runtime limits.

## Round 15

### What should Pylate embed as the main searchable unit in v1?

- Whole episodes — Generate one main embedding per episode so retrieval is fast and coarse-grained.
- **Decisions and summaries** — Embed distilled items like decisions, rationales, and episode summaries rather than all raw content. ← chosen
- Multiple views per episode — Store embeddings for episode summaries plus important decisions or sub-items for better recall precision.

### How should embeddings interact with graph structure during automatic recall?

- **Embeddings first, graph reranks** — Use Pylate to fetch candidates, then use graph links and heuristics to refine ordering or filtering. ← chosen
- Graph first, embeddings fill gaps — Prefer graph-nearby candidates, and only use embeddings when structural links are weak.
- Embeddings only for v1 — Keep the main recall path purely embedding-based and use the graph mostly for explicit MCP navigation.

### What should the Candle-run Qwen model be required to emit for each processed episode?

Chosen by user via custom response: **it should use the insert queries from grafeo**

- Strict JSON extraction — A tightly defined schema of entities, edge types, and supporting spans or evidence.
- Tagged text blocks — A constrained text format that a deterministic parser converts into graph updates.
- Decision-centric extraction — Focus mainly on decisions, actors, tasks, and a small set of edge types rather than a broad graph schema.

### Where should the Qwen/Candle graph extraction run in the lifecycle?

- **Inline during ingestion** — Run immediately when an episode is finalized so graph state is always current. ← chosen
- Short deferred pass — Persist the episode first, then run extraction seconds later in a local background pass.
- Dreaming-only pass — Only run Qwen during idle/end-of-session synthesis, not on the hot ingestion path.

## Round 16

### How constrained should the Grafeo insert queries generated by Qwen be in v1?

- **Tiny fixed query set** — Allow only a very small number of insert templates and edge types so model output stays tightly bounded. ← chosen
- Moderate fixed schema — Support a practical but still closed set of node/edge insert patterns for common coding-memory concepts.
- Broad graph vocabulary — Allow many insert patterns so the model can build a richer graph from the start.

### What context should be sent to Qwen when generating Grafeo inserts for an episode?

- Episode summary only — Feed only the distilled episode summary so extraction is fast and deterministic.
- Summary plus decisions — Send the summary and the already-detected decision/rationale items for more precise graph construction.
- **Full episode bundle** — Send summary, decisions, key tool outcomes, and important conversational spans from the episode. ← chosen

### If Qwen produces invalid or low-confidence Grafeo inserts, what should Lobster do?

- Drop graph update, keep episode — Persist the episode and embeddings, but skip graph changes for that case.
- **Retry with tighter constraints** — Run one deterministic recovery pass with stricter instructions before giving up. ← chosen
- Fallback to heuristics — If model extraction fails, create a minimal graph update using deterministic rule-based extraction.

### When should Pylate embeddings be computed for summaries and decisions?

- **Inline with ingestion** — Compute embeddings immediately so recall is ready as soon as the episode lands. ← chosen
- Two-stage inline — Embed decisions first for fast recall, then summaries right after in the same ingestion flow.
- Deferred but soon — Persist text first and compute embeddings in a short local background pass.

## Round 17

### Which kinds of Grafeo inserts should be in the tiny fixed query set for v1?

- Episodes, decisions, files, links — Only create episode nodes, decision nodes, file references, and a few simple relationship edges.
- **Episodes, decisions, tasks, entities** — Also include task and general entity nodes so the graph can capture more semantic structure. ← chosen
- Decisions-first only — Keep the graph extremely narrow at first: mainly decisions and their core relations.

### Should generated graph inserts keep explicit evidence back-links to the source episode or spans?

- **Yes, always** — Every inserted node or edge should point back to the originating episode or supporting spans. ← chosen
- Only for decisions — Require evidence links for important decision-related graph facts, but not for everything.
- No, keep graph lean — Do not store explicit evidence references in v1 unless needed later.

### How should the Qwen/Candle model be handled operationally in v1?

- Bundled default — Lobster assumes a standard local Candle model setup with convention-over-configuration defaults.
- **Auto-download on first use** — The binary fetches or prepares the model automatically when graph extraction is first needed. ← chosen
- User-provided local model — The user is responsible for placing or registering the Candle model locally.

### If inline ingestion includes both Pylate and Qwen, what latency target feels acceptable per finalized episode in v1?

- **Sub-second** — The whole ingestion pipeline should usually finish in under one second. ← chosen
- 1 to 3 seconds — A small but noticeable delay is acceptable if quality is much better.
- 3 to 10 seconds — Longer inline processing is acceptable because recall quality matters more than responsiveness.

## Round 18

### What should a task node represent in Lobster v1?

- Current user goal — A task is the active goal or request thread Claude is working on.
- **Work item with lifecycle** — A task can persist across episodes with status-like continuity as work progresses. ← chosen
- Loose thematic cluster — A task is mainly a semantic grouping of related episodes, not a strict work item.

### Which non-task entities matter most in the first graph schema?

- Files, symbols, tools — Focus on code artifacts and the tools used around them.
- People, repos, concepts — Focus on collaborators, repository context, and reusable concepts or ideas.
- **Concepts, constraints, components** — Focus on domain concepts, project constraints, and architectural components. ← chosen

### When MCP tools like memory_neighbors navigate the graph, what should they prioritize?

- **Evidence-backed edges only** — Only traverse links that are directly supported by source evidence and deterministic inserts. ← chosen
- High-value semantic neighbors — Prefer decisions, tasks, and important entities even if some links are less central.
- Task-local neighborhood — Stay close to the current task and episode context to avoid graph sprawl.

### Before enough graph data exists, what should Lobster rely on most?

- **Pylate recall only** — Use embeddings over decisions and summaries until the graph becomes useful. ← chosen
- Heuristics plus embeddings — Combine deterministic cues with Pylate while the graph is still sparse.
- Prompt-time fallback summaries — Generate temporary summaries or hints until enough durable memory accumulates.

## Round 19

### Which relationship types should definitely exist in the first Grafeo query set?

- episode→task, episode→decision, decision→entity — Keep the first graph centered on how episodes produce decisions and how decisions touch entities.
- **task→decision, task→entity, entity→entity** — Center the graph on work items and semantic structure rather than episode chronology. ← chosen
- Use both sets — Include episode, task, decision, and entity relations together in the initial fixed set.

### When Qwen extracts a possible decision, how should Lobster store confidence in v1?

- Binary accepted or rejected — After validation, keep only accepted decisions and do not store graded confidence.
- **Small discrete levels** — Store a few deterministic confidence buckets like low, medium, and high. ← chosen
- Numeric score — Store a numeric confidence value for ranking and later inspection.

### What should validate Qwen-generated Grafeo inserts before they are applied?

- Schema and parser only — If the generated inserts fit the allowed templates, accept them.
- Schema plus evidence checks — Require both valid insert shape and explicit evidence references back to the episode.
- **Schema, evidence, and duplicate checks** — Also ensure inserts do not create redundant nodes or links when canonical equivalents already exist. ← chosen

### What text should Pylate embed for a remembered decision in v1?

- Decision statement only — Embed just the normalized decision itself for maximum brevity and speed.
- Decision plus rationale — Embed the decision together with its reason so retrieval reflects why the choice mattered.
- **Decision, rationale, and task context** — Also include a compact task or episode context sentence to improve matching. ← chosen

## Round 20

### What should be the ingestion order for a finalized episode in v1?

- Summarize → detect decisions → embed → graph extract — Distill first, then create embeddings, then ask Qwen to emit Grafeo inserts from the processed bundle.
- Detect decisions → summarize → graph extract → embed — Promote decisions first, build graph structure, and embed the final distilled memory artifacts last.
- **Parallelize after summarization** — Create a summary first, then run embedding and graph extraction in parallel to minimize latency. ← chosen

### How should the Qwen/Candle model be optimized for local v1 use?

- **Prefer quantized weights by default** — Optimize hard for local speed and memory, even if extraction quality drops a bit. ← chosen
- Balanced default — Use a practical default that keeps quality acceptable while still being lightweight enough for common machines.
- Quality-first default — Favor better extraction quality even if the local runtime cost is noticeably higher.

### How tightly should the Qwen extraction prompt be versioned in v1?

- Hard-versioned prompt contract — Treat the extraction prompt like part of the schema and version it explicitly for determinism.
- Stable but editable prompt — Keep it mostly stable, but allow iterative tuning without treating every change as a schema event.
- **Rapidly evolving prompt** — Tune the prompt frequently during v1 and rely on tests rather than strict versioning. ← chosen

### For memory_search in v1, what should be the default thing it returns first?

- Decision hits — Return matched decisions first, since those are the most valuable memory artifacts.
- Episode summaries — Return summary-level results first, with decisions nested underneath when relevant.
- **Mixed ranked hits** — Return whichever memory artifact ranks highest across decisions, summaries, tasks, and entities. ← chosen

## Round 21

### When Qwen proposes an entity or task that looks similar to an existing one, what should Lobster do in v1?

- Prefer existing canonical nodes — Resolve to an existing node whenever deterministic matching says it is the same thing.
- **Create new then merge later** — Insert conservatively as new nodes and rely on later cleanup or dreaming passes to merge them. ← chosen
- Only dedupe decisions/tasks — Be strict for tasks and decisions, but looser for general entities in early v1.

### How aggressive should automatic recall be when using Pylate-ranked mixed hits?

- Very conservative — Surface only highly relevant hits automatically; keep most results behind explicit MCP queries.
- **Moderately selective** — Allow a small number of good candidates to surface when likely useful, but still avoid frequent noise. ← chosen
- More proactive — Surface memory often if the ranking looks promising, even with some risk of irrelevance.

### What should memory_search return for each mixed ranked hit in v1?

- Compact snippet only — Just the matched text and type, optimized for speed and simplicity.
- Snippet plus metadata — Return the text plus type, repo/task, confidence, and a pointer to expand via other MCP tools.
- **Structured object with graph context** — Return a richer object including related entities or edges for each result. ← chosen

### If graph extraction lags behind or fails for an episode, how should that affect recall?

- Embeddings still fully eligible — Decision and summary embeddings should still make that episode searchable immediately.
- Reduced priority until graph exists — Items without graph structure should be searchable, but rank a bit lower by default.
- **Hide until fully processed** — Only let memories participate in recall after both embeddings and graph extraction are complete. ← chosen

## Round 22

### Since you want deterministic behavior but also a rapidly evolving Qwen prompt, what should define compatibility in v1?

- **Behavioral tests decide** — As long as the deterministic test fixtures still pass, prompt changes are acceptable. ← chosen
- Per-version reproducibility only — Each released build must be deterministic for itself, even if prompt behavior changes between versions.
- No strict compatibility guarantee yet — Prompt evolution can change extraction behavior in v1 and that is acceptable while iterating.

### You chose sub-second inline ingestion and also hiding memories until graph extraction completes; which matters more if both cannot be guaranteed together?

- **Completeness first** — Wait for full graph extraction before recall, even if it occasionally exceeds the latency target. ← chosen
- Latency first — Let memories become searchable quickly even if graph extraction is still pending.
- Best effort with cutoff — Try to finish fully inline within a tight budget, but if the budget is exceeded degrade gracefully in a defined way.

### Earlier you wanted deterministic canonicalization, but later preferred creating new nodes and merging later; how should that work in v1?

- **Strict for tasks/decisions only** — Canonicalize tasks and decisions deterministically now, but let general entities be duplicated and merged later. ← chosen
- Strict when exact match exists — Reuse nodes only on strong deterministic matches; otherwise create new nodes for later merging.
- Prefer insertion over reuse — Keep canonicalization minimal in early v1 and accept graph duplication as part of iteration.

### If the retry with tighter constraints still fails to produce valid Grafeo inserts, what is the right final fallback?

- Keep episode hidden — Do not surface the memory at all until a later dreaming pass successfully builds the graph.
- Surface warning and skip memory — Warn that Lobster is degraded and skip that episode from recall for now.
- **Store for later reprocessing** — Persist the episode in a pending state for a later retry pipeline, but keep it out of recall until then. ← chosen

## Round 23

### What should `memory_context` assemble by default in v1?

- **Top ranked mixed hits** — Return the best ranked decisions, summaries, tasks, and entities for the current situation. ← chosen
- Task-centered bundle — Build a compact packet around the current task, including its key decisions, related entities, and recent episodes.
- Decision-first bundle — Prioritize constraints, decisions, and rationale first, with supporting graph context second.

### What should `memory_decisions` optimize for in v1?

- Exact decision lookup — Best for finding a specific prior choice or constraint with minimal extra context.
- **Decision timeline** — Show how decisions evolved over time for a task or repo. ← chosen
- Decision clusters — Group related decisions by task, concept, or component to reveal patterns.

### When should pending episodes be retried for graph extraction after a failed inline pass?

- **Next idle moment** — Retry soon in the background as soon as the system is idle. ← chosen
- End-of-session dreaming — Only retry during the later dreaming/synthesis phase.
- On explicit maintenance command — Keep retries manual or admin-triggered in v1.

### When should general entity merges happen if duplicates are allowed during ingestion?

- **During dreaming only** — Let later background synthesis handle entity deduplication and merge proposals. ← chosen
- On explicit graph maintenance runs — Keep merging as a deliberate maintenance action, not an automatic background behavior.
- Whenever evidence becomes strong — Allow automatic merges later if deterministic evidence reaches a clear threshold.
