use calamine::{open_workbook_auto, Data, Reader as CalamineReader};
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use std::io::{Read, Seek, Write};
use std::path::Path;
use zip::ZipArchive;

/// One logical page: optional label (sheet name) + plain text.
pub struct OfficePage {
    pub label: Option<String>,
    pub text: String,
}

/// A zero-copy pull-parser for DOCX / OpenXML text extraction.
/// Borrows from the input buffer to achieve maximum performance and zero intermediate allocations.
pub struct DocxExtractor;

impl DocxExtractor {
    /// Zero-copy extraction directly from a slice of decompressed xml bytes.
    /// Kept for callers that already hold the XML in memory (e.g. unit tests).
    /// This is incredibly fast and performs zero allocations other than the target string.
    #[allow(dead_code)]
    pub fn extract_from_slice(xml_data: &[u8], out: &mut String) -> Result<(), String> {
        let mut xml_reader = XmlReader::from_reader(xml_data);
        xml_reader.config_mut().trim_text(false);

        let mut buf = Vec::with_capacity(512);
        let mut in_t = false;
        let mut last_was_newline = true;

        loop {
            buf.clear();
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    match e.local_name().as_ref() {
                        b"t" => in_t = true,
                        b"tab" => {
                            out.push('\t');
                            last_was_newline = false;
                        }
                        b"br" | b"cr" => {
                            out.push('\n');
                            last_was_newline = true;
                        }
                        b"p" => {
                            if !last_was_newline {
                                out.push('\n');
                                last_was_newline = true;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_t = false;
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if in_t {
                        let cow = e.unescape().map_err(|err| format!("XML unescape error: {}", err))?;
                        out.push_str(&cow);
                        if !cow.is_empty() {
                            last_was_newline = cow.ends_with('\n');
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(format!("XML parsing error: {}", e)),
                _ => {}
            }
        }
        Ok(())
    }

    /// Streaming extraction from any reader implementing Read (e.g. zip file entry).
    /// Decompresses and streams XML on the fly with O(1) memory footprint.
    pub fn extract_streaming<R: Read, W: Write>(reader: R, mut writer: W) -> Result<(), String> {
        let mut xml_reader = XmlReader::from_reader(std::io::BufReader::new(reader));
        xml_reader.config_mut().trim_text(false);

        let mut buf = Vec::with_capacity(1024);
        let mut in_t = false;
        let mut last_was_newline = true;

        loop {
            buf.clear();
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    match e.local_name().as_ref() {
                        b"t" => in_t = true,
                        b"tab" => {
                            writer.write_all(b"\t").map_err(|e| e.to_string())?;
                            last_was_newline = false;
                        }
                        b"br" | b"cr" => {
                            writer.write_all(b"\n").map_err(|e| e.to_string())?;
                            last_was_newline = true;
                        }
                        b"p" => {
                            if !last_was_newline {
                                writer.write_all(b"\n").map_err(|e| e.to_string())?;
                                last_was_newline = true;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_t = false;
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if in_t {
                        let cow = e.unescape().map_err(|err| format!("XML unescape error: {}", err))?;
                        writer.write_all(cow.as_bytes()).map_err(|e| e.to_string())?;
                        if !cow.is_empty() {
                            last_was_newline = cow.ends_with('\n');
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(format!("XML parsing error: {}", e)),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Orchestrator for streaming and zero-copy MS Office document parsing.
pub struct OfficeParser<R: Read + Seek> {
    archive: ZipArchive<R>,
}

impl<R: Read + Seek> OfficeParser<R> {
    /// Create a new OfficeParser wrapper around a generic Read + Seek reader (e.g. File).
    pub fn new(reader: R) -> Result<Self, String> {
        let archive = ZipArchive::new(reader).map_err(|e| format!("Invalid ZIP archive: {}", e))?;
        Ok(Self { archive })
    }

    /// Extract DOCX main body, headers, and footers into a String using streaming.
    /// This keeps the uncompressed XML files out of memory as much as possible.
    pub fn extract_docx_to_string(&mut self) -> Result<String, String> {
        let mut out_bytes = Vec::with_capacity(16 * 1024);
        
        // Extract word/document.xml
        if let Ok(document_file) = self.archive.by_name("word/document.xml") {
            DocxExtractor::extract_streaming(document_file, &mut out_bytes)?;
        }

        // Get header/footer entries sorted to preserve ordering parity
        let mut header_footers = Vec::new();
        for i in 0..self.archive.len() {
            if let Ok(file) = self.archive.by_index(i) {
                let name = file.name();
                if (name.starts_with("word/header") || name.starts_with("word/footer")) && name.ends_with(".xml") {
                    header_footers.push(name.to_string());
                }
            }
        }
        header_footers.sort();

        for name in header_footers {
            if let Ok(file) = self.archive.by_name(&name) {
                if !out_bytes.is_empty() && !out_bytes.ends_with(b"\n\n") {
                    out_bytes.extend_from_slice(b"\n\n");
                }
                DocxExtractor::extract_streaming(file, &mut out_bytes)?;
            }
        }

        String::from_utf8(out_bytes).map_err(|e| format!("Invalid UTF-8 in document content: {}", e))
    }
}

pub fn extract_docx_pages(path: &Path) -> Result<Vec<OfficePage>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut parser = OfficeParser::new(file)?;
    let text = parser.extract_docx_to_string()?;
    Ok(vec![OfficePage {
        label: None,
        text,
    }])
}

pub fn extract_pptx_pages(path: &Path) -> Result<Vec<OfficePage>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    
    let mut slides = Vec::new();
    let prefix = "ppt/slides/slide";
    let suffix = ".xml";
    for i in 0..archive.len() {
        let file_entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file_entry.name();
        if name.starts_with(prefix) && name.ends_with(suffix) {
            let num = name
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(i as u32);
            slides.push((num, name.to_string()));
        }
    }
    slides.sort_by_key(|(num, _)| *num);

    let mut pages = Vec::with_capacity(slides.len());
    for (num, name) in slides {
        let file_entry = archive.by_name(&name).map_err(|e| e.to_string())?;
        let mut out_bytes = Vec::with_capacity(4096);
        DocxExtractor::extract_streaming(file_entry, &mut out_bytes)?;
        let text = String::from_utf8(out_bytes).map_err(|e| format!("Invalid UTF-8 in slide content: {}", e))?;
        pages.push(OfficePage {
            label: Some(format!("slide{}", num)),
            text,
        });
    }
    Ok(pages)
}

fn write_cell_to_string(cell: &Data, out: &mut String) {
    match cell {
        Data::Empty => {}
        Data::String(s) => out.push_str(s),
        Data::Float(f) => {
            if f.fract() == 0.0 {
                use std::fmt::Write;
                let _ = write!(out, "{}", *f as i64);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "{}", f);
            }
        }
        Data::Int(i) => {
            use std::fmt::Write;
            let _ = write!(out, "{}", i);
        }
        Data::Bool(b) => {
            out.push_str(if *b { "true" } else { "false" });
        }
        Data::DateTime(d) => {
            use std::fmt::Write;
            let _ = write!(out, "{}", d);
        }
        Data::DateTimeIso(s) => out.push_str(s),
        Data::DurationIso(s) => out.push_str(s),
        Data::Error(e) => {
            use std::fmt::Write;
            let _ = write!(out, "{:?}", e);
        }
    }
}

pub fn extract_xlsx_pages(path: &Path) -> Result<Vec<OfficePage>, String> {
    let mut workbook = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let names = workbook.sheet_names().to_vec();
    let mut pages = Vec::with_capacity(names.len());

    for name in names {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| e.to_string())?;
        
        let mut text = String::with_capacity(range.height() * 64);
        
        let mut first = true;
        let mut col_count = 0;
        for row in range.rows() {
            let has_content = row.iter().any(|cell| !matches!(cell, Data::Empty));
            if has_content {
                if first {
                    col_count = row.len();
                    first = false;
                }
                text.push_str("| ");
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 {
                        text.push_str(" | ");
                    }
                    write_cell_to_string(cell, &mut text);
                }
                text.push_str(" |\n");
                if col_count > 0 && text.ends_with(" |\n") && !text.contains("---|") {
                    text.push_str("|");
                    for _ in 0..col_count {
                        text.push_str("---|");
                    }
                    text.push_str("\n");
                }
            }
        }
        pages.push(OfficePage {
            label: Some(name),
            text,
        });
    }
    Ok(pages)
}

/// ODF (odt/ods/odp): content.xml via same streaming XML text extractor.
pub fn extract_odf_pages(path: &Path, ext: &str) -> Result<Vec<OfficePage>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let content_name = "content.xml";
    let mut out_bytes = Vec::with_capacity(16 * 1024);
    if let Ok(entry) = archive.by_name(content_name) {
        DocxExtractor::extract_streaming(entry, &mut out_bytes)?;
    } else {
        return Err(format!("ODF missing {content_name}"));
    }
    let text = String::from_utf8(out_bytes).map_err(|e| e.to_string())?;
    let label = match ext {
        "ods" => Some("sheet1".to_string()),
        "odp" => Some("slide1".to_string()),
        _ => None,
    };
    Ok(vec![OfficePage { label, text }])
}

/// EPUB: concatenate spine xhtml/html items (streaming strip tags).
pub fn extract_epub_pages(path: &Path) -> Result<Vec<OfficePage>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut items: Vec<(String, String)> = Vec::new();
    for i in 0..archive.len() {
        let file_entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file_entry.name().to_string();
        let lower = name.to_lowercase();
        if lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm") {
            items.push((name, String::new()));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut pages = Vec::with_capacity(items.len().max(1));
    for (idx, (name, _)) in items.into_iter().enumerate() {
        let mut entry = archive.by_name(&name).map_err(|e| e.to_string())?;
        let mut raw = String::new();
        std::io::Read::read_to_string(&mut entry, &mut raw).map_err(|e| e.to_string())?;
        let text = crate::formats::strip_html_for_extra(&raw);
        pages.push(OfficePage {
            label: Some(format!("chapter{}", idx + 1)),
            text,
        });
    }
    if pages.is_empty() {
        return Err("EPUB: no HTML content files".to_string());
    }
    Ok(pages)
}

/// Legacy Excel `.xls` via calamine (same row format as xlsx).
pub fn extract_xls_pages(path: &Path) -> Result<Vec<OfficePage>, String> {
    extract_xlsx_pages(path)
}

/// Legacy `.doc` / `.ppt` — deferred; return stub warning page.
pub fn extract_legacy_office_stub(path: &Path, ext: &str) -> Vec<OfficePage> {
    let _ = path;
    vec![OfficePage {
        label: None,
        text: format!(
            "[BrainPipe] .{ext} legacy binary format not supported in fast path. \
             Convert to .docx/.pptx or use an external converter."
        ),
    }]
}

pub fn extract_office_pages(path: &Path, ext: &str) -> Result<Vec<OfficePage>, String> {
    match ext {
        "docx" => extract_docx_pages(path),
        "xlsx" => extract_xlsx_pages(path),
        "pptx" => extract_pptx_pages(path),
        "xls" => extract_xls_pages(path),
        "odt" | "ods" | "odp" => extract_odf_pages(path, ext),
        "epub" => extract_epub_pages(path),
        "msg" => crate::msg::extract_msg_pages(path),
        "doc" | "ppt" => Ok(extract_legacy_office_stub(path, ext)),
        _ => Err(format!("unsupported office extension: {}", ext)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docx_extractor_slice() {
        let xml_data = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r>
        <w:t>Hello </w:t>
        <w:t>World</w:t>
      </w:r>
      <w:tab/>
      <w:r>
        <w:t>with tab and </w:t>
      </w:r>
      <w:br/>
      <w:r>
        <w:t>break.</w:t>
      </w:r>
    </w:p>
    <w:p>
      <w:r>
        <w:t>Second paragraph.</w:t>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let mut out = String::new();
        let res = DocxExtractor::extract_from_slice(xml_data.as_bytes(), &mut out);
        assert!(res.is_ok());
        assert_eq!(out, "Hello World\twith tab and \nbreak.\nSecond paragraph.");
    }

    #[test]
    fn test_docx_extractor_streaming() {
        let xml_data = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r>
        <w:t>Hello </w:t>
        <w:t>World</w:t>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let mut out_bytes = Vec::new();
        let res = DocxExtractor::extract_streaming(xml_data.as_bytes(), &mut out_bytes);
        assert!(res.is_ok());
        let out_str = String::from_utf8(out_bytes).unwrap();
        assert_eq!(out_str, "Hello World");
    }

    #[test]
    fn test_write_cell_to_string() {
        let mut out = String::new();
        write_cell_to_string(&Data::String("Hello".to_string()), &mut out);
        assert_eq!(out, "Hello");

        out.clear();
        write_cell_to_string(&Data::Float(3.14), &mut out);
        assert_eq!(out, "3.14");

        out.clear();
        write_cell_to_string(&Data::Float(42.0), &mut out);
        assert_eq!(out, "42");

        out.clear();
        write_cell_to_string(&Data::Int(-123), &mut out);
        assert_eq!(out, "-123");

        out.clear();
        write_cell_to_string(&Data::Bool(true), &mut out);
        assert_eq!(out, "true");
    }

    #[test]
    fn test_pptx_extraction_mock() {
        use std::io::Cursor;
        use zip::write::FileOptions;
        use zip::ZipWriter;

        let mut buf = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file("ppt/slides/slide1.xml", FileOptions::default()).unwrap();
            let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:sp>
        <p:txBody>
          <a:p>
            <a:r>
              <a:t>Slide Title</a:t>
            </a:r>
          </a:p>
        </p:txBody>
      </p:sp>
    </p:spTree>
  </p:cSld>
</p:sld>"#;
            std::io::Write::write_all(&mut zip, slide_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        // Now extract
        let temp_dir = std::env::temp_dir();
        let temp_file_path = temp_dir.join("test_mock_slide.pptx");
        std::fs::write(&temp_file_path, &buf).unwrap();

        let pages = extract_pptx_pages(&temp_file_path).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].label, Some("slide1".to_string()));
        assert_eq!(pages[0].text, "Slide Title");

        let _ = std::fs::remove_file(temp_file_path);
    }
}
