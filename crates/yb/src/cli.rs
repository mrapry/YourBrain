//! CLI command implementations (in-process engine access).

use anyhow::{Context, Result};
use std::io::Write;

use yb_core::brain::{RememberOptions, RememberOutcome};
use yb_core::conflict::ResolutionAction;
use yb_core::memory::Scope;
use yb_core::search::DetailLevel;

use crate::context;

pub fn remember(
    content: &str,
    tags: Vec<String>,
    room: Option<String>,
    scope: Option<String>,
) -> Result<()> {
    let mut brain = context::open_brain()?;
    let opts = RememberOptions {
        tags,
        room,
        scope: scope.as_deref().map(|s| {
            if s == "team" {
                Scope::Team
            } else {
                Scope::Personal
            }
        }),
        ..Default::default()
    };
    println!("Checking for similar memories...");
    let outcome = brain.remember(content, opts)?;
    brain.save()?;
    match outcome {
        RememberOutcome::Stored { id } => {
            println!("Stored. id = {id}");
        }
        RememberOutcome::AutoResolved {
            id,
            action,
            relation,
        } => {
            println!("Auto-resolved ({}, {action:?}).", relation.as_str());
            if let Some(id) = id {
                println!("  stored: {id}");
            }
        }
        RememberOutcome::NeedsReview {
            conflict_id,
            analysis,
            existing,
        } => {
            println!(
                "\nPOTENTIAL {} DETECTED (confidence {:.0}%)",
                analysis.relation.as_str().to_uppercase(),
                analysis.confidence * 100.0
            );
            for m in &existing {
                println!("\n  EXISTING [{}]", m.id);
                println!("    {}", m.content);
                println!(
                    "    author: {}  created: {}",
                    m.author,
                    m.created_at.format("%Y-%m-%d")
                );
            }
            println!("\n  NEW");
            println!("    {content}");
            println!("\n  Reasoning: {}", analysis.reasoning);
            println!("\nResolve with:");
            println!("  yb resolve {conflict_id} --action replace       # new replaces old");
            println!("  yb resolve {conflict_id} --action keep_both      # both valid");
            println!("  yb resolve {conflict_id} --action discard_new    # keep old only");
            println!("  yb resolve {conflict_id} --action merge --content \"...\"");
        }
    }
    Ok(())
}

pub fn recall(
    query: &str,
    limit: usize,
    detail: &str,
    budget: usize,
    room: Option<String>,
    dynamic: bool,
) -> Result<()> {
    let brain = context::open_brain()?;
    let res = brain.recall(
        query,
        limit,
        DetailLevel::parse(detail),
        budget,
        room.as_deref(),
        dynamic,
    )?;
    println!(
        "# YB Recall ({} memories, {} tokens)",
        res.output.ids.len(),
        res.output.tokens_used
    );
    println!("{}", "-".repeat(40));
    if res.output.lines.is_empty() {
        println!("(no relevant memories found)");
    }
    for line in &res.output.lines {
        println!("{line}");
    }
    println!("{}", "-".repeat(40));
    Ok(())
}

pub fn resolve(
    conflict_id: &str,
    action: &str,
    context_str: Option<String>,
    merged: Option<String>,
) -> Result<()> {
    let mut brain = context::open_brain()?;
    let action = parse_action(action)?;
    let outcome = brain.resolve(conflict_id, action, context_str, merged, None)?;
    brain.save()?;
    println!("Resolved ({:?}).", outcome.action);
    if let Some(id) = outcome.stored_id {
        println!("  stored:   {id}");
    }
    for id in outcome.archived_ids {
        println!("  archived: {id}");
    }
    Ok(())
}

pub fn list(room: Option<String>, limit: usize) -> Result<()> {
    let brain = context::open_brain()?;
    let memories = brain.list(room.as_deref(), limit)?;
    if memories.is_empty() {
        println!("(no memories)");
    }
    for m in memories {
        println!(
            "{} [{}] {} @{}",
            m.id,
            m.memory_type.as_str(),
            m.headline,
            m.author
        );
    }
    Ok(())
}

pub fn get(id: &str) -> Result<()> {
    let brain = context::open_brain()?;
    match brain.get(id)? {
        None => println!("not found: {id}"),
        Some(m) => {
            println!("id:         {}", m.id);
            println!("type:       {}", m.memory_type.as_str());
            println!("state:      {}", m.state.as_str());
            println!("scope:      {}", m.scope.as_str());
            println!("author:     {}", m.author);
            println!("room:       {}", m.room.unwrap_or_default());
            println!("tags:       {}", m.tags.join(", "));
            println!("confidence: {:.2}", m.confidence);
            println!("created:    {}", m.created_at.format("%Y-%m-%d %H:%M"));
            println!("\ncontent:\n{}", m.content);
            let edges = brain.edges_for(id)?;
            if !edges.is_empty() {
                println!("\nrelations:");
                for e in edges {
                    println!(
                        "  {} --{}--> {}",
                        e.source_id,
                        e.edge_type.as_str(),
                        e.target_id
                    );
                }
            }
        }
    }
    Ok(())
}

pub fn forget(id: &str) -> Result<()> {
    let mut brain = context::open_brain()?;
    brain.forget(id)?;
    brain.save()?;
    println!("archived: {id}");
    Ok(())
}

pub fn endorse(id: &str, author: Option<String>) -> Result<()> {
    let mut brain = context::open_brain()?;
    let author = author.unwrap_or_else(|| brain.config().general.author.clone());
    brain.endorse(id, &author)?;
    brain.save()?;
    println!("endorsed {id} by {author}");
    Ok(())
}

pub fn dispute(id: &str, reason: &str) -> Result<()> {
    let mut brain = context::open_brain()?;
    let author = brain.config().general.author.clone();
    brain.dispute(id, &author, reason)?;
    brain.save()?;
    println!("disputed {id}: {reason}");
    Ok(())
}

pub fn conflicts() -> Result<()> {
    let brain = context::open_brain()?;
    let list = brain.list_conflicts(true)?;
    if list.is_empty() {
        println!("(no pending conflicts)");
    }
    for c in list {
        println!(
            "{} [{}] {:.0}% — {}",
            c.id,
            c.analysis.relation.as_str(),
            c.analysis.confidence * 100.0,
            c.new_memory.headline
        );
    }
    Ok(())
}

pub fn timeline(memory_id: &str, limit: usize) -> Result<()> {
    let brain = context::open_brain()?;
    let events = brain.timeline(memory_id, limit)?;
    if events.is_empty() {
        println!("(no events)");
    }
    for e in events {
        println!(
            "{} · {} by {}{}",
            e.created_at.format("%Y-%m-%d %H:%M"),
            e.event_type,
            e.actor,
            e.detail.map(|d| format!(" — {d}")).unwrap_or_default()
        );
    }
    Ok(())
}

pub fn stats() -> Result<()> {
    let brain = context::open_brain()?;
    let s = brain.stats()?;
    println!("YourBrain statistics");
    println!("  yb version:        {}", env!("CARGO_PKG_VERSION"));
    println!("  total memories:    {}", s.total);
    println!("  active:            {}", s.active);
    println!("  superseded:        {}", s.superseded);
    println!("  archived:          {}", s.archived);
    println!("  disputed:          {}", s.disputed);
    println!("  pending conflicts: {}", s.pending_conflicts);
    println!("  embedding model:   {} ({}d)", s.model, s.dimension);
    Ok(())
}

pub fn config_show() -> Result<()> {
    let cfg = context::load_config()?;
    println!("# yb version: {}", env!("CARGO_PKG_VERSION"));
    println!("{}", cfg.to_toml());
    println!("data dir: {}", context::data_dir()?.display());
    println!("active db: {}", context::active_db_dir()?.display());
    println!("config:   {}", context::config_path()?.display());
    Ok(())
}

pub fn export(scope: Option<String>, out: Option<String>) -> Result<()> {
    let brain = context::open_brain()?;
    let scope = scope.as_deref().map(|s| {
        if s == "team" {
            Scope::Team
        } else {
            Scope::Personal
        }
    });
    let memories = brain.export(scope)?;
    let mut writer: Box<dyn Write> = match out {
        Some(path) => {
            Box::new(std::fs::File::create(&path).with_context(|| format!("creating {path}"))?)
        }
        None => Box::new(std::io::stdout()),
    };
    for m in memories {
        writeln!(writer, "{}", serde_json::to_string(&m)?)?;
    }
    Ok(())
}

pub fn import(file: &str) -> Result<()> {
    let mut brain = context::open_brain()?;
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {file}"))?;
    let (mut stored, mut skipped) = (0u32, 0u32);
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let memory = serde_json::from_str(line).with_context(|| "parsing JSONL line")?;
        if brain.import_memory(memory)? {
            stored += 1;
        } else {
            skipped += 1;
        }
    }
    brain.save()?;
    println!("imported: {stored} stored, {skipped} skipped (already present)");
    Ok(())
}

pub fn validate(answer: &str, query: Option<String>) -> Result<()> {
    let brain = context::open_brain()?;
    let report = brain.validate(answer, query.as_deref(), None, None)?;
    println!(
        "Grounding: {:.0}%  ({})",
        report.grounding_score * 100.0,
        if report.grounded {
            "grounded"
        } else {
            "UNSUPPORTED CLAIMS FOUND"
        }
    );
    for c in &report.claims {
        let mark = if c.supported { "ok " } else { "!! " };
        println!("  [{mark}] {:.0}%  {}", c.score * 100.0, c.text);
    }
    if !report.unsupported.is_empty() {
        println!("\nUnsupported claims (revise or verify against the knowledge base):");
        for u in &report.unsupported {
            println!("  - {u}");
        }
    }
    Ok(())
}

pub fn cache(
    action: &str,
    query: Option<String>,
    answer: Option<String>,
    threshold: Option<f32>,
    source_ids: Vec<String>,
) -> Result<()> {
    use yb_core::brain::{CacheLookup, CacheOverrides};
    let brain = context::open_brain()?;
    match action {
        "get" => {
            let q = query.ok_or_else(|| anyhow::anyhow!("`get` requires a query"))?;
            let overrides = CacheOverrides {
                similarity: threshold,
                ..Default::default()
            };
            match brain.cache_get(&q, None, overrides)? {
                CacheLookup::Hit {
                    answer,
                    source,
                    similarity,
                    memory_ids,
                } => {
                    println!(
                        "HIT [{}] ({:.0}% match)",
                        source.as_str(),
                        similarity * 100.0
                    );
                    if !memory_ids.is_empty() {
                        println!("source memories: {}", memory_ids.join(", "));
                    }
                    println!("\n{answer}");
                }
                CacheLookup::Grounding {
                    memories,
                    similarity,
                } => {
                    println!(
                        "GROUNDING ({:.0}% match) — {} memory(ies):",
                        similarity * 100.0,
                        memories.len()
                    );
                    for m in memories {
                        println!("  [{}] {}", m.id, m.headline);
                    }
                }
                CacheLookup::Miss => println!("MISS"),
            }
        }
        "put" => {
            let q = query.ok_or_else(|| anyhow::anyhow!("`put` requires a query"))?;
            let a = answer.ok_or_else(|| anyhow::anyhow!("`put` requires --answer"))?;
            match brain.cache_put(&q, &a, source_ids, None, None)? {
                Some(id) => println!("cached: {id}"),
                None => println!("cache is disabled in config"),
            }
        }
        "clear" => {
            let n = brain.cache_clear(None)?;
            println!("cleared {n} cache entr{}", if n == 1 { "y" } else { "ies" });
        }
        other => anyhow::bail!("unknown cache action `{other}` (get|put|clear)"),
    }
    Ok(())
}

fn parse_action(s: &str) -> Result<ResolutionAction> {
    Ok(match s {
        "replace" => ResolutionAction::Replace,
        "keep_both" => ResolutionAction::KeepBoth,
        "discard_new" | "discard" => ResolutionAction::DiscardNew,
        "merge" => ResolutionAction::Merge,
        other => anyhow::bail!("unknown action `{other}` (replace|keep_both|discard_new|merge)"),
    })
}
