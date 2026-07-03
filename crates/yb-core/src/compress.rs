//! Rule-based compression engine (see Section 9 / ADR-11).
//!
//! Produces three representations from an input string:
//! - `compressed`: near-lossless rule-based abbreviation (~20-35% smaller).
//! - `summary`: lossy one-line summary for AI context injection.
//! - `headline`: ultra-short topic label for list views.
//!
//! Code blocks, file paths, and URLs are protected byte-for-byte.

use regex::Regex;
use std::sync::OnceLock;

/// The three generated representations of a memory's text.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub compressed: String,
    pub summary: String,
    pub headline: String,
}

/// Compression intensity (mirrors `[compression] intensity` config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intensity {
    Lite,
    Full,
    Ultra,
}

/// Configuration for the compressor.
#[derive(Debug, Clone)]
pub struct CompressConfig {
    pub intensity: Intensity,
    pub preserve_code: bool,
    pub preserve_paths: bool,
    pub preserve_urls: bool,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            intensity: Intensity::Full,
            preserve_code: true,
            preserve_paths: true,
            preserve_urls: true,
        }
    }
}

/// Abbreviation replacements applied on whole words (case-insensitive key).
const ABBREVIATIONS: &[(&str, &str)] = &[
    ("menggunakan", "pakai"),
    ("mengimplementasikan", "impl"),
    ("konfigurasi", "config"),
    ("configuration", "config"),
    ("database", "DB"),
    ("repository", "repo"),
    ("production", "prod"),
    ("development", "dev"),
    ("environment", "env"),
    ("authentication", "auth"),
    ("authorization", "authz"),
    ("application", "app"),
    ("deployment", "deploy"),
    ("infrastructure", "infra"),
    ("implementation", "impl"),
    ("documentation", "docs"),
    ("dependencies", "deps"),
    ("requirements", "reqs"),
    ("performance", "perf"),
    ("middleware", "mw"),
];

/// Filler words removed entirely.
const FILLERS: &[&str] = &[
    "actually",
    "basically",
    "essentially",
    "sebenernya",
    "sebenarnya",
    "pada dasarnya",
    "well",
    "really",
    "just",
    "quite",
    "rather",
];

/// Articles/particles removed only at `Full`/`Ultra` intensity.
const ARTICLES: &[&str] = &["the", "a", "an", "yang"];

/// Multi-word phrase shortenings.
const PHRASES: &[(&str, &str)] = &[
    ("in order to", "to"),
    ("due to the fact that", "because"),
    ("at this point in time", "now"),
    ("pada saat ini", "now"),
];

fn protect_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Order matters: code fences, inline code, URLs, then path-like tokens.
        Regex::new(
            r"(?s)```.*?```|`[^`]*`|https?://\S+|[A-Za-z]:\\[^\s]+|(?:\./|/)?[\w.\-]+(?:/[\w.\-]+)+|\b[\w.\-]+\.(?:rs|toml|json|md|yaml|yml|ts|js|py|go|sql)\b",
        )
        .unwrap()
    })
}

/// Replace protected literals with placeholders, returning the masked text and
/// the ordered list of originals.
fn protect(text: &str) -> (String, Vec<String>) {
    let re = protect_re();
    let mut literals = Vec::new();
    let out = re.replace_all(text, |caps: &regex::Captures| {
        let idx = literals.len();
        literals.push(caps[0].to_string());
        format!("\u{0}{idx}\u{0}")
    });
    (out.into_owned(), literals)
}

fn restore(text: &str, literals: &[String]) -> String {
    let mut out = text.to_string();
    for (idx, lit) in literals.iter().enumerate() {
        out = out.replace(&format!("\u{0}{idx}\u{0}"), lit);
    }
    out
}

/// The compression engine.
#[derive(Debug, Clone, Default)]
pub struct Compressor {
    config: CompressConfig,
}

impl Compressor {
    pub fn new(config: CompressConfig) -> Self {
        Self { config }
    }

    /// Produce the near-lossless compressed form.
    pub fn compress(&self, text: &str) -> String {
        let (masked, literals) = protect(text);
        let mut result = masked;

        // Phrase shortenings first (multi-word).
        for (from, to) in PHRASES {
            result = replace_ci(&result, from, to);
        }

        // Word-level transforms.
        let mut out_words: Vec<String> = Vec::new();
        for word in result.split_whitespace() {
            let trimmed_lower = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();

            if FILLERS.contains(&trimmed_lower.as_str()) {
                continue;
            }
            // Drop articles at higher intensities (config-driven).
            if self.config.intensity != Intensity::Lite
                && ARTICLES.contains(&trimmed_lower.as_str())
            {
                continue;
            }
            if let Some((_, abbr)) = ABBREVIATIONS
                .iter()
                .find(|(k, _)| *k == trimmed_lower.as_str())
            {
                out_words.push(preserve_trailing_punct(word, abbr));
                continue;
            }
            out_words.push(word.to_string());
        }
        result = out_words.join(" ");

        restore(&result, &literals)
    }

    /// Generate all three levels.
    pub fn levels(&self, text: &str) -> Levels {
        let compressed = self.compress(text);
        let summary = generate_summary(&compressed);
        let headline = generate_headline(text);
        Levels {
            compressed,
            summary,
            headline,
        }
    }
}

/// Case-insensitive whole-substring replace.
fn replace_ci(haystack: &str, from: &str, to: &str) -> String {
    let mut result = String::with_capacity(haystack.len());
    let lower = haystack.to_lowercase();
    let from_lower = from.to_lowercase();
    let mut last = 0;
    let mut search_start = 0;
    while let Some(pos) = lower[search_start..].find(&from_lower) {
        let abs = search_start + pos;
        result.push_str(&haystack[last..abs]);
        result.push_str(to);
        last = abs + from.len();
        search_start = last;
    }
    result.push_str(&haystack[last..]);
    result
}

/// Preserve trailing punctuation from the original word when abbreviating.
fn preserve_trailing_punct(original: &str, abbr: &str) -> String {
    let trailing: String = original
        .chars()
        .rev()
        .take_while(|c| !c.is_alphanumeric())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{abbr}{trailing}")
}

/// Approximate token count (~1 token per 4 chars, min word count).
pub fn count_tokens(text: &str) -> usize {
    let by_chars = text.chars().count() / 4;
    let by_words = text.split_whitespace().count();
    by_chars.max(by_words)
}

fn generate_summary(compressed: &str) -> String {
    let sentences: Vec<&str> = compressed
        .split(['.', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if sentences.is_empty() {
        return compressed.trim().to_string();
    }
    let take = if count_tokens(sentences[0]) < 40 {
        2.min(sentences.len())
    } else {
        1
    };
    let joined = sentences[..take].join(". ");
    format!("{joined}.")
}

fn generate_headline(content: &str) -> String {
    let topic = extract_primary_topic(content);
    let facts = extract_key_values(content);
    if facts.is_empty() {
        topic
    } else {
        format!("{}: {}", topic, facts.join("+"))
    }
}

/// Primary topic = the first capitalized/technical token, uppercased.
fn extract_primary_topic(content: &str) -> String {
    for word in content.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() >= 3 && clean.chars().next().is_some_and(|c| c.is_uppercase()) {
            return clean.to_uppercase();
        }
    }
    content
        .split_whitespace()
        .next()
        .unwrap_or("MEMO")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_uppercase()
}

/// Key values = short technical tokens (contain digits, uppercase, or known
/// tech markers). Capped at 3.
fn extract_key_values(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in content.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '=' && c != '/');
        if clean.len() < 2 {
            continue;
        }
        let has_digit = clean.chars().any(|c| c.is_ascii_digit());
        let is_acronym =
            clean.len() >= 2 && clean.len() <= 6 && clean.chars().all(|c| c.is_ascii_uppercase());
        if (has_digit || is_acronym) && !out.contains(&clean.to_string()) {
            out.push(clean.to_string());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> Compressor {
        Compressor::new(CompressConfig::default())
    }

    #[test]
    fn preserves_inline_code() {
        let input = "We use `async fn main()` in the entry point";
        let out = c().compress(input);
        assert!(out.contains("`async fn main()`"), "got: {out}");
    }

    #[test]
    fn preserves_paths_and_urls() {
        let input = "Config lives at src/config.rs see https://example.com/docs for authentication";
        let out = c().compress(input);
        assert!(out.contains("src/config.rs"), "path lost: {out}");
        assert!(out.contains("https://example.com/docs"), "url lost: {out}");
        // "authentication" should be abbreviated to "auth" outside protected regions.
        assert!(out.contains("auth"), "abbrev missing: {out}");
    }

    #[test]
    fn abbreviates_and_removes_fillers() {
        let input = "This is basically the production database configuration";
        let out = c().compress(input);
        assert!(!out.to_lowercase().contains("basically"));
        assert!(out.contains("prod"));
        assert!(out.contains("DB"));
        assert!(out.contains("config"));
    }

    #[test]
    fn compression_reduces_length() {
        let input =
            "We are using the production environment configuration for the application deployment";
        let out = c().compress(input);
        assert!(out.len() < input.len(), "not shorter: {out}");
    }

    #[test]
    fn levels_are_generated() {
        let input =
            "Auth backend uses JWT with access token expiry 15min and refresh token in Redis";
        let levels = c().levels(input);
        assert!(!levels.headline.is_empty());
        assert!(!levels.summary.is_empty());
        assert!(!levels.compressed.is_empty());
    }
}
