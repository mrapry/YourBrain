//! Dynamic token budgeting: opt-in compression of recalled memories so more
//! relevant signal fits into a tighter token budget.
//!
//! Disabled by default (see `[token_budget]` config); when enabled it condenses
//! each memory with an extractive, query-aware summarizer. The [`Summarizer`]
//! trait is the extension point for LLMLingua/LLM-backed condensers later.

use crate::compress::count_tokens;
use crate::search::{RecallOutput, ScoredMemory};

/// Condenses a text toward a token budget, given the query for relevance.
pub trait Summarizer: Send + Sync {
    fn condense(&self, query: &str, text: &str, max_tokens: usize) -> String;
}

/// Query-aware extractive summarizer: keeps the sentences most relevant to the
/// query (by term overlap, with a gentle lead bias) until the budget is spent.
pub struct ExtractiveSummarizer;

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

/// Split into sentences on `.`, `!`, `?`, and newlines, keeping non-empty pieces.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let t = cur.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
            cur.clear();
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

impl Summarizer for ExtractiveSummarizer {
    fn condense(&self, query: &str, text: &str, max_tokens: usize) -> String {
        if max_tokens == 0 || count_tokens(text) <= max_tokens {
            return text.to_string();
        }
        let q_terms = tokenize(query);
        let sentences = split_sentences(text);
        if sentences.len() <= 1 {
            return text.to_string();
        }

        // Score each sentence by query-term overlap, with a small lead bias so
        // topic-setting first sentences are not unfairly dropped.
        let mut scored: Vec<(usize, f32)> = sentences
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let terms = tokenize(s);
                let overlap = if terms.is_empty() {
                    0.0
                } else {
                    let hits = terms.iter().filter(|w| q_terms.contains(w)).count() as f32;
                    hits / terms.len() as f32
                };
                let lead_bias = 1.0 / (1.0 + i as f32);
                (i, overlap + 0.15 * lead_bias)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Greedily select sentences within budget, then restore original order.
        let mut chosen: Vec<usize> = Vec::new();
        let mut used = 0usize;
        for (idx, _) in scored {
            let cost = count_tokens(&sentences[idx]);
            if used + cost > max_tokens {
                continue;
            }
            chosen.push(idx);
            used += cost;
            if used >= max_tokens {
                break;
            }
        }
        if chosen.is_empty() {
            // Budget smaller than any sentence: fall back to a hard char cut.
            let approx_chars = max_tokens.saturating_mul(4);
            return text.chars().take(approx_chars).collect();
        }
        chosen.sort_unstable();
        chosen
            .into_iter()
            .map(|i| sentences[i].clone())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Pack scored memories into `max_tokens`, condensing each memory's content with
/// the summarizer so more items fit than the static `allocate_budget` allows.
pub fn dynamic_allocate_budget(
    memories: &[ScoredMemory],
    max_tokens: usize,
    query: &str,
    summarizer: &dyn Summarizer,
) -> RecallOutput {
    let mut lines = Vec::new();
    let mut ids = Vec::new();
    let mut used = 0usize;

    let remaining_items = memories.len().max(1);
    for (i, sm) in memories.iter().enumerate() {
        let remaining = max_tokens.saturating_sub(used);
        if remaining < 8 {
            break;
        }
        // Give each remaining item a fair share of the leftover budget.
        let share = (remaining / remaining_items.saturating_sub(i).max(1)).max(16);
        let per_item = share.min(remaining);

        let condensed = summarizer.condense(query, &sm.memory.content, per_item);
        let stars_n = ((sm.memory.confidence * 5.0).round() as usize).clamp(1, 5);
        let topic = sm.memory.headline.split(':').next().unwrap_or("").trim();
        let line = format!("[{}] {}: {}", "\u{2605}".repeat(stars_n), topic, condensed);

        let cost = count_tokens(&line);
        if used + cost > max_tokens {
            continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_memory;

    #[test]
    fn condense_keeps_query_relevant_sentence() {
        let text = "The weather today is sunny and warm. \
             Authentication uses JWT tokens stored in Redis with a 15 minute expiry. \
             The office cafeteria serves lunch at noon.";
        let out = ExtractiveSummarizer.condense("how does JWT authentication work", text, 20);
        assert!(out.to_lowercase().contains("jwt"), "got: {out}");
        assert!(count_tokens(&out) <= 20 + 8);
    }

    #[test]
    fn short_text_is_unchanged() {
        let out = ExtractiveSummarizer.condense("q", "short text", 100);
        assert_eq!(out, "short text");
    }

    #[test]
    fn dynamic_budget_respects_limit() {
        let mems: Vec<ScoredMemory> = (0..10)
            .map(|i| ScoredMemory {
                memory: sample_memory(&format!(
                    "Memory {i} describes a long technical topic. It has several sentences. \
                     Some of them mention databases and caching and tokens."
                )),
                score: 1.0,
            })
            .collect();
        let out = dynamic_allocate_budget(&mems, 80, "database caching", &ExtractiveSummarizer);
        assert!(out.tokens_used <= 80, "used {}", out.tokens_used);
        assert!(!out.lines.is_empty());
    }
}
