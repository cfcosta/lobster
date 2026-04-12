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

    let db_path = lobster::app::config::db_path(storage_dir);
    let db = lobster::store::db::open(&db_path).context("open database")?;
    let grafeo = lobster::graph::db::new_in_memory();

    // Read hook payload from stdin
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .context("read stdin")?;

    // Parse the hook event
    let event: lobster::hooks::events::HookEvent =
        serde_json::from_str(&input).context("parse hook event")?;

    tracing::debug!(hook_type = ?event.hook_type, "hook invoked");

    // Run the recall pipeline
    let payload = lobster::hooks::recall::run_recall(&event, &db, &grafeo);

    // Output recall payload as JSON to stdout
    let json =
        serde_json::to_string(&payload).context("serialize recall payload")?;
    println!("{json}");

    Ok(())
}

#[allow(clippy::unused_async)]
async fn cmd_mcp(storage_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    let db_path = lobster::app::config::db_path(storage_dir);
    let db = lobster::store::db::open(&db_path).context("open database")?;
    let grafeo = lobster::graph::db::new_in_memory();

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
