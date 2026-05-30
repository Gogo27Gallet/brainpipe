//! Fast regex PII detection & redaction — no Presidio/spaCy (single-pass, Rust-native).
//! Competitors typically: Unstructured parse → external Presidio (2 pipelines, Python-heavy).

use regex::{Regex, RegexSet};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiiMode {
    Off,
    Redact,
    Mask,
    Tag,
}

impl PiiMode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "off" | "" => Ok(PiiMode::Off),
            "redact" | "remove" => Ok(PiiMode::Redact),
            "mask" | "partial" => Ok(PiiMode::Mask),
            "tag" | "label" => Ok(PiiMode::Tag),
            _ => Err(format!(
                "Unknown pii_mode '{}'. Use off, redact, mask, or tag.",
                s
            )),
        }
    }
}

struct PiiEngine {
    set: RegexSet,
    patterns: Vec<(&'static str, Regex)>,
}

static PII_ENGINE: LazyLock<PiiEngine> = LazyLock::new(|| {
    let definitions = vec![
        ("EMAIL", r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b"),
        ("PHONE", r"(?x)(?:\+?\d{1,3}[\s.\-]?)?(?:\(?\d{1,4}\)?[\s.\-]?)?\d{2,3}[\s.\-]?\d{2}[\s.\-]?\d{2}(?:[\s.\-]?\d{2})?"),
        ("SSN", r"\b\d{3}-\d{2}-\d{4}\b"),
        ("IBAN", r"(?i)\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b"),
        ("CREDIT_CARD", r"\b(?:\d{4}[\s\-]?){3}\d{4}\b"),
        ("IP", r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b"),
        ("API_KEY", r"(?i)\b(?:sk|pk)[-_](?:live|test)?[_-]?[a-z0-9]{16,}\b"),
        ("JWT", r"\beyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\b"),
    ];
    let set = RegexSet::new(definitions.iter().map(|(_, pat)| *pat)).unwrap();
    let patterns = definitions.into_iter().map(|(label, pat)| (label, Regex::new(pat).unwrap())).collect();
    PiiEngine { set, patterns }
});

/// Returns (sanitized_text, entity_count, entity_types_found).
pub fn sanitize_text(text: &str, mode: PiiMode) -> (String, usize, Vec<String>) {
    if mode == PiiMode::Off || text.is_empty() {
        return (text.to_string(), 0, Vec::new());
    }

    let engine = &*PII_ENGINE;
    let matches = engine.set.matches(text);
    if !matches.matched_any() {
        return (text.to_string(), 0, Vec::new());
    }

    let mut out = text.to_string();
    let mut count = 0usize;
    let mut types = Vec::new();

    for i in matches.into_iter() {
        let (label, ref re) = engine.patterns[i];
        out = re.replace_all(&out, |caps: &regex::Captures| {
            count += 1;
            if !types.iter().any(|t| t == label) {
                types.push(label.to_string());
            }
            let m = caps.get(0).map(|x| x.as_str()).unwrap_or("");
            match mode {
                PiiMode::Redact => format!("[{label}_REDACTED]"),
                PiiMode::Tag => format!("<PII type=\"{label}\">{m}</PII>"),
                PiiMode::Mask => mask_value(m, label),
                PiiMode::Off => m.to_string(),
            }
        }).into_owned();
    }

    (out, count, types)
}

fn mask_value(value: &str, label: &str) -> String {
    match label {
        "EMAIL" => {
            if let Some((user, domain)) = value.split_once('@') {
                let u = user.chars().next().unwrap_or('*');
                format!("{u}***@{domain}")
            } else {
                "***@***".to_string()
            }
        }
        "PHONE" => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 4 {
                format!("***-***-{}", &digits[digits.len().saturating_sub(4)..])
            } else {
                "***".to_string()
            }
        }
        _ => {
            if value.len() <= 4 {
                "****".to_string()
            } else {
                format!("{}****", &value[..2.min(value.len())])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_email() {
        let (out, n, types) = sanitize_text("Contact me at alice@corp.com please", PiiMode::Redact);
        assert!(out.contains("[EMAIL_REDACTED]"));
        assert!(!out.contains("alice@corp.com"));
        assert!(n >= 1);
        assert!(types.contains(&"EMAIL".to_string()));
    }

    #[test]
    fn masks_phone() {
        let (out, _, _) = sanitize_text("Call +33 6 12 34 56 78", PiiMode::Mask);
        assert!(out.contains("***"));
        assert!(!out.contains("12 34 56 78") || out.contains("***"));
    }
}
