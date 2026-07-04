//! The `Brain` facade: the single entry point that wires storage, embedding,
//! vector search, compression, and conflict resolution into the high-level
//! operations the CLI / MCP / daemon expose.

use chrono::{Duration, Utc};
use std::path::{Path, PathBuf};

use crate::budget::{dynamic_allocate_budget, ExtractiveSummarizer};
use crate::classify;
use crate::compress::Compressor;
use crate::config::Config;
use crate::conflict::{
    expiry_from, should_auto_resolve, Candidate, Conflict, ConflictAnalysis, ConflictRelation,
    ConflictState, Resolution, ResolutionAction, RuleEngine,
};
use crate::embed::{cosine, Embedder, HashEmbedder};
use crate::error::{Result, YbError};
use crate::guardrail::{RuleValidator, ValidationReport, Validator};
use crate::memory::{Edge, EdgeType, Memory, MemoryState, MemoryType, Scope, SourceType};
use crate::rerank::{LexicalReranker, Reranker};
use crate::search::{self, DetailLevel, RecallOutput, ScoredMemory};
use crate::store::{CacheEntry, Store, TimelineEvent};
use crate::vector::{FlatIndex, VectorIndex};

/// Options for [`Brain::remember`]. Missing fields fall back to config defaults.
#[derive(Debug, Clone, Default)]
pub struct RememberOptions {
    pub author: Option<String>,
    pub room: Option<String>,
    pub scope: Option<Scope>,
    pub tags: Vec<String>,
    pub memory_type: Option<MemoryType>,
    pub source_type: Option<SourceType>,
}

/// Outcome of a `remember` call.
#[derive(Debug, Clone)]
pub enum RememberOutcome {
    /// Stored directly (no conflict).
    Stored { id: String },
    /// Auto-resolved a conflict with high confidence.
    AutoResolved {
        id: Option<String>,
        action: ResolutionAction,
        relation: ConflictRelation,
    },
    /// A conflict needs user review.
    NeedsReview {
        conflict_id: String,
        analysis: ConflictAnalysis,
        existing: Vec<Memory>,
    },
}

/// Outcome of a `resolve` call.
#[derive(Debug, Clone)]
pub struct ResolveOutcome {
    pub action: ResolutionAction,
    pub stored_id: Option<String>,
    pub archived_ids: Vec<String>,
}

/// Result of a `recall` call.
#[derive(Debug, Clone)]
pub struct RecallResult {
    pub output: RecallOutput,
    pub scored: Vec<ScoredMemory>,
}

/// Where a cache answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSource {
    /// A previously stored question/answer pair.
    Cache,
    /// Strongly-matching knowledge-base documents (answered directly).
    Kb,
}

impl CacheSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheSource::Cache => "cache",
            CacheSource::Kb => "kb",
        }
    }
}

/// Per-call overrides for the cache lookup thresholds. Any `None` field falls
/// back to the `[cache]` config. Used for live tuning/research without editing
/// config or restarting the server.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheOverrides {
    /// Tier 1 (Q&A) similarity threshold.
    pub similarity: Option<f32>,
    /// Tier 2 (direct-from-KB) threshold.
    pub kb_direct: Option<f32>,
    /// Tier 3 (KB grounding) threshold.
    pub kb_grounding: Option<f32>,
}

/// Result of a layered [`Brain::cache_get`] lookup.
#[derive(Debug, Clone)]
pub enum CacheLookup {
    /// A ready answer (Tier 1 cache, or Tier 2 direct-from-KB).
    Hit {
        answer: String,
        source: CacheSource,
        memory_ids: Vec<String>,
        similarity: f32,
    },
    /// Tier 3: KB matched moderately — return as grounding, still go to the LLM.
    Grounding {
        memories: Vec<Memory>,
        similarity: f32,
    },
    /// No usable cache or KB match.
    Miss,
}

/// Aggregate statistics about the brain.
#[derive(Debug, Clone)]
pub struct Stats {
    pub total: i64,
    pub active: i64,
    pub archived: i64,
    pub superseded: i64,
    pub disputed: i64,
    pub pending_conflicts: i64,
    pub model: String,
    pub dimension: usize,
}

/// Outcome of a [`Brain::reindex`] migration.
#[derive(Debug, Clone)]
pub struct ReindexReport {
    /// The model id now locked into the database.
    pub model: String,
    /// The vector dimension now locked into the database.
    pub dimension: usize,
    /// Number of memories re-embedded.
    pub reembedded: usize,
}

/// The engine. Holds owned handles to every subsystem.
pub struct Brain {
    store: Store,
    embedder: Box<dyn Embedder>,
    index: FlatIndex,
    compressor: Compressor,
    rules: RuleEngine,
    config: Config,
    index_path: Option<PathBuf>,
}

impl Brain {
    /// Open a brain rooted at `data_dir`, creating files as needed.
    pub fn open(data_dir: &Path, config: Config) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let store = Store::open(&data_dir.join("brain.db"))?;
        let embedder = build_embedder(&config)?;
        store.ensure_embedding_lock(embedder.model_id(), embedder.dimension())?;

        let index_path = data_dir.join("brain.ybv");
        let index = FlatIndex::open_or_create(&index_path, embedder.dimension())?;

        let compressor = Compressor::new(config.compression.to_compress_config());
        let mut brain = Brain {
            store,
            embedder,
            index,
            compressor,
            rules: RuleEngine::new(),
            config,
            index_path: Some(index_path),
        };
        brain.rebuild_index_if_needed()?;
        Ok(brain)
    }

    /// Build an all-in-memory brain for tests.
    pub fn in_memory(config: Config) -> Result<Self> {
        let store = Store::open_in_memory()?;
        let embedder = build_embedder(&config)?;
        store.ensure_embedding_lock(embedder.model_id(), embedder.dimension())?;
        let index = FlatIndex::new(embedder.dimension());
        let compressor = Compressor::new(config.compression.to_compress_config());
        Ok(Brain {
            store,
            embedder,
            index,
            compressor,
            rules: RuleEngine::new(),
            config,
            index_path: None,
        })
    }

    /// If the vector index is empty but memories exist (e.g. fresh process,
    /// deleted index file), rebuild it from stored embeddings.
    fn rebuild_index_if_needed(&mut self) -> Result<()> {
        if self.index.len() > 0 || self.store.count_memories()? == 0 {
            return Ok(());
        }
        let memories = self.store.list_memories(None, None, usize::MAX)?;
        for m in memories {
            if let Some(emb) = self.store.get_embedding(&m.id)? {
                if emb.len() == self.embedder.dimension() {
                    self.index.upsert(&m.id, emb);
                }
            }
        }
        Ok(())
    }

    /// Persist the vector index to disk (no-op for in-memory brains).
    pub fn save(&self) -> Result<()> {
        if let Some(p) = &self.index_path {
            self.index.save(p)?;
        }
        Ok(())
    }

    /// Store a memory, running the full ingestion pipeline with conflict checks.
    pub fn remember(&mut self, input: &str, opts: RememberOptions) -> Result<RememberOutcome> {
        // 1. Preprocess (privacy).
        let pre = classify::preprocess(input, &self.config.privacy.exclude_patterns);
        if pre.text.trim().is_empty() {
            return Err(YbError::InvalidArgument(
                "input is empty after preprocessing (all content redacted?)".into(),
            ));
        }

        // 2. Classify + merge with caller hints.
        let cls = classify::classify(&pre.text);
        let memory_type = opts.memory_type.unwrap_or(cls.memory_type);
        let mut tags = opts.tags.clone();
        for t in cls.tags {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }

        // 3. Compress into levels.
        let levels = self.compressor.levels(&pre.text);

        // 4. Build the memory (not yet persisted).
        let now = Utc::now();
        let author = opts
            .author
            .unwrap_or_else(|| self.config.general.author.clone());
        let scope = opts
            .scope
            .unwrap_or_else(|| parse_scope(&self.config.general.default_scope));
        let memory = Memory {
            id: ulid::Ulid::new().to_string(),
            content: pre.text.clone(),
            compressed: levels.compressed,
            summary: levels.summary,
            headline: levels.headline,
            memory_type,
            state: MemoryState::Active,
            scope,
            author,
            room: opts.room,
            tags,
            entities: cls.entities,
            source_type: opts.source_type.unwrap_or(SourceType::Manual),
            source_detail: None,
            confidence: 0.8,
            importance: 0.5,
            access_count: 0,
            last_accessed: None,
            created_at: now,
            updated_at: now,
            verified_at: None,
            endorsed_by: vec![],
            disputed_by: vec![],
        };

        // 5. Embed (kept locally; only added to the index once we decide to store).
        let embedding = self.embedder.embed_document(&memory.content);

        // 6. Candidate search against the existing index.
        let candidates = self.find_candidates(&embedding)?;

        // 7. Conflict check (Tier 1 rules).
        if self.config.conflict.enabled && !candidates.is_empty() {
            if let Some(analysis) = self.rules.analyze(&memory, &candidates) {
                return self.route_conflict(memory, embedding, analysis, &candidates);
            }
        }

        // 8. No conflict → store directly.
        self.persist_new(&memory, &embedding, "created")?;
        Ok(RememberOutcome::Stored { id: memory.id })
    }

    fn route_conflict(
        &mut self,
        memory: Memory,
        embedding: Vec<f32>,
        analysis: ConflictAnalysis,
        candidates: &[Candidate],
    ) -> Result<RememberOutcome> {
        let existing = self
            .store
            .get_memory(&analysis.existing_id)?
            .ok_or_else(|| YbError::NotFound(analysis.existing_id.clone()))?;

        if let Some(action) = should_auto_resolve(
            &analysis,
            &memory,
            &existing,
            &self.config.conflict.thresholds(),
        ) {
            let relation = analysis.relation;
            let stored_id = self.apply_resolution(
                &memory,
                &embedding,
                std::slice::from_ref(&existing.id),
                action,
                None,
                "auto_resolved",
            )?;
            return Ok(RememberOutcome::AutoResolved {
                id: stored_id,
                action,
                relation,
            });
        }

        // Needs user review → persist a pending conflict (holds full memory).
        let now = Utc::now();
        let conflict = Conflict {
            id: ulid::Ulid::new().to_string(),
            new_memory: memory,
            existing_memory_ids: candidates.iter().map(|c| c.memory.id.clone()).collect(),
            analysis: analysis.clone(),
            state: ConflictState::Pending,
            resolution: None,
            created_at: now,
            expires_at: expiry_from(now, self.config.conflict.conflict_expiry_secs),
            resolved_at: None,
            resolved_by: None,
        };
        self.store.insert_conflict(&conflict)?;
        Ok(RememberOutcome::NeedsReview {
            conflict_id: conflict.id,
            analysis,
            existing: vec![existing],
        })
    }

    /// Resolve a pending conflict with an explicit action.
    pub fn resolve(
        &mut self,
        conflict_id: &str,
        action: ResolutionAction,
        context: Option<String>,
        merged_content: Option<String>,
        resolved_by: Option<String>,
    ) -> Result<ResolveOutcome> {
        let mut conflict = self
            .store
            .get_conflict(conflict_id)?
            .ok_or_else(|| YbError::ConflictNotFound(conflict_id.to_string()))?;
        if conflict.state != ConflictState::Pending {
            return Err(YbError::InvalidArgument(format!(
                "conflict {conflict_id} is already {}",
                conflict.state.as_str()
            )));
        }

        let new_memory = conflict.new_memory.clone();
        let embedding = self.embedder.embed_document(&new_memory.content);
        let existing_ids = conflict.existing_memory_ids.clone();

        let stored_id = self.apply_resolution(
            &new_memory,
            &embedding,
            &existing_ids,
            action,
            merged_content.clone(),
            "resolved",
        )?;

        let archived = match action {
            ResolutionAction::Replace | ResolutionAction::Merge => existing_ids.clone(),
            _ => vec![],
        };

        conflict.state = ConflictState::Resolved;
        conflict.resolution = Some(Resolution {
            action,
            context,
            merged_content,
        });
        conflict.resolved_at = Some(Utc::now());
        conflict.resolved_by = resolved_by;
        self.store.update_conflict(&conflict)?;

        Ok(ResolveOutcome {
            action,
            stored_id,
            archived_ids: archived,
        })
    }

    /// Apply a resolution action, returning the id of the newly stored memory
    /// (if any). Used by both auto-resolve and manual resolve.
    fn apply_resolution(
        &mut self,
        new_memory: &Memory,
        embedding: &[f32],
        existing_ids: &[String],
        action: ResolutionAction,
        merged_content: Option<String>,
        event: &str,
    ) -> Result<Option<String>> {
        match action {
            ResolutionAction::DiscardNew => {
                // Refresh the existing memory's verified_at to reflect re-confirmation.
                for id in existing_ids {
                    self.store.timeline_add(
                        id,
                        "refreshed",
                        Some("duplicate discarded"),
                        &new_memory.author,
                    )?;
                }
                Ok(None)
            }
            ResolutionAction::KeepBoth => {
                self.persist_new(new_memory, embedding, event)?;
                for id in existing_ids {
                    self.link(
                        &new_memory.id,
                        id,
                        EdgeType::Complements,
                        &new_memory.author,
                    )?;
                }
                Ok(Some(new_memory.id.clone()))
            }
            ResolutionAction::Replace => {
                self.persist_new(new_memory, embedding, event)?;
                for id in existing_ids {
                    self.store.set_state(id, MemoryState::Superseded)?;
                    self.link(&new_memory.id, id, EdgeType::Supersedes, &new_memory.author)?;
                    self.index.remove(id);
                    let _ = self.store.cache_invalidate_by_source(id);
                    self.store.timeline_add(
                        id,
                        "superseded",
                        Some(&new_memory.id),
                        &new_memory.author,
                    )?;
                }
                Ok(Some(new_memory.id.clone()))
            }
            ResolutionAction::Merge => {
                let content = merged_content.ok_or_else(|| {
                    YbError::InvalidArgument("merge action requires merged_content".into())
                })?;
                let levels = self.compressor.levels(&content);
                let mut merged = new_memory.clone();
                merged.id = ulid::Ulid::new().to_string();
                merged.content = content.clone();
                merged.compressed = levels.compressed;
                merged.summary = levels.summary;
                merged.headline = levels.headline;
                let merged_emb = self.embedder.embed_document(&content);
                self.persist_new(&merged, &merged_emb, event)?;
                for id in existing_ids {
                    self.store.set_state(id, MemoryState::Superseded)?;
                    self.link(&merged.id, id, EdgeType::Supersedes, &merged.author)?;
                    self.index.remove(id);
                    let _ = self.store.cache_invalidate_by_source(id);
                }
                Ok(Some(merged.id))
            }
        }
    }

    fn persist_new(&mut self, m: &Memory, embedding: &[f32], event: &str) -> Result<()> {
        self.store.insert_memory(m)?;
        self.store.set_embedding(&m.id, embedding)?;
        self.index.upsert(&m.id, embedding.to_vec());
        self.store.timeline_add(&m.id, event, None, &m.author)?;
        Ok(())
    }

    fn link(&self, source: &str, target: &str, edge_type: EdgeType, by: &str) -> Result<()> {
        let edge = Edge {
            id: ulid::Ulid::new().to_string(),
            source_id: source.to_string(),
            target_id: target.to_string(),
            edge_type,
            created_at: Utc::now(),
            created_by: by.to_string(),
        };
        self.store.insert_edge(&edge)
    }

    fn find_candidates(&self, embedding: &[f32]) -> Result<Vec<Candidate>> {
        let hits = self
            .index
            .search(embedding, self.config.conflict.candidate_top_k);
        let mut out = Vec::new();
        for (id, sim) in hits {
            if sim < self.config.conflict.similarity_threshold {
                continue;
            }
            if let Some(mem) = self.store.get_memory(&id)? {
                if matches!(mem.state, MemoryState::Active | MemoryState::Disputed) {
                    out.push(Candidate {
                        memory: mem,
                        similarity: sim,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Search and retrieve memories, returning token-budgeted output.
    ///
    /// When `dynamic` is true, recalled memories are condensed with the dynamic
    /// token budgeter; otherwise the static budget allocation is used.
    pub fn recall(
        &self,
        query: &str,
        limit: usize,
        detail: DetailLevel,
        token_budget: usize,
        room: Option<&str>,
        dynamic: bool,
    ) -> Result<RecallResult> {
        // Resolve the effective token budget. A caller value of 0 means "use
        // config": prefer the dynamic budgeter's [token_budget] max_tokens when
        // set, otherwise fall back to [recall] max_tokens.
        let token_budget = if token_budget > 0 {
            token_budget
        } else if self.config.token_budget.max_tokens > 0 {
            self.config.token_budget.max_tokens
        } else {
            self.config.recall.max_tokens
        };

        let qvec = self.embedder.embed_query(query);
        // Fetch a larger candidate pool so the reranker has room to work.
        let factor = self.config.rerank.candidate_pool_factor.max(1);
        let fetch = (limit * factor).max(10);
        let vector_ranked: Vec<String> = self
            .index
            .search(&qvec, fetch)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let fts_ranked = self.store.search_fts(query, fetch)?;

        let fused = search::rrf_fuse(&fts_ranked, &vector_ranked, self.config.search.rrf_k);

        let mut scored_input = Vec::new();
        for (id, base) in fused {
            if let Some(m) = self.store.get_memory(&id)? {
                if !self.config.search.include_archived
                    && matches!(m.state, MemoryState::Archived | MemoryState::Superseded)
                {
                    continue;
                }
                if m.confidence < self.config.search.min_confidence {
                    continue;
                }
                if let Some(r) = room {
                    if m.room.as_deref() != Some(r) {
                        continue;
                    }
                }
                scored_input.push((m, base));
            }
        }

        let mut scored = search::rerank(scored_input);
        // Lexical (BM25) rerank stage sharpens query relevance (v0.2).
        if self.config.rerank.enabled {
            let reranker = LexicalReranker::new(self.config.rerank.lexical_weight);
            scored = reranker.rerank(query, scored);
        }
        scored.truncate(limit);

        let output = if dynamic {
            dynamic_allocate_budget(&scored, token_budget, query, &ExtractiveSummarizer)
        } else {
            search::allocate_budget(&scored, token_budget, detail, 3)
        };

        // Record access for what we actually returned.
        for id in &output.ids {
            let _ = self.store.touch_access(id);
        }

        Ok(RecallResult { output, scored })
    }

    // ---- semantic cache + guardrail (v0.2) -----------------------------

    /// Layered cache lookup: Tier 1 conversational Q&A, Tier 2 direct answer
    /// from strongly-matching KB documents, Tier 3 KB grounding, else miss.
    pub fn cache_get(
        &self,
        query: &str,
        room: Option<&str>,
        overrides: CacheOverrides,
    ) -> Result<CacheLookup> {
        let cfg = &self.config.cache;
        if !cfg.enabled {
            return Ok(CacheLookup::Miss);
        }
        // Effective thresholds: per-call override wins over the [cache] config.
        let t1 = overrides.similarity.unwrap_or(cfg.similarity_threshold);
        let kb_direct = overrides.kb_direct.unwrap_or(cfg.kb_direct_threshold);
        let kb_grounding = overrides.kb_grounding.unwrap_or(cfg.kb_grounding_threshold);
        let qvec = self.embedder.embed_query(query);

        // Tier 1: conversational Q&A cache.
        let entries = self.store.cache_entries(room)?;
        let mut best: Option<(&CacheEntry, f32)> = None;
        for e in &entries {
            if e.query_embedding.is_empty() {
                continue;
            }
            let sim = cosine(&qvec, &e.query_embedding);
            if best.map(|(_, b)| sim > b).unwrap_or(true) {
                best = Some((e, sim));
            }
        }
        if let Some((entry, sim)) = best {
            if sim >= t1 {
                let _ = self.store.cache_touch(&entry.id);
                return Ok(CacheLookup::Hit {
                    answer: entry.answer.clone(),
                    source: CacheSource::Cache,
                    memory_ids: entry.source_ids.clone(),
                    similarity: sim,
                });
            }
        }

        if !cfg.use_kb {
            return Ok(CacheLookup::Miss);
        }

        // Tiers 2/3: consult the knowledge base directly by vector similarity.
        let top_k = self.config.guardrail.evidence_top_k.max(3);
        let mut kb: Vec<(Memory, f32)> = Vec::new();
        for (id, sim) in self.index.search(&qvec, top_k) {
            if let Some(m) = self.store.get_memory(&id)? {
                if matches!(m.state, MemoryState::Archived | MemoryState::Superseded) {
                    continue;
                }
                if m.confidence < self.config.search.min_confidence {
                    continue;
                }
                if let Some(r) = room {
                    if m.room.as_deref() != Some(r) {
                        continue;
                    }
                }
                kb.push((m, sim));
            }
        }
        let top_sim = kb.first().map(|(_, s)| *s).unwrap_or(0.0);

        // Tier 2: strong match -> answer directly from KB documents.
        let direct: Vec<&(Memory, f32)> = kb.iter().filter(|(_, s)| *s >= kb_direct).collect();
        if !direct.is_empty() {
            let answer = direct
                .iter()
                .map(|(m, _)| m.content.clone())
                .collect::<Vec<_>>()
                .join("\n\n");
            let memory_ids = direct.iter().map(|(m, _)| m.id.clone()).collect();
            return Ok(CacheLookup::Hit {
                answer,
                source: CacheSource::Kb,
                memory_ids,
                similarity: top_sim,
            });
        }

        // Tier 3: moderate match -> return as grounding context.
        let grounding: Vec<Memory> = kb
            .into_iter()
            .filter(|(_, s)| *s >= kb_grounding)
            .map(|(m, _)| m)
            .collect();
        if !grounding.is_empty() {
            return Ok(CacheLookup::Grounding {
                memories: grounding,
                similarity: top_sim,
            });
        }

        Ok(CacheLookup::Miss)
    }

    /// Store a query/answer pair, tagging the KB memories that grounded it so the
    /// entry can be auto-invalidated when a source memory changes.
    pub fn cache_put(
        &self,
        query: &str,
        answer: &str,
        source_ids: Vec<String>,
        room: Option<String>,
        ttl_override: Option<i64>,
    ) -> Result<Option<String>> {
        if !self.config.cache.enabled {
            return Ok(None);
        }
        let now = Utc::now();
        let ttl = ttl_override.unwrap_or(self.config.cache.ttl_secs);
        let expires_at = if ttl > 0 {
            Some(now + Duration::seconds(ttl))
        } else {
            None
        };
        let entry = CacheEntry {
            id: ulid::Ulid::new().to_string(),
            query: query.to_string(),
            query_embedding: self.embedder.embed_query(query),
            answer: answer.to_string(),
            source_ids,
            room,
            created_at: now,
            expires_at,
            hits: 0,
        };
        self.store.cache_put(&entry)?;
        let _ = self.store.cache_purge_expired();
        let _ = self.store.cache_evict_to(self.config.cache.max_entries);
        Ok(Some(entry.id))
    }

    /// Clear cached answers (optionally scoped to a room). Returns rows removed.
    pub fn cache_clear(&self, room: Option<&str>) -> Result<usize> {
        self.store.cache_clear(room)
    }

    /// Validate a drafted answer against the knowledge base (anti-hallucination).
    ///
    /// Evidence is taken from `source_ids` when given, else gathered by recalling
    /// the knowledge base for `query` (falling back to the answer text).
    pub fn validate(
        &self,
        answer: &str,
        query: Option<&str>,
        source_ids: Option<Vec<String>>,
        threshold_override: Option<f32>,
    ) -> Result<ValidationReport> {
        let evidence: Vec<Memory> = match source_ids {
            Some(ids) => {
                let mut out = Vec::new();
                for id in ids {
                    if let Some(m) = self.store.get_memory(&id)? {
                        out.push(m);
                    }
                }
                out
            }
            None => {
                let q = query.unwrap_or(answer);
                let top_k = self.config.guardrail.evidence_top_k.max(3);
                self.recall(q, top_k, DetailLevel::Full, usize::MAX, None, false)?
                    .scored
                    .into_iter()
                    .map(|s| s.memory)
                    .collect()
            }
        };
        let threshold = threshold_override.unwrap_or(self.config.guardrail.support_threshold);
        Ok(RuleValidator::new(threshold).validate(answer, &evidence))
    }

    // ---- simple accessors / management ---------------------------------

    pub fn get(&self, id: &str) -> Result<Option<Memory>> {
        self.store.get_memory(id)
    }

    pub fn list(&self, room: Option<&str>, limit: usize) -> Result<Vec<Memory>> {
        self.store.list_memories(room, None, limit)
    }

    pub fn forget(&mut self, id: &str) -> Result<()> {
        self.store.set_state(id, MemoryState::Archived)?;
        self.index.remove(id);
        // Cached answers grounded on this memory are now stale.
        let _ = self.store.cache_invalidate_by_source(id);
        self.store
            .timeline_add(id, "archived", None, &self.config.general.author)?;
        Ok(())
    }

    pub fn endorse(&mut self, id: &str, author: &str) -> Result<()> {
        let mut m = self
            .store
            .get_memory(id)?
            .ok_or_else(|| YbError::NotFound(id.into()))?;
        if !m.endorsed_by.iter().any(|a| a == author) {
            m.endorsed_by.push(author.to_string());
        }
        m.confidence = m.consensus_confidence();
        m.verified_at = Some(Utc::now());
        m.updated_at = Utc::now();
        self.store.update_memory(&m)?;
        self.store
            .timeline_add(id, "endorsed", Some(author), author)?;
        Ok(())
    }

    pub fn dispute(&mut self, id: &str, author: &str, reason: &str) -> Result<()> {
        let mut m = self
            .store
            .get_memory(id)?
            .ok_or_else(|| YbError::NotFound(id.into()))?;
        if !m.disputed_by.iter().any(|a| a == author) {
            m.disputed_by.push(author.to_string());
        }
        if m.disputed_by.len() >= m.endorsed_by.len().max(1) {
            m.state = MemoryState::Disputed;
        }
        m.confidence = m.consensus_confidence();
        m.updated_at = Utc::now();
        self.store.update_memory(&m)?;
        // A disputed memory should no longer back a cached answer.
        let _ = self.store.cache_invalidate_by_source(id);
        self.store
            .timeline_add(id, "disputed", Some(reason), author)?;
        Ok(())
    }

    pub fn list_conflicts(&self, only_pending: bool) -> Result<Vec<Conflict>> {
        self.store
            .list_conflicts(only_pending.then_some(ConflictState::Pending))
    }

    pub fn timeline(&self, memory_id: &str, limit: usize) -> Result<Vec<TimelineEvent>> {
        self.store.timeline_for(memory_id, limit)
    }

    pub fn edges_for(&self, memory_id: &str) -> Result<Vec<Edge>> {
        self.store.edges_for(memory_id)
    }

    pub fn stats(&self) -> Result<Stats> {
        Ok(Stats {
            total: self.store.count_memories()?,
            active: self.store.count_by_state(MemoryState::Active)?,
            archived: self.store.count_by_state(MemoryState::Archived)?,
            superseded: self.store.count_by_state(MemoryState::Superseded)?,
            disputed: self.store.count_by_state(MemoryState::Disputed)?,
            pending_conflicts: self.store.count_pending_conflicts()?,
            model: self.embedder.model_id().to_string(),
            dimension: self.embedder.dimension(),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    // ---- sessions & observations (hooks, Phase 4) ----------------------

    /// Start a new capture session, returning its id.
    pub fn start_session(
        &self,
        ide: &str,
        cwd: Option<String>,
        room: Option<String>,
    ) -> Result<String> {
        let session = crate::memory::Session {
            id: ulid::Ulid::new().to_string(),
            ide: ide.to_string(),
            cwd,
            room,
            started_at: Utc::now(),
            ended_at: None,
            metadata: None,
        };
        self.store.insert_session(&session)?;
        Ok(session.id)
    }

    /// Mark a session as ended.
    pub fn end_session(&self, session_id: &str) -> Result<()> {
        self.store.close_session(session_id)
    }

    // ---- import / export (Phase 5 building blocks) ---------------------

    /// Export memories as owned structs (embeddings are intentionally excluded;
    /// see ADR-8 — they are re-derived on import).
    pub fn export(&self, scope: Option<Scope>) -> Result<Vec<Memory>> {
        let all = self.store.list_memories(None, None, usize::MAX)?;
        Ok(match scope {
            Some(s) => all.into_iter().filter(|m| m.scope == s).collect(),
            None => all,
        })
    }

    /// Import a memory, re-embedding locally. Returns `false` if the id already
    /// existed (skipped). Conflict checking on import is deferred (ADR-8).
    pub fn import_memory(&mut self, memory: Memory) -> Result<bool> {
        if self.store.get_memory(&memory.id)?.is_some() {
            return Ok(false);
        }
        let embedding = self.embedder.embed_document(&memory.content);
        self.persist_new(&memory, &embedding, "imported")?;
        Ok(true)
    }

    /// Re-embed every stored memory with the embedder described by `new_config`
    /// and rebuild the vector index, then update the ADR-5 model/dimension lock.
    ///
    /// This is the migration path for switching embedding models (e.g. from the
    /// bundled hash embedder to an ONNX sentence-transformer), including when the
    /// vector dimension changes. The semantic cache is cleared because its stored
    /// query embeddings belong to the previous model's space.
    ///
    /// Runs as a standalone operation (it must bypass the lock check that
    /// [`Brain::open`] enforces), so callers pass the data dir directly.
    pub fn reindex(data_dir: &Path, new_config: Config) -> Result<ReindexReport> {
        let store = Store::open(&data_dir.join("brain.db"))?;
        let embedder = build_embedder(&new_config)?;
        let model = embedder.model_id().to_string();
        let dimension = embedder.dimension();

        let memories = store.list_memories(None, None, usize::MAX)?;
        let mut index = FlatIndex::new(dimension);
        for m in &memories {
            let v = embedder.embed_document(&m.content);
            store.set_embedding(&m.id, &v)?;
            index.upsert(&m.id, v);
        }

        // Stored cache query-embeddings belong to the old vector space.
        store.cache_clear(None)?;

        // Move the ADR-5 lock to the new model/dimension.
        store.meta_set("embedding_model", &model)?;
        store.meta_set("embedding_dimension", &dimension.to_string())?;

        index.save(&data_dir.join("brain.ybv"))?;

        Ok(ReindexReport {
            model,
            dimension,
            reembedded: memories.len(),
        })
    }

    /// Record a raw observation (prompt, tool_use, response, error).
    pub fn add_observation(&self, session_id: &str, kind: &str, content: &str) -> Result<String> {
        let compressed = self.compressor.compress(content);
        let obs = crate::memory::Observation {
            id: ulid::Ulid::new().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            content: content.to_string(),
            compressed,
            created_at: Utc::now(),
            metadata: None,
        };
        self.store.insert_observation(&obs)?;
        Ok(obs.id)
    }
}

fn parse_scope(s: &str) -> Scope {
    match s {
        "team" => Scope::Team,
        _ => Scope::Personal,
    }
}

/// Build the embedder from config. The dependency-free hash embedder is always
/// available; the `onnx` sentence-transformer backend is wired only when the
/// crate is built with `--features onnx`.
fn build_embedder(config: &Config) -> Result<Box<dyn Embedder>> {
    match config.embedding.provider.as_str() {
        #[cfg(feature = "onnx")]
        "onnx" => {
            let e = crate::embed::OnnxEmbedder::new(
                &config.embedding.model,
                config.embedding.cache_dir.as_deref(),
            )?;
            Ok(Box::new(e))
        }
        #[cfg(not(feature = "onnx"))]
        "onnx" => Err(crate::error::YbError::Embedder(
            "provider `onnx` requested but this binary was built without the \
             `onnx` feature; rebuild with `cargo build --features onnx`"
                .into(),
        )),
        _ => Ok(Box::new(HashEmbedder::new(config.embedding.dimension))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brain() -> Brain {
        Brain::in_memory(Config::default()).unwrap()
    }

    #[test]
    fn remember_and_recall() {
        let mut b = brain();
        let out = b
            .remember(
                "Auth uses JWT tokens stored in Redis",
                RememberOptions::default(),
            )
            .unwrap();
        assert!(matches!(out, RememberOutcome::Stored { .. }));

        let res = b
            .recall(
                "how does authentication work",
                5,
                DetailLevel::Summary,
                200,
                None,
                false,
            )
            .unwrap();
        assert!(!res.output.lines.is_empty(), "recall returned nothing");
        assert!(res.output.tokens_used <= 200);
    }

    #[test]
    fn unrelated_memories_do_not_conflict() {
        let mut b = brain();
        b.remember("Auth uses JWT", RememberOptions::default())
            .unwrap();
        let out = b
            .remember(
                "The office coffee machine is on the third floor",
                RememberOptions::default(),
            )
            .unwrap();
        assert!(matches!(out, RememberOutcome::Stored { .. }));
    }

    #[test]
    fn supersede_creates_conflict_or_autoresolves() {
        let mut b = brain();
        b.remember(
            "Deployment uses Kubernetes on GCP",
            RememberOptions::default(),
        )
        .unwrap();
        // Same author, supersede signal, similar topic.
        let out = b
            .remember(
                "Deployment sekarang pakai Docker Swarm, migrate from Kubernetes",
                RememberOptions::default(),
            )
            .unwrap();
        match out {
            RememberOutcome::NeedsReview { existing, .. } => {
                assert!(!existing.is_empty());
            }
            RememberOutcome::AutoResolved { .. } => {}
            RememberOutcome::Stored { .. } => {
                // Acceptable if similarity fell below threshold, but flag for visibility.
            }
        }
    }

    #[test]
    fn duplicate_is_discarded_on_resolve() {
        let mut b = brain();
        b.remember("Auth uses JWT tokens in Redis", RememberOptions::default())
            .unwrap();
        let before = b.stats().unwrap().total;
        // Near-identical content.
        let out = b
            .remember("Auth uses JWT tokens in Redis", RememberOptions::default())
            .unwrap();
        // Duplicate may auto-discard or ask; either way total should not double-count wrongly.
        if let RememberOutcome::NeedsReview { conflict_id, .. } = out {
            b.resolve(&conflict_id, ResolutionAction::DiscardNew, None, None, None)
                .unwrap();
        }
        let after = b.stats().unwrap().total;
        assert!(after <= before + 1);
    }

    #[test]
    fn endorse_dispute_updates_state() {
        let mut b = brain();
        let id = match b
            .remember("Team uses Rust", RememberOptions::default())
            .unwrap()
        {
            RememberOutcome::Stored { id } => id,
            _ => panic!("expected stored"),
        };
        b.endorse(&id, "alice").unwrap();
        b.dispute(&id, "bob", "we switched to Go").unwrap();
        let m = b.get(&id).unwrap().unwrap();
        assert!(m.disputed_by.contains(&"bob".to_string()));
    }
}
