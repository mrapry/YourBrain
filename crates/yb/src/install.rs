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
/// project name can be detected.
fn mcp_args() -> Vec<String> {
    let mut args = vec!["mcp".to_string()];
    if let Some(name) = detect_project_name() {
        args.push("--db-memory".to_string());
        args.push(name);
    }
    args
}

pub fn run(ide: &str) -> Result<()> {
    match ide {
        "cursor" => install_cursor(),
        "claude-code" | "claude" => install_claude_code(),
        other => anyhow::bail!("unsupported IDE `{other}` (supported: cursor, claude-code)"),
    }
}

fn install_cursor() -> Result<()> {
    let cmd = yb_command();
    let args = mcp_args();
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
";
    append_or_create(Path::new(".cursorrules"), cursorrules)?;

    println!("Installed Cursor integration:");
    println!("  .cursor/mcp.json  (MCP server `yourbrain`)");
    println!("  .cursorrules      (recall/remember guidance)");
    print_db_memory_note(&args);
    println!("\nNote: Cursor is MCP-only (no hooks). Recall is AI-initiated — see ADR-4.");
    Ok(())
}

fn install_claude_code() -> Result<()> {
    let cmd = yb_command();
    let args = mcp_args();
    let mcp = json!({
        "mcpServers": {
            "yourbrain": { "command": cmd, "args": args }
        }
    });
    write_json(Path::new(".mcp.json"), &mcp)?;

    let settings = json!({
        "hooks": {
            "SessionStart": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook session_start") } ] } ],
            "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook prompt_submit") } ] } ],
            "PostToolUse": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook tool_use") } ] } ],
            "Stop": [ { "hooks": [ { "type": "command", "command": format!("{cmd} hook session_end") } ] } ]
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

/// Report whether memories are isolated into a named database or shared globally.
fn print_db_memory_note(args: &[String]) {
    match args.iter().position(|a| a == "--db-memory") {
        Some(i) if i + 1 < args.len() => {
            println!("\nMemory isolated into db_memory `{}`.", args[i + 1]);
            println!("  Edit .cursor/mcp.json (or .mcp.json) to change or remove it for the global database.");
        }
        _ => println!("\nUsing the shared/global memory database (no db_memory detected)."),
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
