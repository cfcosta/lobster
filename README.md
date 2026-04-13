# Lobster

Local, deterministic, per-repo memory system for Claude Code.

## What it does

Lobster gives Claude Code persistent memory. When you make decisions, discuss architecture, or solve problems, Lobster captures them. The next time you (or Claude) return to the repo, prior decisions, constraints, and context are automatically surfaced.

It works through two channels:

- **Hooks** run on every Claude Code interaction, capturing events and injecting tiny high-confidence recall hints (decisions, summaries) directly into the conversation.
- **MCP tools** give Claude deeper, on-demand access to search memory, browse decision timelines, traverse the knowledge graph, and assemble context bundles.

## Install

### Nix

```bash
# CPU-only (default)
nix profile install github:cfcosta/lobster

# NVIDIA GPU (CUDA acceleration for ColBERT embeddings)
nix profile install github:cfcosta/lobster#lobster-cuda

# Apple GPU (Metal acceleration for ColBERT embeddings)
nix profile install github:cfcosta/lobster#lobster-metal
```

### Cargo

Requires Rust nightly (edition 2024).

```bash
# CPU-only (default)
cargo install --git https://github.com/cfcosta/lobster

# NVIDIA GPU
cargo install --git https://github.com/cfcosta/lobster --features cuda
```

## Setup

### 1. Set an LLM API key

Lobster uses an LLM for summarization and entity extraction. Set one of these:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
# or
export OPENAI_API_KEY=sk-...
```

If neither is set, Lobster still captures and stores events, but summarization and extraction are skipped.

### 2. Initialize Lobster for your repo

```bash
cd /path/to/your/repo
lobster init
```

This command:

1. Creates a `.lobster/` directory in your repo root with a `lobster.redb` database.
2. Creates (or merges into) `.claude/settings.json` with hook entries for `UserPromptSubmit` and `PostToolUse`.
3. Creates (or merges into) `.mcp.json` with the Lobster MCP server configuration.
4. Adds `.lobster/` to `.gitignore` if not already present.

Existing settings in `.claude/settings.json` and `.mcp.json` are preserved -- Lobster only adds its own entries and never overwrites other hooks or MCP servers. Running `lobster init` multiple times is safe (idempotent).

After init, **restart Claude Code** to activate the hooks and MCP server.

### 3. (Optional) Install the ColBERT embedding model

```bash
lobster install
```

Downloads the [GTE-ModernColBERT-v1](https://huggingface.co/lightonai/GTE-ModernColBERT-v1) model from HuggingFace for vector search and reranking. The model is loaded on CPU by default. This is optional -- Lobster works without it using BM25 text search only.

## How it works

Once initialized, Lobster operates automatically:

1. **Hooks** fire on every Claude Code interaction (`UserPromptSubmit`, `PostToolUse`). The hook process writes the event to a staging directory (`.lobster/staging/`) as a JSON file and exits. This is lock-free and fast.

2. **The MCP server** (`lobster mcp`, started by Claude Code via `.mcp.json`) runs as a long-lived process. It watches the staging directory via inotify and ingests events into the redb database.

3. **Ingestion** processes raw events into episodes using idle gaps and repo transitions, then:
   - Produces episode summaries via LLM
   - Detects decisions from explicit choice language, constraints, and non-goals
   - Extracts entities and relations into a semantic graph
   - Generates embeddings for vector search (if the ColBERT model is installed)

4. **Automatic recall** runs during hook execution. On each `UserPromptSubmit`, Lobster opens a read-only snapshot of the database, searches for relevant memories, and injects 1-3 high-confidence items as a `systemMessage` hint. This includes:
   - **Core memory**: the top 3 highest-confidence, still-valid decisions are always injected regardless of query.
   - **Query-matched recall**: additional decisions, summaries, or entities matched against the user's prompt.

5. **Background dreaming** runs every 60 seconds in the MCP server process, performing maintenance: retrying failed extractions, mining workflow patterns, and detecting superseded decisions.

## Generated configuration

### `.claude/settings.json`

Lobster adds two hook entries:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/lobster hook UserPromptSubmit",
            "timeout": 10
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/lobster hook PostToolUse",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

### `.mcp.json`

Lobster registers itself as an MCP server:

```json
{
  "mcpServers": {
    "lobster": {
      "command": "/path/to/lobster",
      "args": ["mcp"]
    }
  }
}
```

## Commands

### `lobster init`

Initialize Lobster memory for a repository. Creates `.lobster/`, sets up hooks and MCP config, and adds `.lobster/` to `.gitignore`.

Refuses to run in your home directory to avoid overwriting global Claude Code configuration.

```
lobster init
```

### `lobster hook <type>`

Process a Claude Code hook event. Called automatically by the hooks configured in `.claude/settings.json`. Reads the hook payload from stdin, stages the event, runs automatic recall, and prints a JSON response to stdout.

Supported hook types: `UserPromptSubmit`, `PostToolUse`, `PreToolUse`, `Stop`.

```
lobster hook UserPromptSubmit
```

### `lobster mcp`

Start the MCP server on stdio. This is a long-lived process started by Claude Code via the `.mcp.json` configuration. It:

- Opens and owns the redb database
- Watches the staging directory for new events from hooks
- Runs the ingestion pipeline (segmentation, summarization, extraction)
- Exposes memory tools via MCP
- Runs background dreaming (maintenance, retries, pattern mining)

```
lobster mcp
```

### `lobster status`

Show the current state of Lobster memory for this repository.

```
lobster status
```

Example output:

```
Lobster status: initialized
Storage: /path/to/repo/.lobster

Episodes: 42 total
  Ready:        38
  Pending:      2
  RetryQueued:  1
  FailedFinal:  1
Artifacts:
  Summaries:    38
  Extractions:  36
```

### `lobster reset`

Delete all Lobster memory for this repository. Prompts for confirmation unless `--force` is passed.

```
lobster reset          # interactive confirmation
lobster reset --force  # skip confirmation
```

### `lobster install`

Download the ColBERT embedding model (GTE-ModernColBERT-v1) from HuggingFace. The model is used for vector search and reranking. Downloads to the HuggingFace cache directory (`~/.cache/huggingface/`).

```
lobster install
```

### Global flag: `--repo <path>`

All commands accept `--repo` to specify the repository path. Defaults to the current directory.

```
lobster --repo /path/to/repo status
```

## MCP Tools

When the MCP server is running, Claude Code has access to these tools:

| Tool               | Parameters        | Description                                                                                                       |
| ------------------ | ----------------- | ----------------------------------------------------------------------------------------------------------------- |
| `memory_context`   | `query: string`   | Task-oriented context bundle: returns ranked decisions, summaries, tasks, and entities for the current situation. |
| `memory_search`    | `query: string`   | Search memory for ranked hits with snippets and confidence scores.                                                |
| `memory_recent`    | _(none)_          | List the newest ready artifacts (episodes, decisions, tasks). Up to 20 items, newest first.                       |
| `memory_decisions` | _(none)_          | Return decision timeline with rationale, sorted newest first.                                                     |
| `memory_neighbors` | `node_id: string` | Graph neighbor traversal from a given entity node. Filters on temporal validity (excludes expired edges).         |
| `memory_status`    | _(none)_          | Processing state diagnostics: episode counts, artifacts, pending/failed status, workflow count.                   |

## Environment Variables

| Variable            | Required     | Default             | Description                                                    |
| ------------------- | ------------ | ------------------- | -------------------------------------------------------------- |
| `ANTHROPIC_API_KEY` | One of these | --                  | Anthropic API key for summarization and extraction             |
| `OPENAI_API_KEY`    | required     | --                  | OpenAI API key (alternative to Anthropic)                      |
| `ANTHROPIC_MODEL`   | No           | `claude-sonnet-4-6` | Anthropic model to use for LLM calls                           |
| `OPENAI_MODEL`      | No           | `gpt-5.4-mini`      | OpenAI model to use for LLM calls                              |
| `RUST_LOG`          | No           | _(none)_            | Tracing filter (e.g., `lobster=debug`). All logs go to stderr. |

If neither API key is set, Lobster still captures events and provides retrieval over previously-processed artifacts, but new episodes will not be summarized or have entities extracted.

## Data storage

All data lives in `.lobster/` inside your repository root:

```
.lobster/
  lobster.redb          # redb database (events, episodes, decisions, artifacts)
  lobster.redb.snapshot  # temporary read-only copy used by hooks
  staging/              # event files written by hooks, consumed by MCP server
```

Lobster walks up from the current directory looking for an existing `.lobster/` directory, so you can run commands from subdirectories.

The redb database is the canonical source of truth. The semantic graph (Grafeo) is rebuilt in-memory from redb on each MCP server start and hook recall invocation. If the database is lost, `lobster reset --force` followed by `lobster init` recreates it (but prior memory is gone).

## Architecture

- **redb** -- canonical source of truth (events, episodes, decisions, artifacts). ACID, crash-safe, embedded.
- **Grafeo** -- semantic serving layer rebuilt in-memory from redb. Provides graph facts, BM25 text search, and hybrid retrieval.
- **pylate-rs** -- local ColBERT embedding inference (optional). Used for vector reranking.
- **rig-core** -- LLM adapter for summarization and extraction only. Supports Anthropic and OpenAI.

Hooks never open the live database directly -- they write to the staging directory and read from a snapshot copy, avoiding lock contention with the MCP server.

See `ARCHITECTURE.md` for the full system design.

## Failure behavior

Lobster is designed to fail open:

- If the database is locked or unavailable, hooks return empty output and Claude Code continues normally.
- If no API key is set, events are still captured but summarization/extraction is skipped.
- If embedding model is not installed, retrieval falls back to BM25 text search.
- If recall exceeds its 500ms latency budget, it returns nothing rather than blocking.
- Failed extractions are retried by the background dreaming process.

The memory system never blocks normal coding flow.
