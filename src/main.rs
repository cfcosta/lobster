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
    hook_type: &str,
) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    let db_path = lobster::app::config::db_path(storage_dir);
    let _db = lobster::store::db::open(&db_path).context("open database")?;

    tracing::debug!(hook_type, "hook invoked");

    // Read hook payload from stdin
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .context("read stdin")?;

    // Parse the hook event
    let _event: lobster::hooks::events::HookEvent =
        serde_json::from_str(&input).context("parse hook event")?;

    // TODO: capture event, run segmentation, run recall
    // For now, output empty JSON (no recall hints)
    println!("{{}}");

    Ok(())
}

#[allow(clippy::unused_async)]
async fn cmd_mcp(storage_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(storage_dir).context("create storage dir")?;

    let db_path = lobster::app::config::db_path(storage_dir);
    let _db = lobster::store::db::open(&db_path).context("open database")?;

    tracing::info!("MCP server starting on stdio");

    // TODO: run JSON-RPC server on stdin/stdout
    eprintln!("lobster: MCP server not yet implemented");

    Ok(())
}

fn cmd_status(storage_dir: &std::path::Path) -> Result<()> {
    let db_path = lobster::app::config::db_path(storage_dir);

    if !db_path.exists() {
        println!("Lobster is not initialized for this repository.");
        println!("Run `lobster init` to set up memory tracking.");
        return Ok(());
    }

    let _db = lobster::store::db::open(&db_path).context("open database")?;

    // TODO: query episode counts, processing states
    println!("Lobster status: initialized");
    println!("Storage: {}", storage_dir.display());

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
    // TODO: download and install PyLate model
    anyhow::bail!("model install not yet implemented")
}
