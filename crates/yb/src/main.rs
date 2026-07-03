//! `yb` — YourBrain single binary.
//!
//! Subcommands cover CLI operations, the MCP server (`yb mcp`), hook handling
//! (`yb hook`), and IDE installation (`yb install`).

mod cli;
mod context;
mod hook;
mod install;
mod mcp;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "yb",
    version,
    about = "YourBrain — AI memory engine with conflict resolution"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Isolate data into a named memory database under the data dir
    /// (`dbs/<name>`). When omitted, the shared/global database is used.
    #[arg(long = "db-memory", global = true)]
    db_memory: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Store a new memory (with conflict check).
    Remember {
        /// The memory content.
        content: String,
        /// Comma-free repeated tags: --tag a --tag b.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        room: Option<String>,
        /// personal | team
        #[arg(long)]
        scope: Option<String>,
    },
    /// Search and retrieve memories.
    Recall {
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// headline | summary | full
        #[arg(long, default_value = "summary")]
        detail: String,
        #[arg(long, default_value_t = 200)]
        budget: usize,
        #[arg(long)]
        room: Option<String>,
    },
    /// Resolve a pending conflict.
    Resolve {
        conflict_id: String,
        /// replace | keep_both | discard_new | merge
        #[arg(long)]
        action: String,
        #[arg(long)]
        context: Option<String>,
        /// Merged content (required for --action merge).
        #[arg(long = "content")]
        merged: Option<String>,
    },
    /// List memories.
    List {
        #[arg(long)]
        room: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show full details of a memory.
    Get { id: String },
    /// Archive a memory.
    Forget { id: String },
    /// Endorse a memory as still valid.
    Endorse {
        id: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Flag a memory as potentially wrong.
    Dispute {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// List pending conflicts.
    Conflicts,
    /// Show a memory's audit history.
    Timeline {
        memory_id: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Show statistics and health info.
    Stats,
    /// Configuration commands.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Export memories to JSONL (stdout or --out file).
    Export {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        out: Option<String>,
    },
    /// Import memories from a JSONL file.
    Import { file: String },
    /// Start the MCP server (stdio) for IDE integration.
    Mcp,
    /// Handle a hook event; reads a JSON payload from stdin (ADR-9).
    Hook { event: String },
    /// Install IDE integration (cursor | claude-code).
    Install {
        #[arg(long)]
        ide: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show current config and paths.
    Show,
}

fn main() {
    // `yb mcp` speaks JSON-RPC on stdout; keep logs on stderr only.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("YB_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    // CLI subcommands read this default when opening the brain; `yb mcp` also
    // takes it as its server default (while still allowing per-call overrides).
    context::set_default_db(cli.db_memory.clone());
    match cli.command {
        Command::Remember {
            content,
            tags,
            room,
            scope,
        } => cli::remember(&content, tags, room, scope),
        Command::Recall {
            query,
            limit,
            detail,
            budget,
            room,
        } => cli::recall(&query, limit, &detail, budget, room),
        Command::Resolve {
            conflict_id,
            action,
            context,
            merged,
        } => cli::resolve(&conflict_id, &action, context, merged),
        Command::List { room, limit } => cli::list(room, limit),
        Command::Get { id } => cli::get(&id),
        Command::Forget { id } => cli::forget(&id),
        Command::Endorse { id, author } => cli::endorse(&id, author),
        Command::Dispute { id, reason } => cli::dispute(&id, &reason),
        Command::Conflicts => cli::conflicts(),
        Command::Timeline { memory_id, limit } => cli::timeline(&memory_id, limit),
        Command::Stats => cli::stats(),
        Command::Config { cmd } => match cmd {
            ConfigCmd::Show => cli::config_show(),
        },
        Command::Export { scope, out } => cli::export(scope, out),
        Command::Import { file } => cli::import(&file),
        Command::Mcp => mcp::run(cli.db_memory),
        Command::Hook { event } => hook::run(&event),
        Command::Install { ide } => install::run(&ide),
    }
}
