# IDE Setup

YourBrain talks to your AI IDE over the **Model Context Protocol (MCP)** by
running `yb mcp` as a local stdio server. Any MCP-capable client can use it.

- [Before you start](#before-you-start)
- [Cursor](#cursor)
- [Claude Code](#claude-code)
- [VS Code (GitHub Copilot)](#vs-code-github-copilot)
- [Trae](#trae)
- [Available tools](#available-tools)
- [Per-project settings](#per-project-settings)
- [Verify it works](#verify-it-works)
- [Reloading after changes](#reloading-after-changes)

---

## Before you start

1. Build the binary and note its path — see [INSTALL.md](INSTALL.md).
   The examples below assume `yb` is on your `PATH`. If it is not, replace
   `"yb"` with the **absolute path** to the binary, e.g.
   `"C:\\path\\to\\yourbrain\\target\\release\\yb.exe"` or
   `"/home/you/yourbrain/target/release/yb"`.
2. `yb install` (Cursor / Claude Code) writes configs pointing at the **absolute
   path** of the binary that ran it, so they work regardless of `PATH`.

> **Config root key differs by client:** Cursor, Claude Code, and Trae use
> `mcpServers`. **VS Code uses `servers`.** Copying a block between them without
> changing the root key results in the server silently not loading.

---

## Cursor

The easy path — run from your project root:

```bash
yb install --ide cursor
```

This writes:

- `.cursor/mcp.json` — the MCP server `yourbrain` (with `--db-memory <project>`
  auto-detected from the folder / git repo name).
- `.cursorrules` — guidance telling the assistant to recall first, remember
  durable facts, consult the cache, and validate important answers.

Then **reload** Cursor (or toggle the server in Settings → MCP).

**Manual equivalent** — `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "yourbrain": {
      "command": "yb",
      "args": ["mcp", "--db-memory", "my-project"]
    }
  }
}
```

---

## Claude Code

```bash
yb install --ide claude-code
```

This writes:

- `.mcp.json` — the MCP server `yourbrain`.
- `.claude/settings.json` — **auto-capture hooks** (`yb hook …`) that record
  session activity automatically.

**Windows note:** Claude Code runs hooks via `sh`. If it is missing, install
[Git for Windows](https://git-scm.com/download/win) and add `Git\bin` to `PATH`
(`winget install Git.Git`).

**Manual equivalent** — `.mcp.json`:

```json
{
  "mcpServers": {
    "yourbrain": {
      "command": "yb",
      "args": ["mcp", "--db-memory", "my-project"]
    }
  }
}
```

---

## VS Code (GitHub Copilot)

MCP is available in VS Code 1.102+ and tools only run in **Agent mode** (switch
the Copilot Chat dropdown from *Ask* to *Agent*).

Create **`.vscode/mcp.json`** in your workspace (note the `servers` root key and
the required `"type": "stdio"`):

```json
{
  "servers": {
    "yourbrain": {
      "type": "stdio",
      "command": "yb",
      "args": ["mcp", "--db-memory", "my-project"]
    }
  }
}
```

Open it quickly via the Command Palette → **MCP: Open Workspace Folder
Configuration** (or **MCP: Open User Configuration** for a global server). Start
the server from the CodeLens in that file, then open Copilot Chat in Agent mode.

---

## Trae

Trae (VS Code-based, by ByteDance) supports MCP with the standard `mcpServers`
schema.

**Via the UI:** Settings → **MCP** → **Add** → **Configure Manually**, then paste:

```json
{
  "mcpServers": {
    "yourbrain": {
      "command": "yb",
      "args": ["mcp", "--db-memory", "my-project"]
    }
  }
}
```

**Via a project file:** create **`.trae/mcp.json`** in your workspace root with
the same content (Trae does not create it automatically). You can use
`${workspaceFolder}` in `args` if needed. **Reload the window**
(Command Palette → *Developer: Reload Window*) after editing.

---

## Available tools

Once connected, the agent can call:

| Tool | Purpose |
|---|---|
| `yb_remember` | Store a durable fact (auto conflict check). |
| `yb_recall` | Search & retrieve token-budgeted context. |
| `yb_resolve` | Resolve a detected conflict. |
| `yb_validate` | Fact-check a drafted answer against the KB. |
| `yb_cache_get` / `yb_cache_put` / `yb_cache_clear` | Layered semantic cache. |
| `yb_endorse` / `yb_dispute` | Team consensus on a memory. |
| `yb_timeline` / `yb_get_full` / `yb_stats` | Audit history, full content, health. |

Every tool accepts an optional `db_memory` argument to target a specific named
database, overriding the server default.

---

## Per-project settings

Because each project has its own MCP config, you can tune YourBrain **per
project** by adding launch flags to `yb mcp` — no edits to the shared
`config.toml`. With `yb install` (Cursor/Claude Code) pass them directly; for
VS Code / Trae add them to the `args` array by hand.

```bash
yb install --ide cursor \
  --dynamic-budget --budget 300 \
  --cache-similarity 0.85 --cache-kb-direct 0.80 --cache-kb-grounding 0.50 \
  --embedder onnx --embed-model multilingual-e5-small \
  --conflict-similarity 0.75
```

| Flag (on `yb mcp` / `yb install`) | Effect | Config it overrides |
|---|---|---|
| `--db-memory <name>` | Isolate this project's memories | — |
| `--dynamic-budget true\|false` | Enable/disable dynamic token compression | `[token_budget] enabled` |
| `--budget N` | Recall token budget (`0` = use config) | `[token_budget] max_tokens` / `[recall] max_tokens` |
| `--cache-similarity F` | Tier-1 Q&A cache hit threshold | `[cache] similarity_threshold` |
| `--cache-kb-direct F` | Tier-2 direct-from-KB threshold | `[cache] kb_direct_threshold` |
| `--cache-kb-grounding F` | Tier-3 KB grounding threshold | `[cache] kb_grounding_threshold` |
| `--conflict-similarity F` | Conflict candidate gate (raise to ~0.75 for ONNX) | `[conflict] similarity_threshold` |
| `--embedder local\|onnx` | Embedding backend for this server | `[embedding] provider` |
| `--embed-model <key>` | Embedding model key | `[embedding] model` |

The resulting `.cursor/mcp.json` looks like:

```json
{
  "mcpServers": {
    "yourbrain": {
      "command": "C:\\path\\to\\yourbrain\\target\\release\\yb.exe",
      "args": [
        "mcp", "--db-memory", "my-project",
        "--dynamic-budget", "true", "--budget", "300",
        "--cache-similarity", "0.85",
        "--conflict-similarity", "0.75",
        "--embedder", "onnx", "--embed-model", "multilingual-e5-small"
      ]
    }
  }
}
```

**Precedence (highest wins):** per-call tool argument (e.g. `yb_recall`
`max_tokens` / `dynamic_budget`, `yb_cache_get` `similarity_threshold`) →
server flag in `mcp.json` → `config.toml`.

> If you point the server at `--embedder onnx`, run
> `yb reindex --provider onnx --model <key> --yes` **once** first, with the MCP
> server stopped (see [INSTALL.md](INSTALL.md#optional-onnx-embedder)).

---

## Verify it works

1. Reload the IDE / MCP server.
2. Ask the agent to store and recall something, e.g.
   *"Remember that this project deploys via GitHub Actions"*, then in a new chat
   *"How does this project deploy?"* — the agent should call `yb_recall` and
   answer from memory.
3. Or ask it to run `yb_stats` — it returns memory counts, the active embedding
   model, and the `yb` version.

From a terminal you can cross-check the same database:

```bash
yb --db-memory my-project stats
yb --db-memory my-project list
```

---

## Reloading after changes

Any edit to `mcp.json` / `.trae/mcp.json` / `.vscode/mcp.json`, or a rebuilt
`yb` binary, requires reloading the MCP server:

- **Cursor:** Settings → MCP → toggle the server off/on, or reload the window.
- **Claude Code:** restart the session.
- **VS Code:** restart the server from the CodeLens in `.vscode/mcp.json`.
- **Trae:** Command Palette → *Developer: Reload Window*.
