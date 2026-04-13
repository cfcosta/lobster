# Lobster

Local, deterministic, per-repo memory system for Claude Code.

## What it does

Lobster gives Claude Code persistent memory. When you make decisions, discuss architecture, or solve problems, Lobster captures them. The next time you (or Claude) return to the repo, prior decisions, constraints, and context are automatically surfaced.

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

```bash
# CPU-only (default)
cargo install --git https://github.com/cfcosta/lobster

# NVIDIA GPU
cargo install --git https://github.com/cfcosta/lobster --features cuda
```

## Setup

```bash
# 1. Set an LLM API key (required for summarization + extraction)
export ANTHROPIC_API_KEY=sk-ant-...
# or
export OPENAI_API_KEY=sk-...

# Optional: override default models
export ANTHROPIC_MODEL=claude-sonnet-4-6   # default
export OPENAI_MODEL=gpt-5.4-mini            # default

# 2. Initialize Lobster for your repo
cd /path/to/your/repo
lobster init

# 3. (Optional) Install the ColBERT embedding model for vector search
lobster install
```

The `init` command prints hook configuration to add to `.claude/settings.json`.

## Commands

| Command                 | Description                                             |
| ----------------------- | ------------------------------------------------------- |
| `lobster init`          | Initialize memory for a repo                            |
| `lobster hook <type>`   | Process a Claude Code hook event (called automatically) |
| `lobster mcp`           | Start the MCP server for deep recall tools              |
| `lobster status`        | Show episode/artifact counts                            |
| `lobster reset --force` | Delete all memory for this repo                         |
| `lobster install`       | Download the ColBERT embedding model                    |

## How it works

1. **Hooks** capture every Claude Code interaction as raw events in redb
2. **Episodes** are segmented from events using idle gaps and repo transitions
3. **Summarization** produces episode summaries via LLM (Claude or GPT)
4. **Decision detection** finds "I chose X", "non-goal", "must not" patterns
5. **Extraction** uses LLM to extract entities and relations into a graph
6. **Retrieval** uses BM25 text search + cosine reranking to find relevant memories
7. **Recall** surfaces 1-3 high-confidence items as `systemMessage` hints

## MCP Tools

When running `lobster mcp`, these tools are available:

- `memory_recent` — newest ready artifacts
- `memory_search` — ranked hits with snippets and confidence
- `memory_decisions` — decision timeline with rationale
- `memory_neighbors` — graph neighbor traversal (temporally filtered)
- `memory_context` — task-oriented context bundle
- `memory_status` — processing state diagnostics

## Environment Variables

| Variable            | Required     | Default             | Description       |
| ------------------- | ------------ | ------------------- | ----------------- |
| `ANTHROPIC_API_KEY` | One of these | —                   | Anthropic API key |
| `OPENAI_API_KEY`    | required     | —                   | OpenAI API key    |
| `ANTHROPIC_MODEL`   | No           | `claude-sonnet-4-6` | Anthropic model   |
| `OPENAI_MODEL`      | No           | `gpt-5.4-mini`       | OpenAI model      |

## Architecture

- **redb** — canonical source of truth (events, episodes, decisions, artifacts)
- **Grafeo** — semantic serving layer (graph, BM25 text search)
- **pylate-rs** — local ColBERT embedding (optional, for vector reranking)
- **rig-core** — LLM adapter for summarization and extraction

See `ARCHITECTURE.md` for the full system design.
