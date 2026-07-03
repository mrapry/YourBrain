//! Hybrid search: Reciprocal Rank Fusion of FTS5 + vector results, re-ranking,
//! and token-budgeted recall formatting (Section 10).
//!
//! These are pure functions over already-fetched candidates so they can be unit
//! tested without a database. [`crate::brain::Brain`] wires them to the store
//! and vector index.

use chrono::Utc;
use std::collections::HashMap;

use crate::compress::count_tokens;
use crate::memory::Memory;

/// Level of detail used when rendering a memory into recall output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLevel {
    Headline,
    Summary,
    Full,
}

impl DetailLevel {
    pub fn parse(s: &str) -> DetailLevel {
        match s {
            "headline" => DetailLevel::Headline,
            "full" => DetailLevel::Full,
            _ => DetailLevel::Summary,
        }
    }
}

/// A memory paired with its fused+reranked relevance score.
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub memory: Memory,
    pub score: f32,
}

/// The rendered result of a recall, ready to hand to an AI.
#[derive(Debug, Clone)]
pub struct RecallOutput {
    pub lines: Vec<String>,
    pub tokens_used: usize,
    pub ids: Vec<String>,
}

/// Reciprocal Rank Fusion. `k` dampens the contribution of lower ranks.
///
/// Each input is a list of ids in rank order (best first). Vector results also
/// carry a similarity, unused here (rank is what RRF fuses). Returns ids with
/// fused scores, sorted descending.
pub fn rrf_fuse(fts_ranked: &[String], vector_ranked: &[String], k: f32) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (rank, id) in fts_ranked.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    for (rank, id) in vector_ranked.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    let mut out: Vec<(String, f32)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Apply recency/importance/access/confidence boosts on top of the fused base
/// score, returning memories sorted by final score descending.
pub fn rerank(scored: Vec<(Memory, f32)>) -> Vec<ScoredMemory> {
    let now = Utc::now();
    let mut out: Vec<ScoredMemory> = scored
        .into_iter()
        .map(|(m, base)| {
            let age_days = (now - m.created_at).num_days().max(0) as f32;
            // Gentle recency decay over ~180 days.
            let recency = 1.0 / (1.0 + age_days / 180.0);
            let access = (1.0 + m.access_count as f32).ln();
            let score = base * (1.0 + 0.3 * recency + 0.2 * m.importance + 0.1 * m.confidence)
                + 0.01 * access;
            ScoredMemory { memory: m, score }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Allocate a token budget across scored memories: the top few get a `summary`,
/// the rest get a `headline`, stopping when the budget is exhausted (Section 10.3).
pub fn allocate_budget(
    memories: &[ScoredMemory],
    max_tokens: usize,
    default_detail: DetailLevel,
    top_detail_count: usize,
) -> RecallOutput {
    let mut lines = Vec::new();
    let mut ids = Vec::new();
    let mut used = 0usize;

    for (i, sm) in memories.iter().enumerate() {
        let remaining = max_tokens.saturating_sub(used);
        if remaining < 8 {
            break;
        }
        let want = if default_detail == DetailLevel::Full {
            DetailLevel::Full
        } else if i < top_detail_count {
            DetailLevel::Summary
        } else {
            DetailLevel::Headline
        };

        let line = format_output(&sm.memory, want);
        let cost = count_tokens(&line);
        if used + cost > max_tokens {
            // Try to downgrade to a headline before giving up.
            let hl = format_output(&sm.memory, DetailLevel::Headline);
            let hl_cost = count_tokens(&hl);
            if used + hl_cost <= max_tokens {
                lines.push(hl);
                ids.push(sm.memory.id.clone());
                used += hl_cost;
            }
            break;
        }
        lines.push(line);
        ids.push(sm.memory.id.clone());
        used += cost;
    }

    RecallOutput {
        lines,
        tokens_used: used,
        ids,
    }
}

/// Render a memory at a given detail level, with a confidence star rating.
pub fn format_output(m: &Memory, level: DetailLevel) -> String {
    let stars_n = ((m.confidence * 5.0).round() as usize).clamp(1, 5);
    let stars = "★".repeat(stars_n);
    let date = short_date(&m.created_at);
    match level {
        DetailLevel::Headline => {
            format!("[{}] {} @{} | {}", stars, m.headline, m.author, date)
        }
        DetailLevel::Summary => {
            let topic = m.headline.split(':').next().unwrap_or("").trim();
            let tags = if m.tags.is_empty() {
                String::new()
            } else {
                format!(" #{}", m.tags.join(" #"))
            };
            format!(
                "[{}] {}: {} @{}{} | {}",
                stars, topic, m.summary, m.author, tags, date
            )
        }
        DetailLevel::Full => m.content.clone(),
    }
}

fn short_date(dt: &chrono::DateTime<Utc>) -> String {
    dt.format("%b'%y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_memory;

    #[test]
    fn rrf_prefers_items_ranked_high_in_both() {
        let fts = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let vec = vec!["b".to_string(), "a".to_string(), "d".to_string()];
        let fused = rrf_fuse(&fts, &vec, 60.0);
        // "a" and "b" appear in both → should top the list.
        let top2: Vec<&str> = fused.iter().take(2).map(|(id, _)| id.as_str()).collect();
        assert!(top2.contains(&"a") && top2.contains(&"b"), "got {top2:?}");
    }

    #[test]
    fn budget_respects_limit() {
        let mems: Vec<ScoredMemory> = (0..20)
            .map(|i| ScoredMemory {
                memory: sample_memory(&format!("memory number {i} about various technical topics")),
                score: 1.0,
            })
            .collect();
        let out = allocate_budget(&mems, 100, DetailLevel::Summary, 3);
        assert!(out.tokens_used <= 100, "used {} > 100", out.tokens_used);
        assert!(!out.lines.is_empty());
    }

    #[test]
    fn format_levels_differ() {
        let m = sample_memory("Auth uses JWT with Redis backend");
        let hl = format_output(&m, DetailLevel::Headline);
        let full = format_output(&m, DetailLevel::Full);
        assert_ne!(hl, full);
        assert_eq!(full, m.content);
    }
}
