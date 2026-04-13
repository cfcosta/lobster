use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// Lobster: local, deterministic, per-repo memory system for
/// Claude Code.
#[derive(Parser)]
#[command(name = "lobster", version, about)]
struct Cli {
    /// Path to the repository (defaults to current directory).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// One-shot hook execution: capture event, run recall, exit.
    Hook {
        /// Hook type (`UserPromptSubmit`, `PostToolUse`, etc.)
        hook_type: String,
    },

    /// Long-lived MCP server for memory tools.
    Mcp,

    /// Show Lobster status for this repository.
    Status,

    /// Reset all memory for this repository.
    Reset {
        /// Skip confirmation prompt.
        #[arg(long)]
        force: bool,
    },

    /// Initialize Lobster for a repository.
    Init,

    /// Install or update the embedding model.
    Install,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (stderr only — stdout reserved for
    // hook output and MCP JSON-RPC)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let repo_dir = cli
        .repo
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let storage_dir = lobster::app::config::resolve_storage_path(&repo_dir);

    match cli.command {
        Command::Hook { hook_type } => cmd_hook(&storage_dir, &hook_type).await,
        Command::Mcp => cmd_mcp(&storage_dir).await,
        Command::Status => cmd_status(&storage_dir),
        Command::Reset { force } => cmd_reset(&storage_dir, force),
        Command::Init => cmd_init(&storage_dir),
        Command::Install => cmd_install(&storage_dir),
    }
}

#[allow(clippy::unused_async)]
async fn cmd_hook(
    storage_dir: &std::path::Path,
    _hook_type: &str,
) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    // Read hook payload from stdin
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .context("read stdin")?;

    // Validate it's parseable JSON before staging
    let event: lobster::hooks::events::HookEvent =
        serde_json::from_str(&input).context("parse hook event")?;

    tracing::debug!(hook_type = event.hook_type(), "hook invoked");

    // Stage the event to the filesystem. The MCP server is the sole
    // owner of the redb database and will watch the staging directory
    // via inotify to ingest events as they arrive.
    //
    // Hooks NEVER open redb directly — they use a snapshot copy to
    // avoid lock contention with the MCP server.
    if let Err(e) = lobster::store::staging::stage_event(storage_dir, &input) {
        tracing::warn!(error = %e, "failed to stage event");
    }

    // Run automatic recall for prompt events. Opens a snapshot copy
    // of the database (never the live file) so we don't block the
    // MCP server. Fails open: if the DB is unavailable, return empty.
    let output = try_hook_recall(storage_dir, &event);

    let json =
        serde_json::to_string(&output).context("serialize hook output")?;
    println!("{json}");

    Ok(())
}

/// Attempt automatic recall for a hook event.
///
/// Opens a read-only snapshot of the database, rebuilds Grafeo, runs
/// the recall pipeline, and formats the result. Returns empty output
/// if the database is unavailable or no relevant memories are found.
fn try_hook_recall(
    storage_dir: &std::path::Path,
    event: &lobster::hooks::events::HookEvent,
) -> lobster::hooks::events::HookOutput {
    use lobster::hooks::{
        events::HookOutput,
        recall::run_recall,
        tiered::{OutputTier, classify_tier, format_hint},
    };

    // Only run recall for events that produce queries
    let Some(query) = lobster::hooks::recall::construct_query(event) else {
        tracing::debug!("hook recall: no query for this event");
        return HookOutput::empty();
    };
    tracing::debug!(query = %query, "hook recall: constructed query");

    let db_path = lobster::app::config::db_path(storage_dir);
    let Some(db) = lobster::store::db::open_snapshot(&db_path) else {
        tracing::debug!(path = %db_path.display(), "hook recall: snapshot unavailable");
        return HookOutput::empty();
    };
    tracing::debug!("hook recall: snapshot opened");

    // Rebuild Grafeo from the snapshot for retrieval
    let grafeo = lobster::graph::db::new_in_memory();
    if let Err(e) = lobster::graph::rebuild::rebuild_from_redb(&db, &grafeo) {
        tracing::debug!(error = %e, "hook recall: failed to rebuild grafeo");
        return HookOutput::empty();
    }
    lobster::graph::indexes::ensure_indexes(&grafeo);
    tracing::debug!(
        nodes = grafeo.node_count(),
        edges = grafeo.edge_count(),
        "hook recall: grafeo rebuilt"
    );

    let payload = run_recall(event, &db, &grafeo);
    let tier = classify_tier(&payload);
    tracing::debug!(
        items = payload.items.len(),
        ?tier,
        latency_ms = payload.latency_ms,
        "hook recall: result"
    );

    // Load core memory (always-injected, query-independent),
    // deduplicating against recall results to avoid repeating
    // decisions that were already surfaced by query matching.
    let core_items = lobster::hooks::core_memory::load_core_memory(&db);
    let deduped_core = lobster::hooks::core_memory::dedup_against_recall(
        &core_items,
        &payload,
    );
    let core_text =
        lobster::hooks::core_memory::format_core_memory(&deduped_core);

    match tier {
        OutputTier::Silent if core_text.is_empty() => HookOutput::empty(),
        OutputTier::Silent => {
            // Even with no query-matched recall, inject core memory
            HookOutput::with_message(core_text)
        }
        OutputTier::Hint | OutputTier::Structured => {
            let recall_text = format_hint(&payload);
            let message = if core_text.is_empty() {
                recall_text
            } else if recall_text.is_empty() {
                core_text
            } else {
                format!("{core_text}\n\n{recall_text}")
            };
            if message.is_empty() {
                HookOutput::empty()
            } else {
                HookOutput::with_message(message)
            }
        }
    }
}

#[allow(clippy::unused_async, clippy::too_many_lines)]
async fn cmd_mcp(storage_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    let db_path = lobster::app::config::db_path(storage_dir);
    let db = std::sync::Arc::new(
        lobster::store::db::open(&db_path).context("open database")?,
    );

    // Spawn the write coordinator for serialized writes
    let (write_handle, _coordinator) =
        lobster::store::coordinator::spawn(db.clone(), 64);
    // write_handle is available for tools that need to write
    let _ = write_handle;

    // Rebuild Grafeo from redb for the MCP session.
    // Wrap in Arc for sharing with background tasks (GrafeoDB uses
    // interior mutability and is Send+Sync).
    let grafeo = std::sync::Arc::new(lobster::graph::db::new_in_memory());
    if let Err(e) = lobster::graph::rebuild::rebuild_from_redb(&db, &grafeo) {
        tracing::warn!(error = %e, "failed to rebuild Grafeo");
    }
    lobster::graph::indexes::ensure_indexes(&grafeo);

    // Ingest any events that were staged while MCP was not running
    let initial =
        lobster::store::ingest::ingest_staged(storage_dir, &db, &grafeo).await;
    if initial.events_ingested > 0 {
        tracing::info!(
            events = initial.events_ingested,
            "ingested pre-existing staged events"
        );
        // Rebuild Grafeo after initial ingestion
        if let Err(e) = lobster::graph::rebuild::rebuild_from_redb(&db, &grafeo)
        {
            tracing::warn!(error = %e, "failed to rebuild Grafeo after ingestion");
        }
    }

    // Start watching the staging directory for new events from hooks
    let storage_dir_owned = storage_dir.to_path_buf();
    let ingest_db = db.clone();
    let ingest_grafeo = grafeo.clone();
    let _ingestion = tokio::spawn(async move {
        let (mut rx, _guard) =
            lobster::store::watcher::watch_staging(&storage_dir_owned)
                .expect("failed to start staging watcher");

        tracing::info!("staging watcher started");

        // Debounce: wait a short time after notification to batch
        // multiple rapid file creates into one ingestion cycle.
        loop {
            // Wait for a notification
            if rx.recv().await.is_none() {
                tracing::warn!("staging watcher channel closed");
                break;
            }

            // Debounce: drain any additional notifications that
            // arrived in the next 50ms
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            while rx.try_recv().is_ok() {}

            // Run ingestion
            lobster::store::ingest::ingest_staged(
                &storage_dir_owned,
                &ingest_db,
                &ingest_grafeo,
            )
            .await;
        }
    });

    // Spawn dreaming scheduler in a background task.
    // Per spec: "dreaming belongs to the long-lived process"
    let dream_db = db.clone();
    let dream_grafeo = grafeo.clone();
    let _dreaming = tokio::spawn(async move {
        let config = lobster::dream::scheduler::DreamConfig::default();
        let pattern_config = lobster::dream::patterns::PatternConfig::default();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let result =
                lobster::dream::scheduler::run_cycle(&dream_db, &config);
            if result.retries_attempted > 0 {
                tracing::info!(
                    retries = result.retries_attempted,
                    succeeded = result.retries_succeeded,
                    failed = result.episodes_failed_final,
                    "dreaming cycle completed"
                );
            }

            // Run workflow pattern mining
            let wf_result = lobster::dream::workers::scan_workflow_patterns(
                &dream_db,
                &dream_grafeo,
                &pattern_config,
            );
            if wf_result.workflows_created > 0
                || wf_result.workflows_updated > 0
            {
                tracing::info!(
                    scanned = wf_result.episodes_scanned,
                    created = wf_result.workflows_created,
                    updated = wf_result.workflows_updated,
                    "workflow mining completed"
                );
            }

            // Run decision supersession detection
            let ss_config =
                lobster::dream::supersession::SupersessionConfig::default();
            let ss_result =
                lobster::dream::supersession::scan_superseded_decisions(
                    &dream_db, &ss_config,
                );
            if ss_result.decisions_superseded > 0 {
                tracing::info!(
                    scanned = ss_result.decisions_scanned,
                    superseded = ss_result.decisions_superseded,
                    "decision supersession completed"
                );
            }
        }
    });

    tracing::info!("MCP server starting on stdio");

    lobster::mcp::server::run_server(db, grafeo).await
}

fn cmd_status(storage_dir: &std::path::Path) -> Result<()> {
    let db_path = lobster::app::config::db_path(storage_dir);

    if !db_path.exists() {
        println!("Lobster is not initialized for this repository.");
        println!("Run `lobster init` to set up memory tracking.");
        return Ok(());
    }

    let db = lobster::store::db::open(&db_path).context("open database")?;

    println!("Lobster status: initialized");
    println!("Storage: {}", storage_dir.display());
    println!();

    let report = lobster::app::status::scan(&db);
    print!("{report}");

    Ok(())
}

fn cmd_reset(storage_dir: &std::path::Path, force: bool) -> Result<()> {
    if !storage_dir.exists() {
        println!("Nothing to reset — Lobster is not initialized.");
        return Ok(());
    }

    if !force {
        eprint!(
            "This will delete all Lobster memory for this repo. Continue? [y/N] "
        );
        let mut answer = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer)
            .context("read confirmation")?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let db_path = lobster::app::config::db_path(storage_dir);
    if db_path.exists() {
        std::fs::remove_file(&db_path).context("remove database")?;
    }
    println!("Memory reset complete.");

    Ok(())
}

fn cmd_init(storage_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    // 1. Initialize the database
    let db_path = lobster::app::config::db_path(storage_dir);
    let _db =
        lobster::store::db::open(&db_path).context("initialize database")?;
    println!("Database: {}", db_path.display());

    // 2. Resolve the binary path (current exe)
    let bin_path = std::env::current_exe()
        .context("resolve binary path")?
        .to_string_lossy()
        .to_string();

    // 3. Write .claude/settings.json with hook configuration
    let repo_root = storage_dir
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let claude_dir = repo_root.join(".claude");
    std::fs::create_dir_all(&claude_dir).context("create .claude dir")?;

    let settings_path = claude_dir.join("settings.json");
    let settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{bin_path} hook UserPromptSubmit"),
                    "timeout": 10
                }]
            }],
            "PostToolUse": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{bin_path} hook PostToolUse"),
                    "timeout": 10
                }]
            }]
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)
            .context("serialize settings")?,
    )
    .context("write .claude/settings.json")?;
    println!("Hooks: {}", settings_path.display());

    // 4. Write .mcp.json with MCP server configuration
    let mcp_path = repo_root.join(".mcp.json");
    let mcp = serde_json::json!({
        "mcpServers": {
            "lobster": {
                "command": bin_path,
                "args": ["mcp"]
            }
        }
    });
    std::fs::write(
        &mcp_path,
        serde_json::to_string_pretty(&mcp).context("serialize mcp config")?,
    )
    .context("write .mcp.json")?;
    println!("MCP:   {}", mcp_path.display());

    // 5. Add .lobster/ to .gitignore if not already there
    let gitignore_path = repo_root.join(".gitignore");
    let needs_entry = if gitignore_path.exists() {
        let contents = std::fs::read_to_string(&gitignore_path)
            .context("read .gitignore")?;
        !contents.lines().any(|l| l.trim() == ".lobster/")
    } else {
        true
    };
    if needs_entry {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .context("open .gitignore")?;
        writeln!(f, ".lobster/").context("append to .gitignore")?;
    }

    println!("\nLobster initialized. Restart Claude Code to activate.");
    Ok(())
}

fn cmd_install(_storage_dir: &std::path::Path) -> Result<()> {
    use candle_core::Device;
    use pylate_rs::ColBERT;

    println!("Downloading GTE-ModernColBERT-v1 from HuggingFace...");

    let _model: ColBERT = ColBERT::from("lightonai/GTE-ModernColBERT-v1")
        .with_device(Device::Cpu)
        .try_into()
        .context("failed to download/load ColBERT model")?;

    println!("Model installed successfully (CPU backend).");
    Ok(())
}
