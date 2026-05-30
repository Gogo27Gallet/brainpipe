//! Per-page quality signals: density, confidence, language heuristic.

/// Lightweight script/language guess from character samples.
pub fn detect_language_hint(text: &str) -> Option<String> {
    let sample: String = text.chars().take(2000).collect();
    if sample.trim().is_empty() {
        return None;
    }
    let mut latin = 0u32;
    let mut cjk = 0u32;
    let mut cyrillic = 0u32;
    let mut arabic = 0u32;
    for ch in sample.chars() {
        if ch.is_ascii_alphabetic() {
            latin += 1;
        } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            cjk += 1;
        } else if ('\u{0400}'..='\u{04FF}').contains(&ch) {
            cyrillic += 1;
        } else if ('\u{0600}'..='\u{06FF}').contains(&ch) {
            arabic += 1;
        }
    }
    let total = (latin + cjk + cyrillic + arabic).max(1);
    let ratios = [
        ("en", latin as f32 / total as f32),
        ("zh", cjk as f32 / total as f32),
        ("ru", cyrillic as f32 / total as f32),
        ("ar", arabic as f32 / total as f32),
    ];
    let (lang, score) = ratios
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
        .unwrap_or(("unknown", 0.0));
    if score > 0.15 {
        Some(lang.to_string())
    } else {
        Some("unknown".to_string())
    }
}

pub fn text_density(chars: usize, width: u32, height: u32) -> f32 {
    let area = (width as f64).max(1.0) * (height as f64).max(1.0);
    (chars as f32) / (area.sqrt() as f32 / 100.0).max(1.0)
}

pub fn extraction_confidence(
    text_len: usize,
    ocr_used: bool,
    had_error: bool,
    text_density: f32,
) -> f32 {
    if had_error {
        return 0.0;
    }
    let mut score: f32 = if text_len == 0 {
        0.1
    } else if text_len < 20 {
        0.4
    } else {
        0.75
    };
    if ocr_used {
        score = (score * 0.85).max(0.35);
    }
    if text_density > 0.5 {
        score = (score + 0.15).min(1.0);
    } else if text_density < 0.05 && text_len > 0 {
        score *= 0.9;
    }
    score.clamp(0.0, 1.0)
}

/// Scanned PDF heuristic: low average printable chars per page.
pub fn looks_scanned(avg_chars_per_page: f32) -> bool {
    avg_chars_per_page < 40.0
}

/// Garbled extraction: high ratio of replacement / control chars.
pub fn looks_garbled(text: &str) -> bool {
    if text.len() < 20 {
        return false;
    }
    let sample: String = text.chars().take(4000).collect();
    let bad = sample
        .chars()
        .filter(|c| *c == '\u{FFFD}' || (c.is_control() && *c != '\n' && *c != '\t'))
        .count();
    let weird = sample
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace() && !c.is_ascii_punctuation())
        .count();
    let n = sample.chars().count().max(1);
    (bad as f32 / n as f32) > 0.08 || (weird as f32 / n as f32) > 0.55
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanned_detection() {
        assert!(looks_scanned(10.0));
        assert!(!looks_scanned(500.0));
    }

    #[test]
    fn confidence_bounds() {
        let c = extraction_confidence(100, false, false, 1.0);
        assert!(c > 0.5 && c <= 1.0);
        assert_eq!(extraction_confidence(0, false, true, 0.0), 0.0);
    }
}
