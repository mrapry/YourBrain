# Changelog

All notable changes to YourBrain are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0]

Retrieval precision, token efficiency, anti-hallucination, and a
knowledge-grounded semantic cache. All new "AI-hard" capabilities are pure-Rust
and embedder-independent, sitting behind traits so LLM/ONNX backends can be
added later without touching callers.

### Added

- **Context reranker.** A lexical (BM25-style) reranking stage on top of the
  existing RRF fusion, sharpening the top results returned by `yb_recall`.
  Exposed behind a `Reranker` trait (default `LexicalReranker`).
- **Dynamic token budgeter & compressor.** An opt-in mode (default OFF) that
  condenses recalled memories to fit tighter token budgets via an extractive
  summarizer. Behind a `Summarizer` trait. Toggle with `[token_budget] enabled`
  or the `dynamic_budget` argument on `yb_recall` / `yb recall --dynamic-budget`.
- **Guardrail / fact-checking tool.** New `yb_validate` MCP tool (and `yb
  validate` CLI) that checks a drafted answer's claims against the stored
  knowledge base and reports unsupported claims. Behind a `Validator` trait.
- **Layered semantic cache grounded in the knowledge base.** New `yb_cache_get`
  / `yb_cache_put` / `yb_cache_clear` MCP tools (and `yb cache` CLI):
  - Tier 1 — conversational Q&A cache (query-embedding match).
  - Tier 2 — direct authoritative answer from strongly-matching KB documents
    (bypasses the LLM).
  - Tier 3 — medium-strength KB matches returned as grounding context.
  - Cache entries store `source_ids` (provenance) and are auto-invalidated when
    a linked memory is superseded, forgotten, disputed, or re-imported, keeping
    the cache consistent with the knowledge base.
- New config sections: `[rerank]`, `[token_budget]`, `[guardrail]`, `[cache]`.
- **Overridable cache thresholds.** The `[cache]` thresholds are now defaults
  that can be overridden (a) per-project via `mcp.json` launch flags
  (`yb mcp --cache-similarity/--cache-kb-direct/--cache-kb-grounding`, writable
  through `yb install`), and (b) per-call for live tuning via the `yb_cache_get`
  arguments `similarity_threshold`/`kb_direct_threshold`/`kb_grounding_threshold`
  or `yb cache get --threshold`. Precedence: per-call > mcp.json flag > config.
- `yb cache put` now accepts `--source-id <id>` (repeatable) to link cached
  answers to their KB provenance for auto-invalidation.

### Notes

- Rollback of behavior without git: each feature has a config flag
  (`[rerank] enabled`, `[token_budget] enabled`, `[cache] enabled` /
  `[cache] use_kb`). A full rollback is `git checkout v0.1.0`.

## [0.1.0] — baseline

- Core engine: SQLite + FTS5 storage, dependency-free hash embedder, flat cosine
  vector index, rule-based 3-level compression, privacy preprocessing and
  classification, hybrid RRF retrieval with recency/importance re-ranking, and
  Tier-1 rule-based conflict resolution.
- `yb` binary: CLI, MCP stdio server, IDE hook handler, and installer.
- Named-database isolation via the global `--db-memory` flag and per-call
  `db_memory` argument (isolated `dbs/<name>/` store per project).
