//! Aggregate ingest statistics (competitor-style run summary).

use pyo3::prelude::*;
use std::collections::HashMap;

use crate::DocumentPage;

#[pyclass(get_all)]
#[derive(Clone, Debug)]
pub struct IngestReport {
    pub files_ok: usize,
    pub files_failed: usize,
    pub pages_total: usize,
    pub pages_ocr: usize,
    pub avg_confidence: f32,
    pub repair_count: usize,
    pub by_extension: HashMap<String, usize>,
}

#[pymethods]
impl IngestReport {
    fn __repr__(&self) -> String {
        format!(
            "IngestReport(files_ok={}, files_failed={}, pages={}, ocr_pages={}, avg_confidence={:.2}, repairs={})",
            self.files_ok, self.files_failed, self.pages_total, self.pages_ocr, self.avg_confidence, self.repair_count
        )
    }
}

pub fn build_ingest_report(pages: &[DocumentPage]) -> IngestReport {
    let mut by_path: HashMap<String, bool> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();
    let mut conf_sum = 0.0f32;
    let mut pages_ocr = 0usize;
    let mut repair_count = 0usize;

    for p in pages {
        let failed = p.error.is_some() || p.content.trim().is_empty() && p.extraction_confidence < 0.2;
        by_path
            .entry(p.path.clone())
            .and_modify(|ok| *ok = *ok && !failed)
            .or_insert(!failed);
        if p.ocr_used {
            pages_ocr += 1;
        }
        conf_sum += p.extraction_confidence;
        if p.metadata.get("xref_repaired").is_some()
            || p.metadata.get("repair_method").is_some()
            || p.metadata.get("pdf_open").map(|s| s.contains("repair")).unwrap_or(false)
        {
            repair_count += 1;
        }
        let ext = Path::new(&p.path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        *by_ext.entry(ext).or_insert(0) += 1;
    }

    let files_ok = by_path.values().filter(|&&ok| ok).count();
    let files_failed = by_path.len().saturating_sub(files_ok);
    let n = pages.len().max(1);

    IngestReport {
        files_ok,
        files_failed,
        pages_total: pages.len(),
        pages_ocr,
        avg_confidence: conf_sum / n as f32,
        repair_count,
        by_extension: by_ext,
    }
}

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report() {
        let r = build_ingest_report(&[]);
        assert_eq!(r.pages_total, 0);
    }
}
