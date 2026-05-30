//! Fast text/data/email parsers — no browser, no LibreOffice.

use std::path::Path;

pub fn extract_plain(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("Read '{}': {}", path.display(), e))
}

/// Strip RTF control words — best-effort, streaming-friendly.
pub fn extract_rtf(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("RTF read '{}': {}", path.display(), e))?;
    Ok(strip_rtf_fast(&raw))
}

fn strip_rtf_fast(rtf: &str) -> String {
    let mut out = String::with_capacity(rtf.len() / 2);
    let mut chars = rtf.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' || c == '}' {
            continue;
        }
        if c == '\\' {
            let mut word = String::new();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphabetic() {
                    word.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if word == "par" || word == "line" {
                out.push('\n');
            } else if word == "tab" {
                out.push('\t');
            } else if word == "'" {
                let _ = chars.next();
                let _ = chars.next();
            } else if word.is_empty() {
                let _ = chars.next();
            }
            while chars.peek().is_some_and(|c| c.is_ascii_digit() || *c == '-') {
                chars.next();
            }
            if chars.peek() == Some(&' ') {
                chars.next();
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
    }
    let re_ws = regex::Regex::new(r"[ \t]+\n").unwrap();
    re_ws.replace_all(out.trim(), "\n").into_owned()
}

pub fn extract_xml_as_text(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("XML read '{}': {}", path.display(), e))?;
    let re_tags = regex::Regex::new(r"(?is)<[^>]+>").unwrap();
    let re_ws = regex::Regex::new(r"\s+").unwrap();
    let t = re_tags.replace_all(&raw, " ");
    Ok(re_ws.replace_all(t.trim(), " ").into_owned())
}

pub fn extract_ndjson(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("NDJSON read '{}': {}", path.display(), e))?;
    let mut lines = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            lines.push(flatten_json_line(&v));
        } else {
            lines.push(line.to_string());
        }
    }
    Ok(lines.join("\n"))
}

fn flatten_json_line(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", value_scalar(val)))
            .collect::<Vec<_>>()
            .join(" | "),
        _ => value_scalar(v),
    }
}

fn value_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => v.to_string(),
    }
}

/// Minimal RFC822 — headers + first text/plain or text/html body.
pub fn extract_eml(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("EML read '{}': {}", path.display(), e))?;
    let (headers, body) = split_headers_body(&raw);
    let mut out = String::new();
    for line in headers.lines().take(40) {
        let l = line.trim();
        if l.starts_with("Subject:")
            || l.starts_with("From:")
            || l.starts_with("To:")
            || l.starts_with("Date:")
        {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&extract_eml_body(body));
    Ok(out.trim().to_string())
}

fn split_headers_body(raw: &str) -> (&str, &str) {
    if let Some(pos) = raw.find("\r\n\r\n") {
        return (&raw[..pos], &raw[pos + 4..]);
    }
    if let Some(pos) = raw.find("\n\n") {
        return (&raw[..pos], &raw[pos + 2..]);
    }
    ("", raw)
}

fn extract_eml_body(body: &str) -> String {
    let lower = body.to_ascii_lowercase();
    if lower.contains("content-type: multipart") {
        if let Some(boundary) = parse_boundary(&lower) {
            return extract_multipart(body, &boundary);
        }
    }
    if lower.contains("content-type: text/html") {
        return crate::formats::strip_html_for_extra(body);
    }
    body.trim().to_string()
}

fn parse_boundary(lower_headers: &str) -> Option<String> {
    let re = regex::Regex::new(r#"boundary="?([^";\s]+)"?"#).unwrap();
    re.captures(lower_headers)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn extract_multipart(body: &str, boundary: &str) -> String {
    let delim = format!("--{boundary}");
    let mut best = String::new();
    for part in body.split(&delim) {
        let part = part.trim();
        if part.is_empty() || part == "--" {
            continue;
        }
        let (part_headers, part_body) = split_headers_body(part);
        let pl = part_headers.to_ascii_lowercase();
        if pl.contains("text/plain") {
            return part_body.trim().to_string();
        }
        if pl.contains("text/html") && best.is_empty() {
            best = crate::formats::strip_html_for_extra(part_body);
        }
    }
    best
}

/// Unix mbox — split on "^From " lines, parse each message like EML body.
pub fn extract_mbox(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("MBOX read '{}': {}", path.display(), e))?;
    let mut messages = Vec::new();
    let mut current = String::new();
    for line in raw.lines() {
        if line.starts_with("From ") && !current.is_empty() {
            messages.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        messages.push(current);
    }
    let mut out = String::new();
    for (i, msg) in messages.iter().enumerate() {
        if i > 0 {
            out.push_str("\n---\n\n");
        }
        let (headers, body) = split_headers_body(msg);
        for line in headers.lines().take(8) {
            let l = line.trim();
            if l.starts_with("Subject:") || l.starts_with("From:") {
                out.push_str(l);
                out.push('\n');
            }
        }
        out.push_str(&extract_eml_body(body));
        out.push('\n');
    }
    Ok(out.trim().to_string())
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_strips_controls() {
        let t = strip_rtf_fast(r#"{\rtf1\ansi Hello \par World}"#);
        assert!(t.contains("Hello"));
        assert!(t.contains("World"));
    }
}
