//! MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Exposes the YourBrain tools to IDEs such as Cursor and Claude Code. Runs the
//! engine in-process. Each request line is parsed, dispatched to the [`Brain`],
//! and answered with a single response line.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{json, Value};
use yb_core::brain::RememberOptions;
use yb_core::conflict::ResolutionAction;
use yb_core::search::DetailLevel;
use yb_core::{Brain, Config};

use crate::context;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Lazily-opened set of brains keyed by resolved `db_memory` name.
///
/// The server keeps one brain per database so a session can target multiple
/// isolated memories. The `default_db` (from `yb mcp --db-memory <name>`) is used
/// whenever a tool call omits its own `db_memory` argument. The empty-string key
/// denotes the shared/global database.
struct BrainPool {
    default_db: Option<String>,
    config: Config,
    brains: HashMap<String, Brain>,
}

impl BrainPool {
    fn new(default_db: Option<String>) -> Result<Self> {
        Ok(Self {
            default_db,
            config: context::load_config()?,
            brains: HashMap::new(),
        })
    }

    /// Get (opening on first use) the brain for a tool call's `db_memory`,
    /// falling back to the server default and then the global database.
    fn get(&mut self, db_memory: Option<&str>) -> Result<&mut Brain> {
        let effective: Option<String> = db_memory
            .map(str::to_string)
            .or_else(|| self.default_db.clone());
        let key = effective.clone().unwrap_or_default();
        if !self.brains.contains_key(&key) {
            let dir = context::resolve_db_dir(effective.as_deref())?;
            let brain = Brain::open(&dir, self.config.clone())?;
            self.brains.insert(key.clone(), brain);
        }
        Ok(self.brains.get_mut(&key).expect("brain just inserted"))
    }

    fn save_all(&self) -> Result<()> {
        for brain in self.brains.values() {
            brain.save()?;
        }
        Ok(())
    }
}

/// Run the MCP stdio loop until stdin closes.
///
/// `default_db` is the server-wide `db_memory` from `yb mcp --db-memory <name>`;
/// individual tool calls may override it with their own `db_memory` argument.
pub fn run(default_db: Option<String>) -> Result<()> {
    let mut pool = BrainPool::new(default_db)?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications (no id) get no response.
        if id.is_none() {
            continue;
        }

        let response = match method {
            "initialize" => ok(id, initialize_result()),
            "tools/list" => ok(id, json!({ "tools": tool_specs() })),
            "tools/call" => match handle_tool_call(&mut pool, &req) {
                Ok(result) => ok(id, result),
                Err(e) => tool_error(id, &e.to_string()),
            },
            "ping" => ok(id, json!({})),
            _ => err(id, -32601, "method not found"),
        };

        writeln!(out, "{response}")?;
        out.flush()?;
    }
    pool.save_all()?;
    Ok(())
}

fn ok(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A `tools/call` that failed at the tool level (not the protocol level) is
/// reported via `isError` so the model can react.
fn tool_error(id: Option<Value>, message: &str) -> Value {
    ok(
        id,
        json!({
            "content": [{ "type": "text", "text": format!("Error: {message}") }],
            "isError": true
        }),
    )
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "yourbrain", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn handle_tool_call(pool: &mut BrainPool, req: &Value) -> Result<Value> {
    let params = req.get("params").cloned().unwrap_or(json!({}));
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Any tool may target a specific database via `db_memory`; otherwise the
    // server default (then the global database) is used.
    let db_memory = args.get("db_memory").and_then(|v| v.as_str());
    let brain = pool.get(db_memory)?;

    match name {
        "yb_remember" => tool_remember(brain, &args),
        "yb_recall" => tool_recall(brain, &args),
        "yb_resolve" => tool_resolve(brain, &args),
        "yb_timeline" => tool_timeline(brain, &args),
        "yb_get_full" => tool_get_full(brain, &args),
        "yb_endorse" => tool_endorse(brain, &args),
        "yb_dispute" => tool_dispute(brain, &args),
        "yb_stats" => tool_stats(brain),
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

fn tool_remember(brain: &mut Brain, args: &Value) -> Result<Value> {
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    if content.is_empty() {
        anyhow::bail!("`content` is required");
    }
    let mut opts = RememberOptions::default();
    if let Some(room) = args.get("room").and_then(|v| v.as_str()) {
        opts.room = Some(room.to_string());
    }
    if let Some(tags) = args.get("tags").and_then(|v| v.as_array()) {
        opts.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
    }

    use yb_core::brain::RememberOutcome;
    let text = match brain.remember(content, opts)? {
        RememberOutcome::Stored { id } => {
            format!("{{\"status\":\"stored\",\"id\":\"{id}\"}}")
        }
        RememberOutcome::AutoResolved {
            id,
            action,
            relation,
        } => json!({
            "status": "auto_resolved",
            "relation": relation.as_str(),
            "action": format!("{action:?}"),
            "id": id
        })
        .to_string(),
        RememberOutcome::NeedsReview {
            conflict_id,
            analysis,
            existing,
        } => json!({
            "status": "conflict",
            "conflict_id": conflict_id,
            "relation": analysis.relation.as_str(),
            "confidence": analysis.confidence,
            "reasoning": analysis.reasoning,
            "existing": existing.iter().map(|m| json!({
                "id": m.id, "content": m.content, "author": m.author
            })).collect::<Vec<_>>(),
            "hint": "Ask the user to choose, then call yb_resolve with the conflict_id."
        })
        .to_string(),
    };
    brain.save()?;
    Ok(text_result(text))
}

fn tool_recall(brain: &mut Brain, args: &Value) -> Result<Value> {
    let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
    if query.is_empty() {
        anyhow::bail!("`query` is required");
    }
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    let budget = args
        .get("token_budget")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;
    let detail = DetailLevel::parse(
        args.get("detail")
            .and_then(|v| v.as_str())
            .unwrap_or("summary"),
    );
    let room = args.get("room").and_then(|v| v.as_str());

    let res = brain.recall(query, limit, detail, budget, room)?;
    let mut text = format!(
        "# YB Recall ({} memories, {} tokens)\n",
        res.output.ids.len(),
        res.output.tokens_used
    );
    for line in &res.output.lines {
        text.push_str(line);
        text.push('\n');
    }
    if res.output.lines.is_empty() {
        text.push_str("(no relevant memories found)\n");
    }
    Ok(text_result(text))
}

fn tool_resolve(brain: &mut Brain, args: &Value) -> Result<Value> {
    let conflict_id = args
        .get("conflict_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let action = parse_action(args.get("action").and_then(|v| v.as_str()).unwrap_or(""))?;
    let context = args
        .get("context")
        .and_then(|v| v.as_str())
        .map(String::from);
    let merged = args
        .get("merged_content")
        .and_then(|v| v.as_str())
        .map(String::from);
    let outcome = brain.resolve(conflict_id, action, context, merged, None)?;
    brain.save()?;
    Ok(text_result(
        json!({
            "status": "resolved",
            "action": format!("{:?}", outcome.action),
            "stored_id": outcome.stored_id,
            "archived_ids": outcome.archived_ids
        })
        .to_string(),
    ))
}

fn tool_timeline(brain: &mut Brain, args: &Value) -> Result<Value> {
    let memory_id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let events = brain.timeline(memory_id, limit)?;
    let mut text = String::new();
    for e in events {
        text.push_str(&format!(
            "{} · {} by {}{}\n",
            e.created_at.format("%Y-%m-%d %H:%M"),
            e.event_type,
            e.actor,
            e.detail.map(|d| format!(" — {d}")).unwrap_or_default()
        ));
    }
    if text.is_empty() {
        text.push_str("(no events)\n");
    }
    Ok(text_result(text))
}

fn tool_get_full(brain: &mut Brain, args: &Value) -> Result<Value> {
    let ids = args
        .get("ids")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut items = Vec::new();
    for id in ids {
        if let Some(id) = id.as_str() {
            if let Some(m) = brain.get(id)? {
                items.push(json!({
                    "id": m.id, "content": m.content, "type": m.memory_type.as_str(),
                    "author": m.author, "tags": m.tags, "confidence": m.confidence,
                    "state": m.state.as_str()
                }));
            }
        }
    }
    Ok(text_result(serde_json::to_string_pretty(&items)?))
}

fn tool_endorse(brain: &mut Brain, args: &Value) -> Result<Value> {
    let id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
    let author = args
        .get("author")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| brain.config().general.author.clone());
    brain.endorse(id, &author)?;
    brain.save()?;
    Ok(text_result(format!("endorsed {id} by {author}")))
}

fn tool_dispute(brain: &mut Brain, args: &Value) -> Result<Value> {
    let id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
    let reason = args.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    let author = brain.config().general.author.clone();
    brain.dispute(id, &author, reason)?;
    brain.save()?;
    Ok(text_result(format!("disputed {id}: {reason}")))
}

fn tool_stats(brain: &mut Brain) -> Result<Value> {
    let s = brain.stats()?;
    Ok(text_result(
        json!({
            "total": s.total, "active": s.active, "archived": s.archived,
            "superseded": s.superseded, "disputed": s.disputed,
            "pending_conflicts": s.pending_conflicts,
            "model": s.model, "dimension": s.dimension
        })
        .to_string(),
    ))
}

fn parse_action(s: &str) -> Result<ResolutionAction> {
    Ok(match s {
        "replace" => ResolutionAction::Replace,
        "keep_both" => ResolutionAction::KeepBoth,
        "discard_new" | "discard" => ResolutionAction::DiscardNew,
        "merge" => ResolutionAction::Merge,
        other => anyhow::bail!("unknown action: {other}"),
    })
}

/// JSON schemas for `tools/list`.
fn tool_specs() -> Value {
    json!([
        {
            "name": "yb_remember",
            "description": "Store a new memory. Automatically checks for conflicts with existing memories. If a conflict is detected, returns conflict details for user resolution instead of storing directly.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The memory content to store" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "room": { "type": "string" },
                    "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." }
                },
                "required": ["content"]
            }
        },
        {
            "name": "yb_recall",
            "description": "Search and retrieve relevant memories, returning compressed, token-budgeted context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "number", "default": 5 },
                    "detail": { "type": "string", "enum": ["headline", "summary", "full"], "default": "summary" },
                    "token_budget": { "type": "number", "default": 200 },
                    "room": { "type": "string" },
                    "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." }
                },
                "required": ["query"]
            }
        },
        {
            "name": "yb_resolve",
            "description": "Resolve a memory conflict after the user decides.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "conflict_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["replace", "keep_both", "discard_new", "merge"] },
                    "context": { "type": "string" },
                    "merged_content": { "type": "string" },
                    "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." }
                },
                "required": ["conflict_id", "action"]
            }
        },
        {
            "name": "yb_timeline",
            "description": "Get the audit history of a specific memory.",
            "inputSchema": {
                "type": "object",
                "properties": { "memory_id": { "type": "string" }, "limit": { "type": "number", "default": 10 }, "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." } },
                "required": ["memory_id"]
            }
        },
        {
            "name": "yb_get_full",
            "description": "Get full content of specific memories by ID (progressive disclosure after yb_recall).",
            "inputSchema": {
                "type": "object",
                "properties": { "ids": { "type": "array", "items": { "type": "string" } }, "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." } },
                "required": ["ids"]
            }
        },
        {
            "name": "yb_endorse",
            "description": "Endorse/confirm an existing memory as still valid.",
            "inputSchema": {
                "type": "object",
                "properties": { "memory_id": { "type": "string" }, "author": { "type": "string" }, "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." } },
                "required": ["memory_id"]
            }
        },
        {
            "name": "yb_dispute",
            "description": "Flag an existing memory as potentially incorrect.",
            "inputSchema": {
                "type": "object",
                "properties": { "memory_id": { "type": "string" }, "reason": { "type": "string" }, "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." } },
                "required": ["memory_id", "reason"]
            }
        },
        {
            "name": "yb_stats",
            "description": "Get memory statistics and health info.",
            "inputSchema": { "type": "object", "properties": { "db_memory": { "type": "string", "description": "Optional named memory database to target; omit to use the server default / global database." } } }
        }
    ])
}
