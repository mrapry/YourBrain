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
    /// Override the embedding provider (`local` | `onnx`) for this invocation.
    /// Needed to access a database that was reindexed to a non-default embedder.
    #[arg(long = "embedder", global = true)]
    embedder: Option<String>,
    /// Override the embedding model key (e.g. `multilingual-e5-small`).
    #[arg(long = "embed-model", global = true)]
    embed_model: Option<String>,
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
        /// Token budget; 0 = use [token_budget]/[recall] config.
        #[arg(long, default_value_t = 0)]
        budget: usize,
        #[arg(long)]
        room: Option<String>,
        /// Condense recalled memories to fit the budget (dynamic token budgeter).
        #[arg(long = "dynamic-budget")]
        dynamic_budget: bool,
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
    /// Validate a drafted answer against the knowledge base (anti-hallucination).
    Validate {
        /// The drafted answer to fact-check.
        answer: String,
        /// Optional query used to gather evidence (defaults to the answer).
        #[arg(long)]
        query: Option<String>,
    },
    /// Semantic cache operations: get | put | clear.
    Cache {
        /// get | put | clear
        action: String,
        /// The query (for get/put).
        #[arg(long)]
        query: Option<String>,
        /// The answer to cache (for put).
        #[arg(long)]
        answer: Option<String>,
        /// Override Tier-1 similarity threshold for `get` (research/tuning).
        #[arg(long)]
        threshold: Option<f32>,
        /// KB memory ids that grounded the answer (for put; repeatable) — enables
        /// auto-invalidation when those memories change.
        #[arg(long = "source-id")]
        source_ids: Vec<String>,
    },
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
    /// Re-embed all memories & rebuild the vector index (migrate embedder).
    Reindex {
        /// Embedding provider: `local` (hash) or `onnx` (sentence-transformer).
        #[arg(long)]
        provider: Option<String>,
        /// Model key, e.g. `multilingual-e5-small`, `all-minilm-l6-v2`.
        #[arg(long)]
        model: Option<String>,
        /// Directory to cache downloaded ONNX models.
        #[arg(long = "cache-dir")]
        cache_dir: Option<String>,
        /// Actually perform the migration (otherwise it's a dry-run preview).
        #[arg(long)]
        yes: bool,
    },
    /// Start the MCP server (stdio) for IDE integration.
    Mcp {
        /// Server-wide default for the dynamic token budgeter (per project via
        /// its own mcp.json). Omit to use the [token_budget] config.
        #[arg(long = "dynamic-budget")]
        dynamic_budget: Option<bool>,
        /// Server-wide default token budget; 0 = use config.
        #[arg(long, default_value_t = 0)]
        budget: usize,
        /// Override [cache] similarity_threshold (Tier 1). Omit = use config.
        #[arg(long = "cache-similarity")]
        cache_similarity: Option<f32>,
        /// Override [cache] kb_direct_threshold (Tier 2). Omit = use config.
        #[arg(long = "cache-kb-direct")]
        cache_kb_direct: Option<f32>,
        /// Override [cache] kb_grounding_threshold (Tier 3). Omit = use config.
        #[arg(long = "cache-kb-grounding")]
        cache_kb_grounding: Option<f32>,
        /// Override embedding provider for this server: `local` | `onnx`.
        #[arg(long = "embedder")]
        embedder: Option<String>,
        /// Override embedding model key (e.g. `multilingual-e5-small`).
        #[arg(long = "embed-model")]
        embed_model: Option<String>,
        /// Override [conflict] similarity_threshold. Raise to ~0.75 with ONNX.
        #[arg(long = "conflict-similarity")]
        conflict_similarity: Option<f32>,
    },
    /// Handle a hook event; reads a JSON payload from stdin (ADR-9).
    Hook { event: String },
    /// Install IDE integration (cursor | claude-code).
    Install {
        #[arg(long)]
        ide: String,
        /// Write `--dynamic-budget true` into the generated mcp.json so this
        /// project always uses the dynamic token budgeter.
        #[arg(long = "dynamic-budget")]
        dynamic_budget: bool,
        /// Write a fixed `--budget N` into the generated mcp.json (0 = omit).
        #[arg(long, default_value_t = 0)]
        budget: usize,
        /// Write `--cache-similarity <f>` into mcp.json (Tier 1 threshold).
        #[arg(long = "cache-similarity")]
        cache_similarity: Option<f32>,
        /// Write `--cache-kb-direct <f>` into mcp.json (Tier 2 threshold).
        #[arg(long = "cache-kb-direct")]
        cache_kb_direct: Option<f32>,
        /// Write `--cache-kb-grounding <f>` into mcp.json (Tier 3 threshold).
        #[arg(long = "cache-kb-grounding")]
        cache_kb_grounding: Option<f32>,
        /// Write `--embedder <local|onnx>` into mcp.json for this project.
        #[arg(long = "embedder")]
        embedder: Option<String>,
        /// Write `--embed-model <key>` into mcp.json for this project.
        #[arg(long = "embed-model")]
        embed_model: Option<String>,
        /// Write `--conflict-similarity <f>` into mcp.json ([conflict] threshold).
        #[arg(long = "conflict-similarity")]
        conflict_similarity: Option<f32>,
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
    context::set_embedder_override(cli.embedder.clone(), cli.embed_model.clone());
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
            dynamic_budget,
        } => cli::recall(&query, limit, &detail, budget, room, dynamic_budget),
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
        Command::Validate { answer, query } => cli::validate(&answer, query),
        Command::Cache {
            action,
            query,
            answer,
            threshold,
            source_ids,
        } => cli::cache(&action, query, answer, threshold, source_ids),
        Command::Config { cmd } => match cmd {
            ConfigCmd::Show => cli::config_show(),
        },
        Command::Export { scope, out } => cli::export(scope, out),
        Command::Import { file } => cli::import(&file),
        Command::Reindex {
            provider,
            model,
            cache_dir,
            yes,
        } => cli::reindex(provider, model, cache_dir, yes),
        Command::Mcp {
            dynamic_budget,
            budget,
            cache_similarity,
            cache_kb_direct,
            cache_kb_grounding,
            embedder,
            embed_model,
            conflict_similarity,
        } => mcp::run(
            cli.db_memory,
            dynamic_budget,
            budget,
            yb_core::brain::CacheOverrides {
                similarity: cache_similarity,
                kb_direct: cache_kb_direct,
                kb_grounding: cache_kb_grounding,
            },
            mcp::EmbedderOverride {
                provider: embedder,
                model: embed_model,
            },
            conflict_similarity,
        ),
        Command::Hook { event } => hook::run(&event),
        Command::Install {
            ide,
            dynamic_budget,
            budget,
            cache_similarity,
            cache_kb_direct,
            cache_kb_grounding,
            embedder,
            embed_model,
            conflict_similarity,
        } => install::run(
            &ide,
            dynamic_budget,
            budget,
            cache_similarity,
            cache_kb_direct,
            cache_kb_grounding,
            embedder,
            embed_model,
            conflict_similarity,
        ),
    }
}
