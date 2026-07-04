# Changelog

All notable changes to YourBrain are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Hook foreign-key crash.** IDE hooks (e.g. Claude Code) send their own stable
  `session_id` on every event instead of threading back the id minted by
  `session_start`, so `prompt_submit`/`tool_use` recorded an observation for a
  session row that never existed and failed the `observations -> sessions`
  foreign key (`Error 787: FOREIGN KEY constraint failed`). Hooks now adopt the
  IDE-provided `session_id` and get-or-create the session (`Brain::ensure_session`)
  before writing any observation.

## [Unreleased] — v0.3.0

Optional ONNX sentence-transformer embedder for real semantic understanding.

### Added

- **ONNX embedder (`onnx` cargo feature).** A sentence-transformer backend via
  [`fastembed`](https://github.com/anush008/fastembed-rs) (tokenizer + mean
  pooling + L2 normalization; model auto-downloaded from HuggingFace and cached).
  Off by default so standard builds stay dependency-free; enable with
  `cargo build --features onnx`. Supported model keys include
  `multilingual-e5-small` (default, 384d, multilingual), `multilingual-e5-base`,
  `all-minilm-l6-v2`, `bge-small-en-v1.5`, `paraphrase-multilingual-minilm-l12-v2`.
- **Asymmetric embedding.** New `Embedder::embed_query` / `embed_document`
  methods (default to `embed`); the ONNX backend applies E5 `query:` / `passage:`
  instruction prefixes automatically. Call sites now distinguish query vs.
  document embedding.
- **`yb reindex` (CLI).** Re-embeds every memory with a chosen provider/model,
  rebuilds the vector index, clears the semantic cache, and moves the ADR-5
  model/dimension lock — the migration path for switching embedders. Dry-run by
  default; pass `--yes` to apply.
- **Per-project embedder override.** `yb mcp --embedder onnx --embed-model <key>`
  (writable via `yb install`) overrides `config.toml` `[embedding]` for one
  server. New `[embedding] cache_dir` config option for the model cache. Global
  `--embedder` / `--embed-model` CLI options let any subcommand target a
  reindexed database.
- **Per-project conflict threshold.** `yb mcp --conflict-similarity <f>`
  (writable via `yb install`) overrides `[conflict] similarity_threshold` for one
  server — raise to ~0.75 when using ONNX embeddings.

### Notes

- The ONNX model download requires internet on first use. The `onnx` feature is
  opt-in; a binary built without it errors clearly if `provider = "onnx"`.
- **Windows release build:** the aggressive `lto = "thin"` release profile can
  crash `rustc` codegen (`STATUS_ACCESS_VIOLATION`) on the ONNX dependency graph.
  Build the ONNX binary with `CARGO_PROFILE_RELEASE_LTO=off`
  (and `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`); it stays optimized. See README.

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
