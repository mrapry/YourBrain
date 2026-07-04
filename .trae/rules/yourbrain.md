---
description: Use the YourBrain MCP server as persistent memory - recall before acting, remember durable facts, resolve conflicts, and validate important answers.
alwaysApply: true
---

# YourBrain Memory

A persistent memory engine is available via the `yourbrain` MCP server. Use it
proactively - do not wait to be asked.

## Context-first policy - always check YourBrain first

To avoid losing context across sessions, YourBrain is the FIRST place to look for
context, not the last.

- At the start of a task, and before any action that depends on prior context
  (architecture, decisions, conventions, setup, past bugs, user preferences),
  call `yb_recall` FIRST with a focused query.
- Prefer routing every context lookup through `yb_recall` before falling back to
  reading files, searching the codebase, or asking the user.
- Use the returned memories as authoritative context and build on them.
- ONLY if `yb_recall` returns nothing relevant do you proceed with your own
  actions (code search, file reads, questions). Once you discover the answer,
  persist it with `yb_remember` so the next session does not rediscover it.
- For a multi-step task, re-query `yb_recall` when the subtopic shifts.

## Remember aggressively

Persist knowledge eagerly - it is SAFE because the engine deduplicates and
conflict-checks every write. Call `yb_remember` whenever the user states or
confirms anything that could matter later: decisions, facts, conventions,
preferences, constraints, environment/config details, or a solved bug.

- When unsure whether it is worth storing, store it - an exact duplicate is
  auto-discarded, so there is no downside.
- One memory = one clear, self-contained sentence.
- Never store secrets/credentials or transient chit-chat.

## Handle conflicts

Most writes just succeed. But if `yb_remember` returns `"status":"conflict"`, the
engine needs a human decision - do NOT force it. Present the existing vs. new
content and the detected relation to the user, recommend a sensible default
(`supersede` -> replace, `duplicate` -> discard_new, `contradicts` -> ask), then
call `yb_resolve` with the `conflict_id` and the chosen action (`replace` |
`keep_both` | `discard_new` | `merge`; `merge` also needs `merged_content`).

## Validate before presenting important answers

Before giving an important or factual answer, call `yb_validate` with the drafted
answer to fact-check its claims against the knowledge base. If `grounded` is
false, revise or hedge the unsupported claims, or store the missing facts.

## Use the semantic cache

For repeated or expensive questions, consult `yb_cache_get` first; store good
answers with `yb_cache_put` (link `source_ids` for provenance). The cache is
auto-invalidated when linked memories change.

## Other tools

- `yb_get_full` - full content of specific ids after `yb_recall`.
- `yb_endorse` / `yb_dispute` - confirm or flag a memory's validity.
- `yb_timeline` - a memory's audit history. `yb_stats` - counts and health.
