//! SQLite storage layer.
//!
//! Owns the schema, migrations, FTS5 sync triggers (ADR-6), the `brain_meta`
//! dimension lock (ADR-5), the `embed_queue` (ADR-2), the `vector_key_map`
//! (ADR-18), and CRUD for every domain type. All timestamps are stored as
//! RFC3339 text; JSON columns hold arrays/objects.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;

use crate::conflict::{Conflict, ConflictState};
use crate::error::{Result, YbError};
use crate::memory::{
    Edge, EdgeType, Memory, MemoryState, MemoryType, Observation, Scope, Session, SourceType,
};

/// Current schema version. Bump when adding migrations.
pub const SCHEMA_VERSION: i64 = 1;

/// A handle to the on-disk (or in-memory) brain database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) a database at `path`, running migrations and enabling WAL.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    /// Access the raw connection (used by higher layers for custom queries).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ---- brain_meta (embedding lock) ------------------------------------

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row("SELECT value FROM brain_meta WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(v)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO brain_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Initialize the embedding model lock if unset. If already set, validate
    /// that the configured model/dimension match (ADR-5).
    pub fn ensure_embedding_lock(&self, model: &str, dimension: usize) -> Result<()> {
        match self.meta_get("embedding_model")? {
            None => {
                self.meta_set("embedding_model", model)?;
                self.meta_set("embedding_dimension", &dimension.to_string())?;
                self.meta_set("schema_version", &SCHEMA_VERSION.to_string())?;
                self.meta_set("created_at", &Utc::now().to_rfc3339())?;
                Ok(())
            }
            Some(stored_model) => {
                if stored_model != model {
                    return Err(YbError::ModelMismatch {
                        configured: model.to_string(),
                        stored: stored_model,
                        hint: "Run `yb reindex --model <new_model>` to migrate embeddings.".into(),
                    });
                }
                let stored_dim: usize = self
                    .meta_get("embedding_dimension")?
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(dimension);
                if stored_dim != dimension {
                    return Err(YbError::DimensionMismatch {
                        expected: stored_dim,
                        got: dimension,
                    });
                }
                Ok(())
            }
        }
    }

    // ---- memories -------------------------------------------------------

    pub fn insert_memory(&self, m: &Memory) -> Result<()> {
        self.conn.execute(
            "INSERT INTO memories (
                id, content, compressed, summary, headline,
                memory_type, state, scope,
                author, room, tags, entities, source_type, source_detail,
                confidence, importance, access_count, last_accessed,
                created_at, updated_at, verified_at,
                endorsed_by, disputed_by, embedding
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18,
                ?19, ?20, ?21,
                ?22, ?23, ?24
            )",
            params![
                m.id,
                m.content,
                m.compressed,
                m.summary,
                m.headline,
                m.memory_type.as_str(),
                m.state.as_str(),
                m.scope.as_str(),
                m.author,
                m.room,
                json_arr(&m.tags),
                json_arr(&m.entities),
                m.source_type.as_str(),
                m.source_detail,
                m.confidence,
                m.importance,
                m.access_count,
                m.last_accessed.map(|d| d.to_rfc3339()),
                m.created_at.to_rfc3339(),
                m.updated_at.to_rfc3339(),
                m.verified_at.map(|d| d.to_rfc3339()),
                json_arr(&m.endorsed_by),
                json_arr(&m.disputed_by),
                embedding_to_blob(&[]),
            ],
        )?;
        Ok(())
    }

    /// Store the embedding blob for a memory and record the vector_key_map entry.
    pub fn set_embedding(&self, id: &str, embedding: &[f32]) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET embedding = ?2 WHERE id = ?1",
            params![id, embedding_to_blob(embedding)],
        )?;
        self.conn.execute(
            "INSERT INTO vector_key_map(memory_id, embedded_at) VALUES (?1, ?2)
             ON CONFLICT(memory_id) DO UPDATE SET embedded_at = excluded.embedded_at",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_embedding(&self, id: &str) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT embedding FROM memories WHERE id = ?1", [id], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()?;
        Ok(blob.map(|b| blob_to_embedding(&b)))
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let m = self
            .conn
            .query_row(
                &format!("SELECT {MEMORY_COLS} FROM memories WHERE id = ?1"),
                [id],
                row_to_memory,
            )
            .optional()?;
        Ok(m)
    }

    pub fn update_memory(&self, m: &Memory) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE memories SET
                content=?2, compressed=?3, summary=?4, headline=?5,
                memory_type=?6, state=?7, scope=?8,
                author=?9, room=?10, tags=?11, entities=?12, source_type=?13, source_detail=?14,
                confidence=?15, importance=?16, access_count=?17, last_accessed=?18,
                updated_at=?19, verified_at=?20, endorsed_by=?21, disputed_by=?22
             WHERE id=?1",
            params![
                m.id,
                m.content,
                m.compressed,
                m.summary,
                m.headline,
                m.memory_type.as_str(),
                m.state.as_str(),
                m.scope.as_str(),
                m.author,
                m.room,
                json_arr(&m.tags),
                json_arr(&m.entities),
                m.source_type.as_str(),
                m.source_detail,
                m.confidence,
                m.importance,
                m.access_count,
                m.last_accessed.map(|d| d.to_rfc3339()),
                m.updated_at.to_rfc3339(),
                m.verified_at.map(|d| d.to_rfc3339()),
                json_arr(&m.endorsed_by),
                json_arr(&m.disputed_by),
            ],
        )?;
        if affected == 0 {
            return Err(YbError::NotFound(m.id.clone()));
        }
        Ok(())
    }

    pub fn set_state(&self, id: &str, state: MemoryState) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET state=?2, updated_at=?3 WHERE id=?1",
            params![id, state.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn touch_access(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET access_count = access_count + 1, last_accessed = ?2 WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// List memories, optionally filtered by room/tag/state. Newest first.
    pub fn list_memories(
        &self,
        room: Option<&str>,
        state: Option<MemoryState>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        // All named params are always present in the SQL (guarded by IS NULL)
        // so binding them unconditionally is valid regardless of filters.
        let sql = format!(
            "SELECT {MEMORY_COLS} FROM memories
             WHERE (:room IS NULL OR room = :room)
               AND (:state IS NULL OR state = :state)
             ORDER BY created_at DESC LIMIT :limit"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::named_params! {
                ":room": room,
                ":state": state.map(|s| s.as_str()),
                ":limit": limit as i64,
            },
            row_to_memory,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Full-text search returning memory ids ranked best-first.
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let match_query = fts_escape(query);
        if match_query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT m.id FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY bm25(memories_fts) ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_query, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn count_memories(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?)
    }

    pub fn count_by_state(&self, state: MemoryState) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE state = ?1",
            [state.as_str()],
            |r| r.get(0),
        )?)
    }

    // ---- edges ----------------------------------------------------------

    pub fn insert_edge(&self, e: &Edge) -> Result<()> {
        self.conn.execute(
            "INSERT INTO edges(id, source_id, target_id, edge_type, created_at, created_by)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                e.id,
                e.source_id,
                e.target_id,
                e.edge_type.as_str(),
                e.created_at.to_rfc3339(),
                e.created_by
            ],
        )?;
        Ok(())
    }

    pub fn edges_for(&self, memory_id: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, target_id, edge_type, created_at, created_by
             FROM edges WHERE source_id = ?1 OR target_id = ?1",
        )?;
        let rows = stmt.query_map([memory_id], |r| {
            Ok(Edge {
                id: r.get(0)?,
                source_id: r.get(1)?,
                target_id: r.get(2)?,
                edge_type: EdgeType::from_str_row(r, 3)?,
                created_at: parse_dt_row(r, 4)?,
                created_by: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ---- sessions & observations ---------------------------------------

    pub fn insert_session(&self, s: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions(id, ide, cwd, room, started_at, ended_at, metadata)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                s.id,
                s.ide,
                s.cwd,
                s.room,
                s.started_at.to_rfc3339(),
                s.ended_at.map(|d| d.to_rfc3339()),
                s.metadata
            ],
        )?;
        Ok(())
    }

    pub fn close_session(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn insert_observation(&self, o: &Observation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO observations(id, session_id, kind, content, compressed, created_at, metadata)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                o.id,
                o.session_id,
                o.kind,
                o.content,
                o.compressed,
                o.created_at.to_rfc3339(),
                o.metadata
            ],
        )?;
        Ok(())
    }

    pub fn session_observations(&self, session_id: &str) -> Result<Vec<Observation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, kind, content, compressed, created_at, metadata
             FROM observations WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(Observation {
                id: r.get(0)?,
                session_id: r.get(1)?,
                kind: r.get(2)?,
                content: r.get(3)?,
                compressed: r.get(4)?,
                created_at: parse_dt_row(r, 5)?,
                metadata: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ---- embed queue (ADR-2) -------------------------------------------

    pub fn enqueue_embed(
        &self,
        target_type: &str,
        target_id: &str,
        content: &str,
        priority: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO embed_queue(target_type, target_id, content, priority, status, created_at)
             VALUES (?1,?2,?3,?4,'pending',?5)",
            params![
                target_type,
                target_id,
                content,
                priority,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Fetch up to `n` pending items ordered by priority.
    pub fn pending_embeds(&self, n: usize) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, target_type, target_id, content FROM embed_queue
             WHERE status = 'pending' ORDER BY priority DESC, id ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map([n as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn mark_embed_done(&self, queue_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status='done', processed_at=?2 WHERE id=?1",
            params![queue_id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // ---- conflicts ------------------------------------------------------

    pub fn insert_conflict(&self, c: &Conflict) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conflicts(
                id, new_memory_json, existing_memory_ids, analysis_json,
                state, resolution_json, created_at, expires_at, resolved_at, resolved_by
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                c.id,
                serde_json::to_string(&c.new_memory)?,
                serde_json::to_string(&c.existing_memory_ids)?,
                serde_json::to_string(&c.analysis)?,
                c.state.as_str(),
                c.resolution
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                c.created_at.to_rfc3339(),
                c.expires_at.to_rfc3339(),
                c.resolved_at.map(|d| d.to_rfc3339()),
                c.resolved_by,
            ],
        )?;
        Ok(())
    }

    pub fn get_conflict(&self, id: &str) -> Result<Option<Conflict>> {
        let c = self
            .conn
            .query_row(
                "SELECT id, new_memory_json, existing_memory_ids, analysis_json,
                        state, resolution_json, created_at, expires_at, resolved_at, resolved_by
                 FROM conflicts WHERE id = ?1",
                [id],
                row_to_conflict,
            )
            .optional()?;
        Ok(c)
    }

    pub fn list_conflicts(&self, state: Option<ConflictState>) -> Result<Vec<Conflict>> {
        let sql = "SELECT id, new_memory_json, existing_memory_ids, analysis_json,
                          state, resolution_json, created_at, expires_at, resolved_at, resolved_by
                   FROM conflicts
                   WHERE (:state IS NULL OR state = :state)
                   ORDER BY created_at DESC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(
            rusqlite::named_params! { ":state": state.map(|s| s.as_str()) },
            row_to_conflict,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn update_conflict(&self, c: &Conflict) -> Result<()> {
        self.conn.execute(
            "UPDATE conflicts SET state=?2, resolution_json=?3, resolved_at=?4, resolved_by=?5 WHERE id=?1",
            params![
                c.id,
                c.state.as_str(),
                c.resolution.as_ref().map(serde_json::to_string).transpose()?,
                c.resolved_at.map(|d| d.to_rfc3339()),
                c.resolved_by,
            ],
        )?;
        Ok(())
    }

    pub fn count_pending_conflicts(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM conflicts WHERE state = 'pending'",
            [],
            |r| r.get(0),
        )?)
    }

    // ---- timeline -------------------------------------------------------

    pub fn timeline_add(
        &self,
        memory_id: &str,
        event_type: &str,
        detail: Option<&str>,
        actor: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO timeline(id, memory_id, event_type, detail, actor, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                ulid::Ulid::new().to_string(),
                memory_id,
                event_type,
                detail,
                actor,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn timeline_for(&self, memory_id: &str, limit: usize) -> Result<Vec<TimelineEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_type, detail, actor, created_at FROM timeline
             WHERE memory_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![memory_id, limit as i64], |r| {
            Ok(TimelineEvent {
                event_type: r.get(0)?,
                detail: r.get(1)?,
                actor: r.get(2)?,
                created_at: parse_dt_row(r, 3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// A single audit-log entry for a memory.
#[derive(Debug, Clone)]
pub struct TimelineEvent {
    pub event_type: String,
    pub detail: Option<String>,
    pub actor: String,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

const MEMORY_COLS: &str = "id, content, compressed, summary, headline,
    memory_type, state, scope, author, room, tags, entities, source_type, source_detail,
    confidence, importance, access_count, last_accessed, created_at, updated_at, verified_at,
    endorsed_by, disputed_by";

fn row_to_memory(r: &Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?,
        content: r.get(1)?,
        compressed: r.get(2)?,
        summary: r.get(3)?,
        headline: r.get(4)?,
        memory_type: MemoryType::from_str_row(r, 5)?,
        state: MemoryState::from_str_row(r, 6)?,
        scope: Scope::from_str_row(r, 7)?,
        author: r.get(8)?,
        room: r.get(9)?,
        tags: parse_json_arr(r, 10)?,
        entities: parse_json_arr(r, 11)?,
        source_type: SourceType::from_str_row(r, 12)?,
        source_detail: r.get(13)?,
        confidence: r.get(14)?,
        importance: r.get(15)?,
        access_count: r.get(16)?,
        last_accessed: parse_opt_dt_row(r, 17)?,
        created_at: parse_dt_row(r, 18)?,
        updated_at: parse_dt_row(r, 19)?,
        verified_at: parse_opt_dt_row(r, 20)?,
        endorsed_by: parse_json_arr(r, 21)?,
        disputed_by: parse_json_arr(r, 22)?,
    })
}

fn row_to_conflict(r: &Row) -> rusqlite::Result<Conflict> {
    let new_memory = serde_json::from_str(&r.get::<_, String>(1)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let existing_memory_ids = serde_json::from_str(&r.get::<_, String>(2)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let analysis = serde_json::from_str(&r.get::<_, String>(3)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let resolution = match r.get::<_, Option<String>>(5)? {
        Some(s) => Some(serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };
    Ok(Conflict {
        id: r.get(0)?,
        new_memory,
        existing_memory_ids,
        analysis,
        state: conflict_state_from(&r.get::<_, String>(4)?),
        resolution,
        created_at: parse_dt_row(r, 6)?,
        expires_at: parse_dt_row(r, 7)?,
        resolved_at: parse_opt_dt_row(r, 8)?,
        resolved_by: r.get(9)?,
    })
}

fn conflict_state_from(s: &str) -> ConflictState {
    match s {
        "resolved" => ConflictState::Resolved,
        "expired" => ConflictState::Expired,
        "auto_resolved" => ConflictState::AutoResolved,
        _ => ConflictState::Pending,
    }
}

fn json_arr(items: &[String]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
}

fn parse_json_arr(r: &Row, idx: usize) -> rusqlite::Result<Vec<String>> {
    let s: Option<String> = r.get(idx)?;
    match s {
        Some(s) if !s.is_empty() => Ok(serde_json::from_str(&s).unwrap_or_default()),
        _ => Ok(vec![]),
    }
}

fn parse_dt_row(r: &Row, idx: usize) -> rusqlite::Result<DateTime<Utc>> {
    let s: String = r.get(idx)?;
    parse_dt(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_opt_dt_row(r: &Row, idx: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let s: Option<String> = r.get(idx)?;
    match s {
        Some(s) => Ok(Some(parse_dt(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })?)),
        None => Ok(None),
    }
}

fn parse_dt(s: &str) -> std::result::Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc))
}

fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn blob_to_embedding(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Escape an FTS5 query by quoting each term, avoiding syntax errors from
/// arbitrary user input while still matching all terms.
fn fts_escape(query: &str) -> String {
    query
        .split_whitespace()
        .map(|t| {
            let cleaned = t.replace('"', "");
            format!("\"{cleaned}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

// Helpers to parse enums from a row column.
trait FromRowStr: Sized {
    fn from_str_row(r: &Row, idx: usize) -> rusqlite::Result<Self>;
}

macro_rules! impl_from_row_str {
    ($t:ty) => {
        impl FromRowStr for $t {
            fn from_str_row(r: &Row, idx: usize) -> rusqlite::Result<Self> {
                let s: String = r.get(idx)?;
                s.parse::<$t>().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        idx,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e.to_string(),
                        )),
                    )
                })
            }
        }
    };
}

impl_from_row_str!(MemoryType);
impl_from_row_str!(MemoryState);
impl_from_row_str!(Scope);
impl_from_row_str!(SourceType);
impl_from_row_str!(EdgeType);

/// The complete schema, applied idempotently on open.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS brain_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    compressed TEXT NOT NULL,
    summary TEXT NOT NULL,
    headline TEXT NOT NULL,
    memory_type TEXT NOT NULL DEFAULT 'fact',
    state TEXT NOT NULL DEFAULT 'active',
    scope TEXT NOT NULL DEFAULT 'personal',
    author TEXT NOT NULL,
    room TEXT,
    tags TEXT,
    entities TEXT,
    source_type TEXT NOT NULL,
    source_detail TEXT,
    confidence REAL NOT NULL DEFAULT 0.8,
    importance REAL NOT NULL DEFAULT 0.5,
    access_count INTEGER NOT NULL DEFAULT 0,
    last_accessed TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    verified_at TEXT,
    endorsed_by TEXT DEFAULT '[]',
    disputed_by TEXT DEFAULT '[]',
    embedding BLOB
);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, summary, tags, entities,
    content='memories', content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, summary, tags, entities)
    VALUES (new.rowid, new.content, new.summary, new.tags, new.entities);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, tags, entities)
    VALUES ('delete', old.rowid, old.content, old.summary, old.tags, old.entities);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, tags, entities)
    VALUES ('delete', old.rowid, old.content, old.summary, old.tags, old.entities);
    INSERT INTO memories_fts(rowid, content, summary, tags, entities)
    VALUES (new.rowid, new.content, new.summary, new.tags, new.entities);
END;

CREATE TABLE IF NOT EXISTS vector_key_map (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    embedded_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS embed_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    content TEXT NOT NULL,
    priority INTEGER DEFAULT 0,
    status TEXT DEFAULT 'pending',
    created_at TEXT NOT NULL,
    processed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_embed_queue_status ON embed_queue(status, priority DESC);

CREATE TABLE IF NOT EXISTS edges (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES memories(id),
    target_id TEXT NOT NULL REFERENCES memories(id),
    edge_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    ide TEXT NOT NULL,
    cwd TEXT,
    room TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    metadata TEXT
);

CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    compressed TEXT NOT NULL,
    created_at TEXT NOT NULL,
    metadata TEXT
);

CREATE TABLE IF NOT EXISTS conflicts (
    id TEXT PRIMARY KEY,
    new_memory_json TEXT NOT NULL,
    existing_memory_ids TEXT NOT NULL,
    analysis_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    resolution_json TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    resolved_at TEXT,
    resolved_by TEXT
);

CREATE TABLE IF NOT EXISTS timeline (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    detail TEXT,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_state ON memories(state);
CREATE INDEX IF NOT EXISTS idx_memories_room ON memories(room);
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_author ON memories(author);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
CREATE INDEX IF NOT EXISTS idx_observations_session ON observations(session_id);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);
CREATE INDEX IF NOT EXISTS idx_conflicts_state ON conflicts(state);
CREATE INDEX IF NOT EXISTS idx_timeline_memory ON timeline(memory_id);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_memory;

    #[test]
    fn open_and_meta() {
        let s = Store::open_in_memory().unwrap();
        s.ensure_embedding_lock("hash-bow-v1", 256).unwrap();
        assert_eq!(
            s.meta_get("embedding_model").unwrap().as_deref(),
            Some("hash-bow-v1")
        );
        // Same model is fine.
        s.ensure_embedding_lock("hash-bow-v1", 256).unwrap();
        // Different model errors.
        assert!(s.ensure_embedding_lock("other", 256).is_err());
    }

    #[test]
    fn insert_get_update_memory() {
        let s = Store::open_in_memory().unwrap();
        let mut m = sample_memory("auth uses JWT and Redis");
        s.insert_memory(&m).unwrap();
        let got = s.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.content, m.content);

        m.state = MemoryState::Archived;
        s.update_memory(&m).unwrap();
        assert_eq!(
            s.get_memory(&m.id).unwrap().unwrap().state,
            MemoryState::Archived
        );
    }

    #[test]
    fn fts_search_finds_memory() {
        let s = Store::open_in_memory().unwrap();
        let m = sample_memory("authentication uses JWT tokens stored in Redis");
        s.insert_memory(&m).unwrap();
        let ids = s.search_fts("JWT Redis", 5).unwrap();
        assert!(ids.contains(&m.id), "fts did not find memory: {ids:?}");
    }

    #[test]
    fn embedding_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let m = sample_memory("hello");
        s.insert_memory(&m).unwrap();
        s.set_embedding(&m.id, &[0.1, 0.2, 0.3]).unwrap();
        let got = s.get_embedding(&m.id).unwrap().unwrap();
        assert_eq!(got.len(), 3);
        assert!((got[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn embed_queue_flow() {
        let s = Store::open_in_memory().unwrap();
        s.enqueue_embed("memory", "01ABC", "some text", 0).unwrap();
        let pending = s.pending_embeds(10).unwrap();
        assert_eq!(pending.len(), 1);
        s.mark_embed_done(pending[0].0).unwrap();
        assert!(s.pending_embeds(10).unwrap().is_empty());
    }
}
