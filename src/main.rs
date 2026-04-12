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

    // Stage the event to the filesystem. The MCP server (the sole
    // owner of the redb database) will watch the staging directory
    // and ingest this event.
    if let Err(e) = lobster::store::staging::stage_event(storage_dir, &input) {
        tracing::warn!(error = %e, "failed to stage event");
    }

    // Standalone fallback: if MCP is not running, try to open the
    // database non-blocking. If we get the lock, ingest staged
    // events and run recall. If locked, MCP will handle it.
    let db_path = lobster::app::config::db_path(storage_dir);
    let output = if let Some(db) = lobster::store::db::try_open(&db_path) {
        tracing::debug!("standalone mode: acquired DB lock");

        // Ingest all pending staged events
        let grafeo = lobster::graph::db::new_in_memory();
        if let Err(e) = lobster::graph::rebuild::rebuild_from_redb(&db, &grafeo)
        {
            tracing::warn!(error = %e, "failed to rebuild Grafeo");
        }
        lobster::graph::indexes::ensure_indexes(&grafeo);

        lobster::store::ingest::ingest_staged(storage_dir, &db, &grafeo).await;

        // Run recall on the current event
        let payload = lobster::hooks::recall::run_recall(&event, &db, &grafeo);

        if payload.items.is_empty() {
            lobster::hooks::events::HookOutput::empty()
        } else {
            let hint = lobster::hooks::tiered::format_hint(&payload);
            if hint.is_empty() {
                lobster::hooks::events::HookOutput::empty()
            } else {
                lobster::hooks::events::HookOutput::with_message(hint)
            }
        }
    } else {
        tracing::debug!(
            "MCP server has DB lock or DB not initialized; \
             event staged for later ingestion"
        );
        lobster::hooks::events::HookOutput::empty()
    };

    let json =
        serde_json::to_string(&output).context("serialize hook output")?;
    println!("{json}");

    Ok(())
}

#[allow(clippy::unused_async)]
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
        let watch_result =
            lobster::store::watcher::watch_staging(&storage_dir_owned);
        let (mut rx, _guard) = match watch_result {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "failed to start staging watcher, \
                     falling back to polling"
                );
                // Fall back to polling every 2 seconds
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    lobster::store::ingest::ingest_staged(
                        &storage_dir_owned,
                        &ingest_db,
                        &ingest_grafeo,
                    )
                    .await;
                }
            }
        };

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
    let _dreaming = tokio::spawn(async move {
        let config = lobster::dream::scheduler::DreamConfig::default();
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
        }
    });

    tracing::info!("MCP server starting on stdio");
    eprintln!("lobster: MCP server ready (JSON-RPC on stdio)");

    lobster::mcp::server::run_server(&db, &grafeo)
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))
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

    let db_path = lobster::app::config::db_path(storage_dir);
    let _db =
        lobster::store::db::open(&db_path).context("initialize database")?;

    println!("Lobster initialized at {}", storage_dir.display());
    println!("Database: {}", db_path.display());
    println!();

    // Generate Claude Code hook configuration
    let hook_json = lobster::app::hooks_config::to_json("lobster")
        .context("generate hook config")?;
    println!("Add this to .claude/settings.json hooks:");
    println!("{hook_json}");

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
