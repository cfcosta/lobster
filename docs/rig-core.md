# rig-core

An opinionated Rust library for building LLM-powered applications. Provides a unified,
ergonomic interface over 20+ LLM providers and 10+ vector store backends, covering the
full spectrum from simple one-shot prompts to complex RAG pipelines with dynamic tool
selection.

The crate is published as `rig-core` but the Rust library name is `rig` (i.e., you write
`use rig::...` in code).

- **Repository**: <https://github.com/0xPlaygrounds/rig>
- **Crates.io**: <https://crates.io/crates/rig-core>
- **Documentation**: <https://docs.rs/rig-core/latest/rig/>
- **Guides**: <https://docs.rig.rs>

## Adding to your project

```toml
[dependencies]
rig-core = "0.34.0"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Optional features:

| Feature              | What it enables                                              |
| -------------------- | ------------------------------------------------------------ |
| `derive`             | `#[derive(Embed)]` and `#[rig::tool_macro]` attribute macros |
| `pdf`                | PDF file loading via `lopdf`                                 |
| `epub`               | EPUB file loading                                            |
| `rmcp`               | MCP (Model Context Protocol) tool integration                |
| `audio`              | Audio generation model support                               |
| `image`              | Image generation model support                               |
| `discord-bot`        | Discord bot integration via serenity                         |
| `rayon`              | Parallel processing                                          |
| `wasm`               | WebAssembly target support                                   |
| `websocket`          | OpenAI Responses API websocket mode                          |
| `reqwest-middleware` | Middleware support for HTTP client                           |

```toml
rig-core = { version = "0.34.0", features = ["derive"] }
```

## Key concepts

### Provider clients

Every provider has a `Client` type. The client is the entry point for creating models,
agents, and extractors. All providers support `from_env()` to read API keys from
environment variables.

```rust
use rig::providers::openai;
use rig::client::{ProviderClient, CompletionClient};

// From environment variable (OPENAI_API_KEY)
let client = openai::Client::from_env();

// From explicit key
let client = openai::Client::new("sk-...").unwrap();

// With builder for custom configuration
let client = openai::Client::builder()
    .api_key("sk-...")
    .base_url("https://custom-endpoint.example.com")
    .build()
    .unwrap();
```

Built-in providers (in `rig::providers`): `openai`, `anthropic`, `gemini`, `cohere`,
`mistral`, `ollama`, `deepseek`, `groq`, `openrouter`, `huggingface`, `azure`,
`hyperbolic`, `together`, `voyageai`, `xai`, `galadriel`, `llamafile`, `mira`,
`moonshot`, `perplexity`.

**Note**: The `openai` provider has two client types: `openai::Client` (Responses API,
the default) and `openai::CompletionsClient` (traditional Chat Completions API). Use the
latter if you need the classic completions endpoint.

Model constants:

```rust
// OpenAI
openai::GPT_4O                          // "gpt-4o"
openai::GPT_4O_MINI                     // "gpt-4o-mini"
openai::O3                              // "o3"
openai::TEXT_EMBEDDING_3_LARGE          // "text-embedding-3-large"
openai::TEXT_EMBEDDING_ADA_002          // "text-embedding-ada-002"

// Anthropic
anthropic::completion::CLAUDE_OPUS_4_6   // "claude-opus-4-6"
anthropic::completion::CLAUDE_SONNET_4_6 // "claude-sonnet-4-6"
anthropic::completion::CLAUDE_HAIKU_4_5  // "claude-haiku-4-5"
```

### Core traits

- **`Prompt`** -- simplest interface: prompt in, string out.
- **`Chat`** -- prompt + chat history in, string out.
- **`Completion`** -- low-level, returns a `CompletionRequestBuilder`.
- **`TypedPrompt`** -- structured output, returns a deserialized `T`.
- **`StreamingPrompt`** / **`StreamingChat`** -- streaming variants.

### Agent

The central high-level abstraction. Combines a model with a system prompt, static
context, dynamic context (RAG), tools, and configuration. Implements all of the above
traits.

```rust
let agent = client
    .agent(openai::GPT_4O)
    .preamble("You are a helpful assistant.")
    .temperature(0.7)
    .max_tokens(2048)
    .build();
```

## Basic usage

### Simple prompt

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O)
        .preamble("You are a comedian here to entertain the user.")
        .build();

    let response = agent.prompt("Entertain me!").await?;
    println!("{response}");
    Ok(())
}
```

### Using Anthropic

```rust
use rig::providers::anthropic;

let client = anthropic::Client::from_env(); // reads ANTHROPIC_API_KEY
let agent = client
    .agent(anthropic::completion::CLAUDE_SONNET_4_6)
    .preamble("You are a helpful assistant.")
    .build();

let response = agent.prompt("Hello!").await?;
```

### Using Ollama (local models)

```rust
use rig::providers::ollama;

let client = ollama::Client::from_env();
let agent = client.agent("llama3").preamble("You are helpful.").build();
```

### Chat with history

```rust
use rig::completion::Chat;
use rig::message::Message;

let history = vec![
    Message::user("Tell me a joke!"),
    Message::assistant("Why did the chicken cross the road? To get to the other side!"),
];

let response = agent.chat("Tell me another one!", &history).await?;
```

## Tools

### Defining a tool

```rust
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
struct AddArgs { x: i32, y: i32 }

#[derive(Debug, thiserror::Error)]
#[error("math error")]
struct MathError;

#[derive(Deserialize, Serialize)]
struct Add;

impl Tool for Add {
    const NAME: &'static str = "add";
    type Error = MathError;
    type Args = AddArgs;
    type Output = i32;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "add".to_string(),
            description: "Add x and y together".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "First number" },
                    "y": { "type": "number", "description": "Second number" }
                },
                "required": ["x", "y"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(args.x + args.y)
    }
}
```

### Agent with tools

```rust
let agent = client
    .agent(openai::GPT_4O)
    .preamble("You are a calculator. Use the provided tools.")
    .tool(Add)
    .build();

// .max_turns(20) allows up to 20 tool-calling round trips
let result = agent.prompt("What is 5 + 3?").max_turns(20).await?;
```

### Agent nesting

Agents implement `Tool`, so you can nest them:

```rust
let calculator = client.agent(openai::GPT_4O)
    .preamble("You are a calculator.")
    .tool(Add)
    .tool(Subtract)
    .build();

let meta_agent = client.agent(openai::GPT_4O)
    .preamble("You are a helpful assistant.")
    .tool(calculator)  // calculator agent used as a tool
    .build();
```

### ToolSet for dynamic tool selection

```rust
use rig::tool::ToolSet;

let toolset = ToolSet::builder()
    .static_tool(Add)
    .static_tool(Subtract)
    .dynamic_tool(Multiply)   // must implement ToolEmbedding
    .dynamic_tool(Divide)
    .build();
```

## Embeddings

### Embedding model

```rust
use rig::client::EmbeddingsClient;

let embedding_model = client.embedding_model(openai::TEXT_EMBEDDING_ADA_002);
```

### The Embed trait

Marks which fields of a struct should be embedded. With the `derive` feature:

```rust
use rig::Embed;
use serde::Serialize;

#[derive(Embed, Serialize, Clone, Debug, Eq, PartialEq, Default)]
struct WordDefinition {
    id: String,
    word: String,
    #[embed]
    definitions: Vec<String>,  // this field will be embedded
}
```

### Building embeddings

```rust
use rig::embeddings::EmbeddingsBuilder;

let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
    .documents(vec![doc1, doc2, doc3])?
    .build()
    .await?;
// Returns Vec<(T, OneOrMany<Embedding>)>
```

For simple strings:

```rust
let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
    .document("A flurbo is a green alien.")?
    .document("A glarb-glarb is a fictional dance.")?
    .build()
    .await?;
```

## Vector stores

### In-memory vector store (built-in)

```rust
use rig::vector_store::in_memory_store::InMemoryVectorStore;

let vector_store = InMemoryVectorStore::from_documents(embeddings);
let index = vector_store.index(embedding_model);
```

With custom IDs:

```rust
let vector_store = InMemoryVectorStore::from_documents_with_id_f(
    embeddings,
    |doc| doc.id.clone(),
);
```

### External vector stores

Available as separate crates: `rig-mongodb`, `rig-lancedb`, `rig-qdrant`, `rig-sqlite`,
`rig-neo4j`, `rig-surrealdb`, `rig-milvus`, `rig-scylladb`, `rig-s3vectors`,
`rig-helixdb`, `rig-postgres`.

### Vector search

```rust
use rig::vector_store::VectorSearchRequest;

let req = VectorSearchRequest::builder()
    .query("What is a flurbo?")
    .samples(3)
    .build()?;

let results = index.top_n::<WordDefinition>(req).await?;
// Returns Vec<(f64, String, WordDefinition)> -- (score, id, document)
```

`VectorStoreIndex` also implements `Tool`, so you can register a vector store directly
as a tool on an agent.

## RAG (Retrieval-Augmented Generation)

### Agent with dynamic context

```rust
let agent = client.agent(openai::GPT_4O)
    .preamble("You are a dictionary assistant. Use the context below to answer.")
    .dynamic_context(2, index)  // retrieve top 2 documents per query
    .build();

let response = agent.prompt("What is a flurbo?").await?;
```

### Dynamic tool selection via RAG

When you have many tools, use RAG to select the most relevant ones per query:

```rust
let agent = client.agent(openai::GPT_4O)
    .preamble("You are a calculator.")
    .dynamic_tools(2, index, toolset)  // select top 2 relevant tools per query
    .build();
```

### Pipeline API for custom RAG

```rust
use rig::pipeline::{self, Op};
use rig::parallel;

let chain = pipeline::new()
    .chain(parallel!(
        passthrough::<&str>(),
        lookup::<_, _, String>(index, 1),
    ))
    .map(|(prompt, maybe_docs)| match maybe_docs {
        Ok(docs) => format!("Context:\n{}\n\nQuery: {}", docs[0].2, prompt),
        Err(_) => prompt.to_string(),
    })
    .prompt(agent);

let response = chain.call("What does glarb-glarb mean?").await?;
```

## Structured extraction

Extract structured data from text by having the LLM fill a schema:

```rust
use schemars::JsonSchema;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct Person {
    first_name: Option<String>,
    last_name: Option<String>,
    job: Option<String>,
}

let extractor = client.extractor::<Person>(openai::GPT_4).build();

let person = extractor.extract("John Doe is a software engineer.").await?;
// Person { first_name: Some("John"), last_name: Some("Doe"), job: Some("software engineer") }
```

With usage tracking:

```rust
let response = extractor.extract_with_usage("Jane Smith is a data scientist.").await?;
println!("Data: {:?}", response.data);
println!("Tokens: {}", response.usage.total_tokens);
```

## Structured output (TypedPrompt)

Request typed, schema-constrained responses from an agent:

```rust
use rig::completion::TypedPrompt;
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
struct WeatherForecast {
    city: String,
    temperature_f: f64,
    conditions: String,
}

let agent = client.agent(openai::GPT_4O).build();

let forecast: WeatherForecast = agent
    .prompt_typed("What's the weather in NYC?")
    .max_turns(3)
    .await?;
```

## Streaming

```rust
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::streaming::StreamingPrompt;

let mut stream = agent.stream_prompt("Tell me a joke").await;

while let Some(item) = stream.next().await {
    match item? {
        MultiTurnStreamItem::StreamAssistantItem(content) => {
            // Handles text chunks, tool calls, reasoning, etc.
            // Variants: StreamedAssistantContent::Text, ToolCall,
            //           ToolCallDelta, Reasoning, ReasoningDelta, Final
        }
        MultiTurnStreamItem::StreamUserItem(content) => {
            // Handles tool results
            // Variants: StreamedUserContent::ToolResult { .. }
        }
        MultiTurnStreamItem::FinalResponse(response) => {
            println!("Done: {}", response.response());
        }
    }
}
```

Multi-turn streaming with tools:

```rust
let mut stream = agent
    .stream_prompt("Calculate 5 + 3")
    .multi_turn(10)
    .await;
```

## Prompt hooks

Hooks observe and control the agent loop. Implement `PromptHook<M>` to intercept events:

```rust
use rig::agent::{PromptHook, HookAction, ToolCallHookAction};
use rig::completion::{CompletionModel, CompletionResponse, Message};

#[derive(Clone)]
struct LoggingHook;

impl<M: CompletionModel> PromptHook<M> for LoggingHook {
    async fn on_completion_call(&self, prompt: &Message, _history: &[Message]) -> HookAction {
        println!("Sending prompt...");
        HookAction::cont()
    }

    async fn on_completion_response(
        &self,
        _prompt: &Message,
        response: &CompletionResponse<M::Response>,
    ) -> HookAction {
        println!("Got response: {:?}", response.choice);
        HookAction::cont()
    }

    async fn on_tool_call(
        &self,
        tool_name: &str,
        _id: Option<String>,
        _internal_id: &str,
        args: &str,
    ) -> ToolCallHookAction {
        println!("Calling tool: {} with args: {}", tool_name, args);
        ToolCallHookAction::cont()
        // Or: ToolCallHookAction::skip("reason")
        // Or: ToolCallHookAction::terminate("reason")
    }

    async fn on_tool_result(
        &self,
        tool_name: &str,
        _id: Option<String>,
        _internal_id: &str,
        _args: &str,
        result: &str,
    ) -> HookAction {
        println!("Tool {} returned: {}", tool_name, result);
        HookAction::cont()
    }
}

// Set as default on agent
let agent = client.agent(openai::GPT_4O)
    .hook(LoggingHook)
    .build();

// Or per-request
let response = agent.prompt("Hello")
    .with_hook(LoggingHook)
    .await?;
```

## PromptRequest builder

When you call `agent.prompt("...")`, it returns a `PromptRequest` builder:

```rust
let response = agent
    .prompt("Calculate 5 - 2")
    .max_turns(20)              // allow up to 20 tool-calling round trips
    .with_history(chat_history) // pass conversation history
    .with_hook(my_hook)         // per-request event hook
    .with_tool_concurrency(4)   // run tools in parallel
    .extended_details()         // return PromptResponse with usage info
    .await?;
```

Without `.extended_details()`, returns `String`. With it, returns a `PromptResponse`
with fields: `output: String` (the response text), `usage: Usage`, and
`messages: Option<Vec<Message>>` (full message history).

## AgentBuilder reference

```rust
client.agent(model_id)
    .name("my-agent")                        // name (for logging, tool identity)
    .description("A helpful agent")          // description (when used as a tool)
    .preamble("System prompt")               // system prompt
    .append_preamble("Additional context")   // append to system prompt
    .context("Static context document")      // add static context
    .dynamic_context(2, vector_index)        // RAG context (top-n docs)
    .tool(my_tool)                           // add a tool
    .tools(vec_of_boxed_tools)               // add multiple boxed tools
    .dynamic_tools(2, index, toolset)        // RAG-selected tools
    .temperature(0.7)                        // temperature
    .max_tokens(2048)                        // max tokens
    .additional_params(json)                 // provider-specific params
    .tool_choice(ToolChoice::Required)       // force tool use
    .default_max_turns(10)                   // default multi-turn depth
    .hook(my_hook)                           // default prompt hook
    .output_schema::<T>()                    // structured output schema
    .build()
```

## Common patterns

1. **`from_env()` for all providers**: Reads API keys from environment variables
   (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.).

2. **Builder pattern everywhere**: Agents, extractors, embeddings, vector search requests.

3. **`use rig::prelude::*`**: Brings `CompletionClient`, `EmbeddingsClient`,
   `ProviderClient`, `TypedPrompt` into scope.

4. **`.max_turns(n)` for tool use**: Without it, default is 0 (one-shot). Set it when
   your agent has tools.

5. **`ToolDyn` for heterogeneous collections**: `Vec<Box<dyn ToolDyn>>` for dynamically
   constructed tool lists.

6. **`tracing` integration**: The library uses the `tracing` crate. Initialize with
   `tracing_subscriber::fmt()` for logging.

7. **Agent nesting**: Agents implement `Tool`, enabling hierarchical agent architectures.

## Role in Lobster

In Lobster's architecture, rig-core is available as the LLM abstraction layer. It could
be used for:

- Summarization workers (via agents with appropriate system prompts)
- Extraction workers (via the `Extractor` API for structured fact extraction)
- Any LLM-powered processing step in the episode finalization pipeline

The swappable `Summarizer` and `Extractor` traits in Lobster's architecture align with
rig's provider-agnostic design -- different backends (OpenAI, Anthropic, local Ollama)
can be swapped by changing the provider client without altering the pipeline logic.
