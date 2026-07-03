# YourBrain (`yb`)

**An AI memory engine with first-class conflict resolution.** YourBrain gives
AI coding assistants a persistent, personal/team memory that stays *coherent*
over time: when new knowledge contradicts or supersedes what was stored before,
YourBrain detects it and resolves it — instead of silently accumulating stale,
conflicting facts.

It ships as a **single Rust binary** (`yb`) with **zero external
infrastructure**: no server to run, no database to provision, no cloud
dependency. Everything is local by default.

---

## Why YourBrain?

Most "AI memory" tools are append-only vector stores. They answer *"what did I
say about X?"* but they never notice when *"we use Postgres"* is later
contradicted by *"we migrated to MySQL"*. Over months, the memory becomes a pile
of half-truths.

YourBrain's differentiator is the **conflict engine**:

- Detects **duplicates**, **supersessions**, and **contradictions** on write.
- Auto-resolves the safe cases; asks a human (or the AI) for the ambiguous ones.
- Keeps an **audit timeline** and **relationship graph** (`supersedes`,
  `contradicts`, `complements`, …) so history is never lost.

## Features

| Capability | Description |
|---|---|
| Conflict resolution | Rule-based tier that flags duplicate/supersede/contradiction, with auto-resolution thresholds. Pluggable higher tiers (NLI / LLM) via a trait. |
| Hybrid search | SQLite **FTS5** keyword search fused with **vector** similarity via Reciprocal Rank Fusion, then re-ranked by recency/importance/confidence. |
| Compression | Rule-based, near-lossless compression that **preserves code, paths, and URLs**, plus lossy `summary`/`headline` levels for token-efficient recall. |
| Token budgeting | Recall fits within a configurable token budget, giving the top hits more detail and the rest a headline. |
| Privacy | Strips `<private>…</private>` blocks and redacts secrets (API keys, tokens, connection strings) before anything is stored. |
| Team knowledge | Endorse / dispute memories; confidence is a consensus score. Export/import as JSONL. |
| IDE integration | An **MCP server** (`yb mcp`) for Cursor / Claude Code, plus **hooks** (`yb hook`) for auto-capture. |

## Architecture at a glance

```
                 ┌─────────────────────────────────────────┐
   yb CLI  ─────▶│                yb-core                    │
   yb mcp  ─────▶│  Brain (facade)                           │
   yb hook ─────▶│   ├── classify (privacy + type)           │
                 │   ├── compress (3 levels)                 │
                 │   ├── embed  (Embedder trait)  ──┐        │
                 │   ├── vector (VectorIndex trait) │ pluggable
                 │   ├── search (FTS5 + RRF)        │ backends │
                 │   ├── conflict (tiered)          │         │
                 │   └── store  (SQLite + FTS5)  ◀──┘         │
                 └─────────────────────────────────────────┘
```

The engine depends only on the [`Embedder`] and [`VectorIndex`] **traits**. The
default build uses pure-Rust implementations (a deterministic feature-hashing
embedder and an exact flat cosine index) so it **compiles and runs anywhere**
with no C++/ONNX toolchain. Production ONNX embeddings and a HNSW index can be
dropped in behind the same traits — see [`docs/TEKNIS.md`](docs/TEKNIS.md).

## Install / Build

Prerequisites: a Rust toolchain (1.75+) and a C compiler (bundled SQLite is
compiled from source; on Windows the MSVC Build Tools are used).

```bash
git clone <repo-url> yourbrain
cd yourbrain
cargo build --release
# binary at ./target/release/yb  (yb.exe on Windows)
```

Run the tests:

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
```

## Quick start

```bash
# Store some knowledge
yb remember "Backend API uses Rust with the Axum web framework" --tag backend
yb remember "Auth uses JWT tokens stored in Redis with 15min expiry" --tag auth

# Recall (token-budgeted, compressed)
yb recall "how does authentication work"

# Update knowledge → conflict detected
yb remember "Auth now migrates to session cookies instead of JWT"
#  ↳ POTENTIAL SUPERSEDE DETECTED … resolve with:
yb resolve <conflict_id> --action replace

# Inspect
yb list
yb stats
yb get <id>
yb timeline <id>
```

## IDE integration

```bash
# Cursor (MCP-based recall/remember)
yb install --ide cursor

# Claude Code (MCP + auto-capture hooks)
yb install --ide claude-code
```

This writes `.cursor/mcp.json` / `.mcp.json` / `.claude/settings.json` pointing
at the current `yb` binary. See the
[usage guide](docs/PANDUAN.md) for details.

## Command reference

| Command | Description |
|---|---|
| `yb remember <content> [--tag T]… [--room R] [--scope personal\|team]` | Store a memory (with conflict check). |
| `yb recall <query> [--limit N] [--detail headline\|summary\|full] [--budget N]` | Search & retrieve. |
| `yb resolve <id> --action replace\|keep_both\|discard_new\|merge [--content …]` | Resolve a conflict. |
| `yb list [--room R] [--limit N]` | List memories. |
| `yb get <id>` | Show full memory + relations. |
| `yb forget <id>` | Archive a memory. |
| `yb endorse <id> [--author A]` / `yb dispute <id> --reason R` | Team consensus. |
| `yb conflicts` | List pending conflicts. |
| `yb timeline <id>` | Audit history. |
| `yb stats` | Statistics & health. |
| `yb export [--scope team] [--out file]` / `yb import <file>` | JSONL export/import. |
| `yb config show` | Show config & paths. |
| `yb mcp` | Start the MCP stdio server. |
| `yb hook <event>` | Handle an IDE hook (reads JSON from stdin). |
| `yb install --ide cursor\|claude-code` | Generate IDE integration files. |

## Configuration

Config lives at `<data_dir>/config.toml`. Data directory resolution:

- `YB_DATA_DIR` environment variable (overrides all), else
- Windows: `%APPDATA%\yourbrain`, else `~/.yourbrain`.

See [`config.example.toml`](config.example.toml) for every option.

### Isolating memories per project (`--db-memory`)

By default every project shares one global database. To keep a project's
memories isolated, pass a named `db_memory`:

```bash
yb --db-memory my-project remember "Project-specific decision"
yb --db-memory my-project recall "decisions"
```

Named databases live in an isolated `dbs/<name>/` subfolder of the data dir, so
their SQLite store, FTS index, vector index, and conflict scope are fully
separate. Omitting `--db-memory` uses the shared/global database. The shared
`config.toml` applies to all databases.

For IDE integration this is wired through the MCP server config (see below):
`yb install --ide cursor` auto-detects the project name (git repo folder) and
writes `--db-memory <name>` into the generated config. Individual MCP tool calls
may also override it with their own `db_memory` argument.

## Documentation

- [`docs/TEKNIS.md`](docs/TEKNIS.md) — technical documentation (Bahasa Indonesia) for engineers continuing the project.
- [`docs/PANDUAN.md`](docs/PANDUAN.md) — usage guide (Bahasa Indonesia).
- [`PLAN.md`](PLAN.md) — the full product/architecture plan, including the ADRs.

## Project status

This repository implements a clean, fully tested **core** (storage, embedding,
vector search, compression, classification, hybrid retrieval, Tier‑1 conflict
resolution) plus the `yb` binary (CLI, MCP server, hook handler, installer)
running in **in-process mode**. Higher-tier conflict judges (NLI/LLM), ONNX
embeddings, the standalone daemon/IPC, and team relay sync are designed behind
stable interfaces and are the natural next steps — see the roadmap in `PLAN.md`.

## License

Apache-2.0.
