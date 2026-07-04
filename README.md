# YourBrain (`yb`)

**A local-first memory engine that gives AI coding agents a persistent, coherent long-term memory — with first-class conflict resolution.**

![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust&logoColor=white)
![License](https://img.shields.io/badge/License-Apache--2.0-blue)
![Local-first](https://img.shields.io/badge/Local--first-no%20cloud%2C%20no%20server-2ea44f)
![Single binary](https://img.shields.io/badge/Ships%20as-one%20binary-informational)
![MCP](https://img.shields.io/badge/MCP-Cursor%20%C2%B7%20Claude%20%C2%B7%20VS%20Code%20%C2%B7%20Trae-8A2BE2)

YourBrain is a single Rust binary (`yb`) that plugs into any MCP-capable AI IDE
and remembers what matters across sessions — decisions, conventions, gotchas,
your preferences — and **keeps that memory coherent** as it grows. No server to
run, no database to provision, no cloud dependency. Everything is local by
default.

---

## The problem: agentic AI has amnesia

Today's AI coding agents are brilliant but **stateless**. Every new chat starts
from zero:

- **They forget.** Architecture decisions, naming conventions, that bug you
  fixed last week, the fact that you migrated off Postgres — gone the moment the
  session ends. You re-explain the same context over and over.
- **They waste tokens and time.** Re-pasting context into every prompt burns
  tokens (and money), and slows you down.
- **They hallucinate and contradict.** With no grounded memory, agents confidently
  state things that were true three refactors ago — or never true at all.
- **"Memory" tools just pile up.** Most add-on memory is an append-only vector
  store. It can tell you *"what did I say about X?"* but never notices when
  *"we use JWT"* is later replaced by *"we moved to session cookies."* Over
  months the memory rots into a pile of conflicting half-truths.

MCP gave agents *tools*. It did not give them a **coherent long-term memory**.

## What YourBrain does — and why it matters

YourBrain is the memory layer. Your agent calls it before answering and after
learning something durable.

| Without YourBrain | With YourBrain |
|---|---|
| Re-explain context every session | Agent recalls prior decisions instantly |
| Memory silently goes stale | Conflicts are **detected and resolved** on write |
| Big prompts, high token cost | Token-budgeted, compressed recall |
| Confident hallucinations | Answers **fact-checked** against your knowledge base |
| Repeated Q&A recomputed | **Semantic cache** reuses grounded answers |
| Cloud lock-in, privacy risk | 100% local, single binary, your data stays put |

**The differentiator is the conflict engine.** When new knowledge duplicates,
supersedes, or contradicts what was stored before, YourBrain:

- detects it on write (duplicate / supersede / contradiction),
- auto-resolves the safe cases and asks a human for the ambiguous ones,
- keeps an **audit timeline** and a **relationship graph** (`supersedes`,
  `contradicts`, `complements`, …) so history is never lost.

## Features

| Capability | What it gives you |
|---|---|
| **Conflict resolution** | Rule-based detection of duplicate/supersede/contradiction with auto-resolution thresholds. Higher tiers (NLI / LLM) are pluggable via a trait. |
| **Hybrid search** | SQLite **FTS5** keyword search fused with **vector** similarity (Reciprocal Rank Fusion), re-ranked by recency/importance/confidence. |
| **Lexical reranking** | A BM25 rerank stage sharpens query relevance on top of RRF fusion (`Reranker` trait). |
| **Compression** | Near-lossless compression that **preserves code, paths, and URLs**, plus `summary`/`headline` levels for token-efficient recall. |
| **Dynamic token budgeting** | Opt-in query-aware extractive summarization condenses recall to fit tight budgets (`Summarizer` trait). |
| **Guardrail / fact-check** | `yb validate` checks a drafted answer against the knowledge base and flags unsupported claims (`Validator` trait). |
| **Semantic cache** | Layered cache grounded in the KB: prior Q&A → direct KB answer → KB grounding, with provenance-based **auto-invalidation**. |
| **Semantic embeddings** | Dependency-free hash embedder by default; optional **ONNX sentence-transformers** (e5/MiniLM/BGE) for true meaning-based recall. |
| **Privacy** | Strips `<private>…</private>` blocks and redacts secrets (API keys, tokens, connection strings) before storing. |
| **Team knowledge** | Endorse / dispute memories (confidence = consensus). Export/import as JSONL. |
| **IDE integration** | An **MCP server** (`yb mcp`) for Cursor, Claude Code, VS Code, and Trae, plus **hooks** (`yb hook`) for auto-capture. |

## How YourBrain compares

The local-first agent-memory space is healthy and growing — projects like
[uteke](https://github.com/codecoradev/uteke) and
[cavemem](https://github.com/JuliusBrussee/cavemem) share the same "single
binary, offline, MCP" philosophy, and are excellent at *storing and retrieving*
memory. YourBrain's focus is one layer up: keeping that memory **coherent and
trustworthy** as it grows.

| | **YourBrain** | uteke | cavemem | Mem0 |
|---|---|---|---|---|
| Distribution | Single Rust binary | Single Rust binary (+Docker) | npm / Node (TS) | Python/JS SDK + SaaS |
| Offline / local-first | Yes | Yes | Yes | Partial (cloud embedding) |
| MCP server | Yes — stdio, 12 tools | Yes — stdio + HTTP | Yes — stdio, 3 tools | Via integrations |
| Hybrid search (FTS5 + vector + RRF) | Yes (flat cosine) | Yes (HNSW) | Yes | Semantic |
| Default embeddings | Hash; ONNX optional | ONNX (EmbeddingGemma 768d) | Remote optional | LLM / cloud |
| BM25 rerank + dynamic token budget | Yes | Decay/salience ranking | Tunable ranker | — |
| Code-safe compression | Yes (3 levels) | — | Yes (~75%) | — |
| **Automatic conflict detection + resolution** | **Yes** (duplicate/supersede/contradiction, auto or ask) | Typed edges (manual) | No (append-only) | Limited dedup/update |
| **Answer fact-check / guardrail** | **Yes** (`yb_validate`) | No | No | No |
| **KB-grounded semantic answer cache** | **Yes** (tiered + provenance auto-invalidation) | LRU recall cache (perf) | No | No |
| **Team consensus** (endorse/dispute) | **Yes** (Laplace confidence, scope) | Author attribution / namespaces | No | Per-user |
| Relationship graph + audit timeline | Yes | Yes (rich; Graph API) | Timeline | Limited |
| Server/daemon + HTTP, document engine | Roadmap | Yes | Web viewer | Cloud platform |
| License | Apache-2.0 | Apache-2.0 | MIT | Apache-2.0 |

**Honest take:** uteke ships more breadth today (HNSW, ONNX by default, server
mode, a document/wiki engine, benchmark harness); cavemem has a slick web viewer
and the widest one-command IDE installers. **What is unique to YourBrain** is the
*coherence layer* — automatic conflict resolution, answer validation against the
knowledge base, and a KB-grounded semantic cache — so your agent's memory does
not quietly rot into contradictions over time.

## How it works

```
                 ┌─────────────────────────────────────────┐
   yb CLI  ─────▶│                yb-core                    │
   yb mcp  ─────▶│  Brain (facade)                           │
   yb hook ─────▶│   ├── classify (privacy + type)           │
                 │   ├── compress (3 levels)                 │
                 │   ├── embed  (Embedder trait)  ──┐        │
                 │   ├── vector (VectorIndex trait) │ pluggable
                 │   ├── search (FTS5 + RRF + rerank)│ backends │
                 │   ├── budget / guardrail / cache │         │
                 │   ├── conflict (tiered)          │         │
                 │   └── store  (SQLite + FTS5)  ◀──┘         │
                 └─────────────────────────────────────────┘
```

The engine depends only on the `Embedder` and `VectorIndex` **traits**. The
default build uses pure-Rust implementations (a deterministic feature-hashing
embedder and an exact flat-cosine index) so it **compiles and runs anywhere**
with no C++/ONNX toolchain. Production ONNX embeddings drop in behind the same
trait — see [Installation](docs/INSTALL.md).

## Quick start

```bash
# 1. Build (needs a Rust toolchain 1.75+)
git clone <repo-url> yourbrain && cd yourbrain
cargo build --release            # binary at ./target/release/yb (yb.exe on Windows)

# 2. Store and recall knowledge
yb remember "Backend API uses Rust with the Axum web framework" --tag backend
yb recall  "how is the backend built"

# 3. Update knowledge → conflict detected → resolve
yb remember "Auth now moved to session cookies instead of JWT"
#  ↳ POTENTIAL SUPERSEDE DETECTED … resolve with:
yb resolve <conflict_id> --action replace

# 4. Wire it into your IDE (Cursor shown; see docs for Claude/VS Code/Trae)
yb install --ide cursor
```

Full, OS-by-OS build and run instructions: **[docs/INSTALL.md](docs/INSTALL.md)**.

## Use it in your AI IDE

YourBrain speaks the **Model Context Protocol (MCP)**, so it works with any
MCP-capable client. `yb install` generates the config for Cursor and Claude
Code automatically; VS Code and Trae take a one-file config.

| IDE | Setup | Auto-capture hooks |
|---|---|---|
| **Cursor** | `yb install --ide cursor` | — (MCP only) |
| **Claude Code** | `yb install --ide claude-code` | yes |
| **VS Code** (Copilot Agent) | `.vscode/mcp.json` (manual) | — |
| **Trae** | Settings → MCP → Add, or `.trae/mcp.json` | — |

Step-by-step for every IDE, plus per-project tuning:
**[docs/IDE_SETUP.md](docs/IDE_SETUP.md)**.

> **Claude Code hooks need matching flags.** The generated `.claude/settings.json`
> is machine-specific (absolute paths) so it is gitignored — see the committed
> template **[`.claude/settings.example.json`](.claude/settings.example.json)**.
> Every `yb hook` command must repeat the same `--db-memory` / `--embedder` /
> `--embed-model` flags as your MCP server, or captured sessions land in the wrong
> database or fail the embedding-dimension lock. `yb install --ide claude-code`
> handles this for you.

Once connected, the agent gets these tools: `yb_remember`, `yb_recall`,
`yb_resolve`, `yb_endorse`, `yb_dispute`, `yb_timeline`, `yb_get_full`,
`yb_stats`, `yb_validate`, `yb_cache_get`, `yb_cache_put`, `yb_cache_clear`.

## Command reference

| Command | Description |
|---|---|
| `yb remember <content> [--tag T]… [--room R] [--scope personal\|team]` | Store a memory (with conflict check). |
| `yb recall <query> [--limit N] [--detail headline\|summary\|full] [--budget N] [--dynamic-budget]` | Search & retrieve. |
| `yb resolve <id> --action replace\|keep_both\|discard_new\|merge [--content …]` | Resolve a conflict. |
| `yb validate <answer> [--query Q]` | Fact-check an answer against the knowledge base. |
| `yb cache get\|put\|clear [--query Q] [--answer A] [--threshold F] [--source-id ID]…` | Layered semantic cache. |
| `yb list [--room R] [--limit N]` | List memories. |
| `yb get <id>` | Show full memory + relations. |
| `yb forget <id>` | Archive a memory. |
| `yb endorse <id> [--author A]` / `yb dispute <id> --reason R` | Team consensus. |
| `yb conflicts` | List pending conflicts. |
| `yb timeline <id>` | Audit history. |
| `yb stats` | Statistics & health. |
| `yb export [--scope team] [--out file]` / `yb import <file>` | JSONL export/import. |
| `yb reindex [--provider onnx] [--model KEY] [--yes]` | Re-embed all memories & rebuild the index (migrate embedder). |
| `yb config show` | Show config & paths. |
| `yb mcp [flags]` | Start the MCP stdio server (see IDE setup for per-project flags). |
| `yb install --ide cursor\|claude-code [flags]` | Generate IDE integration files. |

Global options (any command): `--db-memory <name>` (isolate a project),
`--embedder <local\|onnx>` and `--embed-model <key>` (target a reindexed database).

## Configuration

Config lives at `<data_dir>/config.toml`. The data directory is resolved as:

- `YB_DATA_DIR` environment variable (overrides everything), else
- Windows `%APPDATA%\yourbrain`, otherwise `~/.yourbrain`.

Every option is documented in **[config.example.toml](config.example.toml)**.
Key tunables: retrieval (`[search]`, `[rerank]`), token budget
(`[token_budget]`), fact-check (`[guardrail]`), semantic cache (`[cache]`),
conflict sensitivity (`[conflict]`), and embeddings (`[embedding]`).

### Isolate memories per project

By default every project shares one global database. Pass a named `db_memory`
to keep a project's memories fully isolated (separate SQLite store, indexes, and
conflict scope):

```bash
yb --db-memory my-project remember "Project-specific decision"
yb --db-memory my-project recall  "decisions"
```

`yb install` auto-detects the project (git repo folder) and writes
`--db-memory <name>` into the generated MCP config. See
[docs/IDE_SETUP.md](docs/IDE_SETUP.md#per-project-settings) for per-project token
budget, cache thresholds, conflict sensitivity, and embedder selection.

### Semantic understanding (optional ONNX)

The default embedder is dependency-free but matches mostly on shared vocabulary.
For true meaning-based recall (paraphrases with no common words), build with the
optional ONNX sentence-transformer backend and migrate:

```bash
cargo build --release --features onnx
yb reindex --provider onnx --model multilingual-e5-small --yes
```

Full details, supported models, and the Windows build note are in
**[docs/INSTALL.md](docs/INSTALL.md#optional-onnx-embedder)**.

## Team collaboration

Memory is more valuable when a whole team shares it. YourBrain is built so a team
converges on **one coherent source of truth** instead of many drifting private
notes:

- **Scopes** — `yb remember "<fact>" --scope team` marks a memory as shared;
  `--scope personal` (the default) keeps it author-private. Only team-scoped
  memories are meant to travel between people.
- **Consensus, not just storage** — anyone can `yb endorse <id>` or
  `yb dispute <id>`. Confidence is computed as a Laplace-smoothed ratio
  `(endorsements + 1) / (endorsements + disputes + 2)`, so widely-endorsed facts
  rise and contested ones fall (and can flip to a `Disputed` state) — no single
  author's claim wins by default.
- **Author-aware conflict resolution** — when a new memory *supersedes* an
  existing one written by **a different author**, YourBrain **always asks for a
  human decision** rather than silently overwriting a teammate's knowledge.
  Same-author updates can auto-resolve.
- **Share via JSONL or git** — `yb export --scope team --out team.jsonl` then
  `yb import team.jsonl` on another machine; or commit a project's isolated
  `--db-memory <name>` database into your repo so everyone loads the same context.

**Why it matters:** new teammates inherit architecture and decisions instantly,
changed decisions are recorded on the timeline instead of lost, and important
claims can be checked with `yb_validate` before an agent acts on them.

> Real-time multi-user sync (git/relay) is on the roadmap; the foundations —
> `brain_meta`, JSONL export/import, and `Scope` — are already in place.

## Documentation

| Doc | What's inside |
|---|---|
| [docs/INSTALL.md](docs/INSTALL.md) | Build & run on Windows, macOS, Linux; ONNX build; updating. |
| [docs/IDE_SETUP.md](docs/IDE_SETUP.md) | Cursor, Claude Code, VS Code, Trae — config + per-project tuning. |
| [config.example.toml](config.example.toml) | Every configuration option, annotated. |
| [CHANGELOG.md](CHANGELOG.md) | Release history. |

## Project status

A clean, fully tested **core** (storage, embedding, vector search, compression,
classification, hybrid retrieval + reranking, dynamic budgeting, guardrail,
semantic cache, Tier-1 conflict resolution) plus the `yb` binary (CLI, MCP
server, hook handler, installer) running in **in-process mode**. Optional ONNX
embeddings ship behind the `onnx` feature. Higher-tier conflict judges (NLI/LLM),
a HNSW index, a standalone daemon/IPC, and team relay sync are designed behind
stable interfaces as the natural next steps.

## Contributing

Contributions are welcome. Before opening a PR:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Keep code comments in English, and add a note to [CHANGELOG.md](CHANGELOG.md)
for user-facing changes.

## License

[Apache-2.0](LICENSE).
