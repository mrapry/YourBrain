//! Runtime context: resolve the data directory, load config, open the brain.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use yb_core::{Brain, Config};

/// Process-wide default `db_memory`, set once from the CLI `--db-memory` flag.
///
/// This lets the no-argument [`open_brain`] convenience honor the flag without
/// threading the value through every subcommand. The MCP server does not rely on
/// this; it resolves databases explicitly through its own pool.
static DEFAULT_DB: OnceLock<Option<String>> = OnceLock::new();

/// Record the process-wide default `db_memory` (idempotent; first write wins).
pub fn set_default_db(db: Option<String>) {
    let _ = DEFAULT_DB.set(db);
}

fn default_db() -> Option<String> {
    DEFAULT_DB.get().cloned().flatten()
}

/// Process-wide embedder override from the global `--embedder` / `--embed-model`
/// CLI flags. Lets any command target a database that was reindexed to a
/// non-default embedder without editing the shared `config.toml`.
static EMBEDDER_OVERRIDE: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();

/// Record the process-wide embedder override (idempotent; first write wins).
pub fn set_embedder_override(provider: Option<String>, model: Option<String>) {
    let _ = EMBEDDER_OVERRIDE.set((provider, model));
}

fn apply_embedder_override(config: &mut Config) {
    if let Some((provider, model)) = EMBEDDER_OVERRIDE.get() {
        if let Some(p) = provider {
            config.embedding.provider = p.clone();
        }
        if let Some(m) = model {
            config.embedding.model = m.clone();
        }
    }
}

/// Resolve the base data directory holding `config.toml` and, for the global
/// database, `brain.db` / `brain.ybv`.
///
/// Precedence: `YB_DATA_DIR` env var → platform default. On Windows the default
/// is `%APPDATA%\yourbrain`; elsewhere `~/.yourbrain` (see ADR-14).
pub fn data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YB_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        let base = dirs::data_dir().context("cannot determine %APPDATA%")?;
        Ok(base.join("yourbrain"))
    }
    #[cfg(not(windows))]
    {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(home.join(".yourbrain"))
    }
}

/// Sanitize a `db_memory` name into a single safe path component.
///
/// Keeps `[A-Za-z0-9._-]`, replaces every other character with `-`, and rejects
/// names that would escape the data directory (empty, `.`, `..`, or containing a
/// path separator). Returns `None` when the name cannot be made safe.
pub fn sanitize_db_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\\') {
        return None;
    }
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Reject dot-only names that would resolve to the current/parent directory.
    if safe.is_empty() || safe.chars().all(|c| c == '.') {
        return None;
    }
    Some(safe)
}

/// Resolve the directory that stores `brain.db` / `brain.ybv` for the given
/// `db_memory`.
///
/// When `db_memory` is `None` the shared/global database (the base data dir) is
/// used. Otherwise each named memory lives in an isolated `dbs/<name>` subfolder,
/// so its SQLite store, FTS index, vector index, and conflict scope are fully
/// separate from other databases.
pub fn resolve_db_dir(db_memory: Option<&str>) -> Result<PathBuf> {
    let base = data_dir()?;
    match db_memory {
        Some(name) => {
            let safe = sanitize_db_name(name)
                .with_context(|| format!("invalid db_memory name `{name}`"))?;
            Ok(base.join("dbs").join(safe))
        }
        None => Ok(base),
    }
}

/// Directory of the currently active database (honoring the `--db-memory` flag).
pub fn active_db_dir() -> Result<PathBuf> {
    resolve_db_dir(default_db().as_deref())
}

/// Path to the (shared) config file inside the base data directory.
pub fn config_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

/// Load config from `config.toml`, or defaults if the file is absent.
///
/// Config is always read from the base data directory and shared across every
/// `db_memory`; only the memory data is isolated per database.
pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Config::from_toml(&text).with_context(|| format!("parsing {}", path.display()))
    } else {
        Ok(Config::default())
    }
}

/// Open the brain for the process-wide default `db_memory` (set via the
/// `--db-memory` CLI flag, else the global database).
pub fn open_brain() -> Result<Brain> {
    open_brain_with(default_db().as_deref())
}

/// Open the brain for a specific `db_memory` (or the global database when `None`).
pub fn open_brain_with(db_memory: Option<&str>) -> Result<Brain> {
    let dir = resolve_db_dir(db_memory)?;
    let mut config = load_config()?;
    apply_embedder_override(&mut config);
    let brain = Brain::open(&dir, config).context("opening brain")?;
    Ok(brain)
}
