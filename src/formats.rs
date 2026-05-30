//! Fast parsers for HTML, CSV, JSON, and raster images.

use std::path::Path;

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "pdf", "txt", "md", "docx", "xlsx", "pptx", "xls", "doc", "ppt",
    "odt", "ods", "odp", "rtf", "epub", "tex", "rst",
    "png", "jpg", "jpeg", "tiff", "tif", "webp", "bmp",
    "html", "htm", "csv", "json", "xml", "yaml", "yml", "ndjson", "log",
    "eml", "msg", "mbox",
    "py", "js", "ts", "rs", "go", "java", "c", "cpp", "h", "cs", "rb", "php",
    "zip",
];

/// Plain-text code / markup extensions (read as UTF-8).
pub const PLAIN_TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "tex", "rst", "log", "yaml", "yml",
    "py", "js", "ts", "rs", "go", "java", "c", "cpp", "h", "cs", "rb", "php",
];

pub fn is_supported_extension(ext: &str) -> bool {
    let e = ext.to_lowercase();
    SUPPORTED_EXTENSIONS.contains(&e.as_str())
}

pub fn extract_html(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("HTML read '{}': {}", path.display(), e))?;
    Ok(strip_html_fast(&raw))
}

/// Tag stripping without a browser — tuned for speed.
fn strip_html_fast(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}[^>]*>.*?</{tag}>")).unwrap();
        s = re.replace_all(&s, "\n").into_owned();
    }
    let re_tags = regex::Regex::new(r"(?is)<br\s*/?>").unwrap();
    s = re_tags.replace_all(&s, "\n").into_owned();
    let re_block = regex::Regex::new(r"(?is)</(p|div|h[1-6]|li|tr)>").unwrap();
    s = re_block.replace_all(&s, "\n").into_owned();
    let re_all = regex::Regex::new(r"(?is)<[^>]+>").unwrap();
    s = re_all.replace_all(&s, " ").into_owned();
    let re_ws = regex::Regex::new(r"[ \t]+\n").unwrap();
    s = re_ws.replace_all(&s, "\n").into_owned();
    let re_multi = regex::Regex::new(r"\n{3,}").unwrap();
    s = re_multi.replace_all(&s, "\n\n").into_owned();
    html_unescape_basic(&s).trim().to_string()
}

fn html_unescape_basic(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

pub fn extract_csv(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("CSV read '{}': {}", path.display(), e))?;
    let mut out = String::new();
    for (i, line) in raw.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    Ok(out)
}

pub fn extract_json(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("JSON read '{}': {}", path.display(), e))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("JSON parse: {}", e))?;
    if v.is_object() || v.is_array() {
        Ok(flatten_json_value(&v, 0))
    } else {
        Ok(serde_json::to_string_pretty(&v).unwrap_or(raw))
    }
}

fn flatten_json_value(v: &serde_json::Value, depth: usize) -> String {
    const MAX_DEPTH: usize = 12;
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(|x| flatten_json_value(x, depth + 1))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(map) => {
            if depth >= MAX_DEPTH {
                return serde_json::to_string_pretty(v).unwrap_or_default();
            }
            let mut lines = Vec::new();
            for (k, val) in map {
                let inner = flatten_json_value(val, depth + 1);
                if inner.contains('\n') {
                    lines.push(format!("{k}:\n{inner}"));
                } else if !inner.is_empty() {
                    lines.push(format!("{k}: {inner}"));
                }
            }
            lines.join("\n")
        }
    }
}

/// RGB8 raw bytes + dimensions for OCR pipeline.
pub struct ImagePageData {
    pub width: u32,
    pub height: u32,
    pub rgb_bytes: Vec<u8>,
}

pub fn load_image_for_ocr(path: &Path) -> Result<ImagePageData, String> {
    let img = image::open(path).map_err(|e| format!("Image open '{}': {}", path.display(), e))?;
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    Ok(ImagePageData {
        width,
        height,
        rgb_bytes: rgb.into_raw(),
    })
}

pub fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "tiff" | "tif" | "webp" | "bmp"
    )
}

pub fn is_plain_text_extension(ext: &str) -> bool {
    PLAIN_TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

pub fn is_archive_extension(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("zip")
}

/// Shared HTML strip for EML bodies.
pub fn strip_html_for_extra(html: &str) -> String {
    strip_html_fast(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_removes_tags() {
        let html = "<html><body><p>Hello <b>world</b></p></body></html>";
        let t = strip_html_fast(html);
        assert!(t.contains("Hello"));
        assert!(!t.contains("<p>"));
    }

    #[test]
    fn flatten_json_object() {
        let v: serde_json::Value = serde_json::json!({"a": 1, "b": {"c": "x"}});
        let s = flatten_json_value(&v, 0);
        assert!(s.contains("a: 1"));
        assert!(s.contains("c: x"));
    }

    #[test]
    fn supported_extensions_include_new_formats() {
        assert!(is_supported_extension("html"));
        assert!(is_supported_extension("png"));
        assert!(!is_supported_extension("exe"));
    }
}
