//! `yb install --ide <name>`: generate IDE integration files.
//!
//! Writes project-local config into the current working directory:
//! - Cursor: `.cursor/mcp.json` (query via MCP) + `.cursorrules` (recall guidance).
//! - Claude Code: `.mcp.json` + `.claude/settings.json` hooks (auto-capture).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use crate::context;

/// The absolute path to the current `yb` executable, for embedding in configs.
fn yb_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "yb".to_string())
}

/// Best-effort project name for the isolated `db_memory`.
///
/// Prefers the git repository root's folder name, falling back to the current
/// directory name. The result is sanitized into a safe database name so each
/// project's memories stay isolated by default (still editable in the generated
/// config).
fn detect_project_name() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join(".git").exists() {
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                if let Some(safe) = context::sanitize_db_name(name) {
                    return Some(safe);
                }
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    cwd.file_name()
        .and_then(|s| s.to_str())
        .and_then(context::sanitize_db_name)
}

/// Build the `yb mcp` argument list, isolating into `--db-memory <name>` when a
/// project name can be detected, and optionally pinning this project's dynamic
/// token budgeter and budget (written into the generated mcp.json).
/// Per-project cache-threshold overrides written into the generated mcp.json.
#[derive(Clone, Copy, Default)]
pub struct CacheFlags {
    pub similarity: Option<f32>,
    pub kb_direct: Option<f32>,
    pub kb_grounding: Option<f32>,
}

/// Per-project embedder override written into the generated mcp.json.
#[derive(Clone, Default)]
pub struct EmbedFlags {
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Override [conflict] similarity_threshold for this server.
    pub conflict_similarity: Option<f32>,
}

fn mcp_args(
    dynamic_budget: bool,
    budget: usize,
    cache: CacheFlags,
    embed: &EmbedFlags,
) -> Vec<String> {
    let mut args = vec!["mcp".to_string()];
    if let Some(name) = detect_project_name() {
        args.push("--db-memory".to_string());
        args.push(name);
    }
    if dynamic_budget {
        args.push("--dynamic-budget".to_string());
        args.push("true".to_string());
    }
    if budget > 0 {
        args.push("--budget".to_string());
        args.push(budget.to_string());
    }
    if let Some(v) = cache.similarity {
        args.push("--cache-similarity".to_string());
        args.push(v.to_string());
    }
    if let Some(v) = cache.kb_direct {
        args.push("--cache-kb-direct".to_string());
        args.push(v.to_string());
    }
    if let Some(v) = cache.kb_grounding {
        args.push("--cache-kb-grounding".to_string());
        args.push(v.to_string());
    }
    if let Some(p) = &embed.provider {
        args.push("--embedder".to_string());
        args.push(p.clone());
    }
    if let Some(m) = &embed.model {
        args.push("--embed-model".to_string());
        args.push(m.clone());
    }
    if let Some(v) = embed.conflict_similarity {
        args.push("--conflict-similarity".to_string());
        args.push(v.to_string());
    }
    args
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    ide: &str,
    dynamic_budget: bool,
    budget: usize,
    cache_similarity: Option<f32>,
    cache_kb_direct: Option<f32>,
    cache_kb_grounding: Option<f32>,
    embedder: Option<String>,
    embed_model: Option<String>,
    conflict_similarity: Option<f32>,
) -> Result<()> {
    let cache = CacheFlags {
        similarity: cache_similarity,
        kb_direct: cache_kb_direct,
        kb_grounding: cache_kb_grounding,
    };
    let embed = EmbedFlags {
        provider: embedder,
        model: embed_model,
        conflict_similarity,
    };
    match ide {
        "cursor" => install_cursor(dynamic_budget, budget, cache, &embed),
        "claude-code" | "claude" => install_claude_code(dynamic_budget, budget, cache, &embed),
        other => anyhow::bail!("unsupported IDE `{other}` (supported: cursor, claude-code)"),
    }
}

fn install_cursor(
    dynamic_budget: bool,
    budget: usize,
    cache: CacheFlags,
    embed: &EmbedFlags,
) -> Result<()> {
    let cmd = yb_command();
    let args = mcp_args(dynamic_budget, budget, cache, embed);
    let mcp = json!({
        "mcpServers": {
            "yourbrain": { "command": cmd, "args": args }
        }
    });
    write_json(Path::new(".cursor/mcp.json"), &mcp)?;

    let cursorrules = "\
# YourBrain memory
Before answering questions about this project, call the `yb_recall` tool with a
relevant query to load prior context. When the user establishes a durable fact,
decision, or preference, call `yb_remember` to persist it. If `yb_remember`
returns a conflict, present the options to the user and call `yb_resolve`.

For repeat or clearly answerable questions, call `yb_cache_get` FIRST: reuse a
`hit` answer directly, or use `grounding` memories as context before answering.
After composing a fresh answer worth reusing, store it with `yb_cache_put`,
passing the `source_ids` of the memories that grounded it. Before presenting an
important factual answer, call `yb_validate` and revise any unsupported claims.
";
    append_or_create(Path::new(".cursorrules"), cursorrules)?;

    println!("Installed Cursor integration:");
    println!("  .cursor/mcp.json  (MCP server `yourbrain`)");
    println!("  .cursorrules      (recall/remember/cache/validate guidance)");
    print_db_memory_note(&args);
    println!("\nNote: Cursor is MCP-only (no hooks). Recall is AI-initiated — see ADR-4.");
    Ok(())
}

fn install_claude_code(
    dynamic_budget: bool,
    budget: usize,
    cache: CacheFlags,
    embed: &EmbedFlags,
) -> Result<()> {
    let cmd = yb_command();
    let args = mcp_args(dynamic_budget, budget, cache, embed);
    let mcp = json!({
        "mcpServers": {
            "yourbrain": { "command": cmd, "args": args }
        }
    });
    write_json(Path::new(".mcp.json"), &mcp)?;

    // Hooks must open the same database + embedder as the MCP server, otherwise
    // capturing an observation fails (wrong db, or a vector-dimension mismatch
    // against the embedder the db is locked to). Read-time knobs are irrelevant.
    let h = hook_flags(&args);
    let settings = json!({
        "hooks": {
            "SessionStart": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook session_start{h}") } ] } ],
            "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook prompt_submit{h}") } ] } ],
            "PostToolUse": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook tool_use{h}") } ] } ],
            "Stop": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook session_end{h}") } ] } ]
        }
    });
    write_json(Path::new(".claude/settings.json"), &settings)?;

    #[cfg(windows)]
    {
        if which_sh().is_none() {
            println!("Warning: Claude Code runs hooks via `sh` on Windows.");
            println!(
                "  Install Git for Windows and add Git\\bin to PATH (winget install Git.Git)."
            );
        }
    }

    println!("Installed Claude Code integration:");
    println!("  .mcp.json               (MCP server `yourbrain`)");
    println!("  .claude/settings.json   (auto-capture hooks)");
    print_db_memory_note(&args);
    Ok(())
}

/// The `yb hook` flag suffix that mirrors the MCP server's database + embedder.
///
/// Hooks record observations into the same isolated `--db-memory`, and must use
/// the same `--embedder`/`--embed-model` so opening the brain doesn't fail the
/// embedding-dimension lock (ADR-5). Budget/cache/conflict knobs only affect the
/// MCP read path, so they're deliberately left out. Returns a leading-space
/// string (e.g. ` --db-memory foo --embedder onnx`) ready to append after the
/// hook event; empty when the global database with the default embedder is used.
fn hook_flags(args: &[String]) -> String {
    let mut out = String::new();
    for flag in ["--db-memory", "--embedder", "--embed-model"] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            if let Some(v) = args.get(i + 1) {
                out.push_str(&format!(" {flag} {v}"));
            }
        }
    }
    out
}

#[cfg(windows)]
fn which_sh() -> Option<()> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.join("sh.exe").exists() {
            return Some(());
        }
    }
    None
}

/// Report whether memories are isolated into a named database or shared globally,
/// plus any per-project token-budget pinned into the generated config.
fn print_db_memory_note(args: &[String]) {
    match args.iter().position(|a| a == "--db-memory") {
        Some(i) if i + 1 < args.len() => {
            println!("\nMemory isolated into db_memory `{}`.", args[i + 1]);
            println!("  Edit .cursor/mcp.json (or .mcp.json) to change or remove it for the global database.");
        }
        _ => println!("\nUsing the shared/global memory database (no db_memory detected)."),
    }
    if let Some(i) = args.iter().position(|a| a == "--dynamic-budget") {
        let v = args.get(i + 1).map(String::as_str).unwrap_or("true");
        println!("Dynamic token budgeter pinned for this project: {v}.");
    }
    if let Some(i) = args.iter().position(|a| a == "--budget") {
        if let Some(n) = args.get(i + 1) {
            println!("Token budget pinned for this project: {n}.");
        }
    }
    for (flag, label) in [
        ("--cache-similarity", "cache Tier-1 similarity"),
        ("--cache-kb-direct", "cache Tier-2 kb_direct"),
        ("--cache-kb-grounding", "cache Tier-3 kb_grounding"),
    ] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            if let Some(v) = args.get(i + 1) {
                println!("Pinned {label} threshold: {v}.");
            }
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--embedder") {
        if let Some(v) = args.get(i + 1) {
            println!("Embedder pinned for this project: {v}.");
            println!("Reminder: run `yb reindex --provider {v} --yes` once before using it.");
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--conflict-similarity") {
        if let Some(v) = args.get(i + 1) {
            println!("Pinned conflict similarity threshold: {v}.");
        }
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn append_or_create(path: &Path, contents: &str) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains("YourBrain memory") {
        return Ok(()); // already present
    }
    let combined = if existing.is_empty() {
        contents.to_string()
    } else {
        format!("{existing}\n{contents}")
    };
    fs::write(path, combined).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
