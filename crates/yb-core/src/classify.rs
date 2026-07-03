//! Preprocessing (privacy) and rule-based classification.
//!
//! Phase 1 classification is deliberately simple and high-precision (ADR-10):
//! keyword-signalled type detection plus lightweight entity/tag extraction.

use regex::Regex;
use std::sync::OnceLock;

use crate::memory::MemoryType;

/// Result of preprocessing raw input before storage.
#[derive(Debug, Clone)]
pub struct Preprocessed {
    pub text: String,
    pub redacted: bool,
}

/// Result of classifying a piece of text.
#[derive(Debug, Clone)]
pub struct Classification {
    pub memory_type: MemoryType,
    pub entities: Vec<String>,
    pub tags: Vec<String>,
}

fn private_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<private>.*?</private>").unwrap())
}

fn secret_res() -> &'static Vec<Regex> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        [
            r#"(?i)(api[_-]?key|apikey)\s*[:=]\s*['"]?[A-Za-z0-9\-_]{20,}"#,
            r"(?i)bearer\s+[A-Za-z0-9\-_.~+/]+=*",
            r"AKIA[0-9A-Z]{16}",
            r"-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----",
            r#"(?i)(password|passwd|secret|token)\s*[:=]\s*['"]?[^\s'"]{8,}"#,
            r"(?i)(postgres|postgresql|mysql|mongodb|redis)://\S+",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

/// Strip `<private>` blocks, redact secrets, and normalize whitespace.
///
/// `exclude_patterns` are treated as literal substrings whose surrounding line
/// is dropped (Phase 1 keeps path exclusion simple and predictable).
pub fn preprocess(input: &str, exclude_patterns: &[String]) -> Preprocessed {
    let mut text = private_block_re().replace_all(input, "").to_string();

    let mut redacted = text != input;

    for re in secret_res() {
        let replaced = re.replace_all(&text, "[REDACTED]").to_string();
        if replaced != text {
            redacted = true;
            text = replaced;
        }
    }

    if !exclude_patterns.is_empty() {
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| !exclude_patterns.iter().any(|p| line.contains(p.as_str())))
            .collect();
        let filtered = kept.join("\n");
        if filtered != text {
            redacted = true;
            text = filtered;
        }
    }

    // Normalize runs of whitespace but preserve newlines.
    let normalized = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    Preprocessed {
        text: normalized,
        redacted,
    }
}

/// Classify text into a memory type and extract entities/tags.
pub fn classify(text: &str) -> Classification {
    Classification {
        memory_type: detect_type(text),
        entities: extract_entities(text),
        tags: extract_tags(text),
    }
}

/// Rule-based type detection. Order encodes priority.
pub fn detect_type(text: &str) -> MemoryType {
    let l = text.to_lowercase();
    let any = |signals: &[&str]| signals.iter().any(|s| l.contains(s));

    if any(&[
        "we chose",
        "we decided",
        "kita pilih",
        "memutuskan",
        "keputusan",
        "decided to",
        "opted for",
    ]) {
        MemoryType::Decision
    } else if any(&[
        "to deploy",
        "steps:",
        "langkah",
        "cara ",
        "run the following",
        "first ",
        "then ",
        "procedure",
    ]) {
        MemoryType::Procedure
    } else if any(&[
        "fixed by",
        "solved by",
        "solution",
        "bug ",
        "workaround",
        "resolved by",
        "diperbaiki",
    ]) {
        MemoryType::Solution
    } else if any(&[
        "prefer",
        "lebih suka",
        "biasanya pakai",
        "i like",
        "favorite",
        "selalu pakai",
    ]) {
        MemoryType::Preference
    } else if any(&[
        "deployed",
        "released",
        "shipped",
        "launched",
        "on june",
        "on may",
        "pada tanggal",
        "rilis",
    ]) {
        MemoryType::Event
    } else {
        MemoryType::Fact
    }
}

/// Known technology terms that make good tags/entities.
const TECH_TERMS: &[&str] = &[
    "jwt",
    "redis",
    "postgres",
    "postgresql",
    "mysql",
    "mongodb",
    "kubernetes",
    "k8s",
    "docker",
    "rust",
    "python",
    "typescript",
    "javascript",
    "go",
    "java",
    "auth",
    "oauth",
    "oauth2",
    "grpc",
    "rest",
    "graphql",
    "kafka",
    "rabbitmq",
    "aws",
    "gcp",
    "azure",
    "nginx",
    "grafana",
    "prometheus",
    "argocd",
    "terraform",
];

/// Extract entities: known tech terms + capitalized / versioned tokens.
pub fn extract_entities(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        let clean = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/');
        if clean.len() < 2 {
            continue;
        }
        let lower = clean.to_lowercase();
        let is_tech = TECH_TERMS.contains(&lower.as_str());
        let is_capitalized = clean.chars().next().is_some_and(|c| c.is_uppercase())
            && clean
                .chars()
                .skip(1)
                .any(|c| c.is_lowercase() || c.is_uppercase());
        let has_version =
            clean.chars().any(|c| c.is_ascii_digit()) && clean.chars().any(|c| c.is_alphabetic());
        if (is_tech || is_capitalized || has_version)
            && !out.iter().any(|e| e.eq_ignore_ascii_case(clean))
        {
            out.push(clean.to_string());
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Extract lowercase tags from known tech terms present in the text.
pub fn extract_tags(text: &str) -> Vec<String> {
    let l = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for term in TECH_TERMS {
        if l.split(|c: char| !c.is_alphanumeric()).any(|w| w == *term)
            && !out.contains(&term.to_string())
        {
            out.push(term.to_string());
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_private_blocks() {
        let p = preprocess("public <private>secret stuff</private> text", &[]);
        assert!(!p.text.contains("secret stuff"));
        assert!(p.redacted);
    }

    #[test]
    fn redacts_secrets() {
        let p = preprocess("api_key = ABCDEFGHIJKLMNOPQRSTUVWX", &[]);
        assert!(p.text.contains("[REDACTED]"), "got: {}", p.text);
        assert!(p.redacted);
    }

    #[test]
    fn excludes_pattern_lines() {
        let input = "keep this\npassword line at .env\nkeep that";
        let p = preprocess(input, &[".env".to_string()]);
        assert!(!p.text.contains(".env"));
        assert!(p.text.contains("keep this"));
    }

    #[test]
    fn detects_types() {
        assert_eq!(
            detect_type("We chose PostgreSQL because of reliability"),
            MemoryType::Decision
        );
        assert_eq!(
            detect_type("Bug fixed by adding a null check"),
            MemoryType::Solution
        );
        assert_eq!(detect_type("Auth uses JWT"), MemoryType::Fact);
    }

    #[test]
    fn extracts_tags_and_entities() {
        let tags = extract_tags("Auth uses JWT tokens with Redis backend");
        assert!(tags.contains(&"jwt".to_string()));
        assert!(tags.contains(&"redis".to_string()));
        let ents = extract_entities("Deployed on Kubernetes v1.29 in GCP");
        assert!(ents.iter().any(|e| e.eq_ignore_ascii_case("kubernetes")));
    }
}
