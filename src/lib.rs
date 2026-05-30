#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use pyo3::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use pdfium_render::prelude::*;
use std::sync::mpsc;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, LazyLock};
use std::thread;
use crossbeam_channel;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model as RtenModel;
use image::GenericImageView;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::execution_providers::{CUDAExecutionProvider, DirectMLExecutionProvider, CoreMLExecutionProvider};
use xxhash_rust::xxh3::xxh3_64;

mod archives;
mod extra_formats;
mod formats;
mod ingest_report;
mod msg;
mod ocr_enhance;
mod office;
mod page_quality;
mod pdf_repair;
mod pii;
mod reading_order;

#[pyclass(get_all)]
#[derive(Serialize, Deserialize, Clone)]
pub struct ChildChunk {
    pub text: String,
    pub parent_index: usize,
}

#[pymethods]
impl ChildChunk {
    fn __repr__(&self) -> String { format!("ChildChunk(text_len={}, parent_index={})", self.text.len(), self.parent_index) }
    fn __str__(&self) -> String { self.__repr__() }
}

#[pyclass(get_all)]
#[derive(Serialize, Deserialize, Clone)]
pub struct DocumentPage {
    pub path: String,
    pub page_index: usize,
    pub content: String,
    pub chunks: Vec<String>,
    pub child_chunks: Vec<ChildChunk>,
    pub metadata: HashMap<String, String>,
    pub form_fields: HashMap<String, String>,
    pub num_chunks: usize,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub extraction_confidence: f32,
    #[serde(default)]
    pub ocr_used: bool,
    #[serde(default)]
    pub text_density: f32,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub language: Option<String>,
}

fn finish_document_page(
    path: String,
    page_index: usize,
    mut content: String,
    mut chunks: Vec<String>,
    child_chunks: Vec<ChildChunk>,
    mut metadata: HashMap<String, String>,
    form_fields: HashMap<String, String>,
    error: Option<String>,
    ocr_used: bool,
    width: u32,
    height: u32,
    warnings: Vec<String>,
    pii_mode: pii::PiiMode,
) -> DocumentPage {
    let (pii_count, pii_types) = apply_pii_to_page(&mut content, &mut chunks, pii_mode);
    if pii_count > 0 {
        metadata.insert("pii_redacted".to_string(), pii_count.to_string());
        metadata.insert("pii_types".to_string(), pii_types.join(","));
    }
    let text_len = content.len();
    let density = page_quality::text_density(text_len, width, height);
    let had_error = error.is_some();
    let confidence = page_quality::extraction_confidence(text_len, ocr_used, had_error, density);
    let language = page_quality::detect_language_hint(&content);
    if ocr_used {
        metadata.insert("ocr_used".to_string(), "true".to_string());
    }
    if let Some(ref e) = error {
        metadata.insert("page_error".to_string(), e.clone());
    }
    DocumentPage {
        path,
        page_index,
        num_chunks: chunks.len(),
        content,
        chunks,
        child_chunks,
        metadata,
        form_fields,
        error,
        extraction_confidence: confidence,
        ocr_used,
        text_density: density,
        warnings,
        language,
    }
}

fn apply_pii_to_page(content: &mut String, chunks: &mut [String], mode: pii::PiiMode) -> (usize, Vec<String>) {
    if mode == pii::PiiMode::Off {
        return (0, Vec::new());
    }
    let (sanitized, count, types) = pii::sanitize_text(content, mode);
    *content = sanitized;
    for c in chunks.iter_mut() {
        let (s, _, _) = pii::sanitize_text(c, mode);
        *c = s;
    }
    (count, types)
}

/// Fast path for bulk PDF text: skips language scan and per-page metadata clones.
fn finish_document_page_bulk(
    path: Arc<str>,
    page_index: usize,
    content: String,
    chunks: Vec<String>,
    file_metadata: Option<&HashMap<String, String>>,
    pii_mode: pii::PiiMode,
) -> DocumentPage {
    let mut content = content;
    let mut chunks = chunks;
    let (pii_count, pii_types) = apply_pii_to_page(&mut content, &mut chunks, pii_mode);
    let text_len = content.len();
    let num_chunks = if chunks.is_empty() {
        usize::from(text_len > 0)
    } else {
        chunks.len()
    };
    let mut metadata = if page_index == 0 {
        file_metadata.cloned().unwrap_or_default()
    } else {
        HashMap::new()
    };
    if pii_count > 0 {
        metadata.insert("pii_redacted".to_string(), pii_count.to_string());
        metadata.insert("pii_types".to_string(), pii_types.join(","));
    }
    DocumentPage {
        path: path.to_string(),
        page_index,
        num_chunks,
        content,
        chunks,
        child_chunks: Vec::new(),
        metadata,
        form_fields: HashMap::new(),
        error: None,
        extraction_confidence: 0.75,
        ocr_used: false,
        text_density: 0.0,
        warnings: Vec::new(),
        language: None,
    }
}

fn send_error_page(
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    path_str: &str,
    page_index: usize,
    err: String,
    metadata: HashMap<String, String>,
) {
    let doc = finish_document_page(
        path_str.to_string(),
        page_index,
        String::new(),
        Vec::new(),
        Vec::new(),
        metadata,
        HashMap::new(),
        Some(err),
        false,
        0,
        0,
        vec!["pdf_open_failed".to_string()],
        pii::PiiMode::Off,
    );
    let _ = tx.send(Ok(doc));
}

#[pymethods]
impl DocumentPage {
    fn __repr__(&self) -> String { format!("DocumentPage(path='{}', page={}, chunks={}, content_len={})", self.path, self.page_index, self.num_chunks, self.content.len()) }
    fn __str__(&self) -> String { self.__repr__() }
}

enum ChunkStreamBackend {
    Channel(Arc<Mutex<mpsc::Receiver<PyResult<DocumentPage>>>>),
    Buffer {
        pages: Vec<DocumentPage>,
        index: AtomicUsize,
    },
}

#[pyclass(unsendable)]
pub struct ChunkStream {
    backend: ChunkStreamBackend,
}

impl ChunkStream {
    fn from_channel(rx: mpsc::Receiver<PyResult<DocumentPage>>) -> Self {
        ChunkStream {
            backend: ChunkStreamBackend::Channel(Arc::new(Mutex::new(rx))),
        }
    }

    fn from_vec(pages: Vec<DocumentPage>) -> Self {
        ChunkStream {
            backend: ChunkStreamBackend::Buffer {
                pages,
                index: AtomicUsize::new(0),
            },
        }
    }
}

#[pymethods]
impl ChunkStream {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> { slf }

    /// Drain all pages in one Rust loop (much faster than repeated __next__ from Python).
    fn drain_all(&self, py: Python<'_>) -> PyResult<Vec<DocumentPage>> {
        match &self.backend {
            ChunkStreamBackend::Buffer { pages, .. } => Ok(pages.clone()),
            ChunkStreamBackend::Channel(rx) => {
                let rx = rx.clone();
                py.allow_threads(move || {
                    let lock = rx.lock().unwrap();
                    let mut out = Vec::new();
                    while let Ok(item) = lock.recv() {
                        out.push(item?);
                    }
                    Ok(out)
                })
            }
        }
    }

    #[allow(deprecated)]
    fn __next__(slf: PyRefMut<'_, Self>, py: Python) -> Option<PyObject> {
        match &slf.backend {
            ChunkStreamBackend::Buffer { pages, index } => {
                let i = index.fetch_add(1, Ordering::Relaxed);
                if i >= pages.len() {
                    return None;
                }
                return Some(pages[i].clone().into_py(py));
            }
            ChunkStreamBackend::Channel(rx) => {
                let rx = rx.clone();
                py.allow_threads(move || {
                    let lock = rx.lock().unwrap();
                    lock.recv().ok()
                })
                .map(|res| match res {
                    Ok(doc) => doc.into_py(py),
                    Err(e) => e.into_py(py),
                })
            }
        }
    }
}

fn page_cache_hash(chars: &[CharInfo], page_index: usize) -> u64 {
    let mut bytes = Vec::with_capacity(chars.len() + 16);
    for c in chars {
        let mut buf = [0u8; 4];
        bytes.extend_from_slice(c.c.encode_utf8(&mut buf).as_bytes());
    }
    bytes.extend_from_slice(page_index.to_string().as_bytes());
    xxh3_64(&bytes)
}

fn page_cache_hash_text(text: &str, page_index: usize) -> u64 {
    let mut bytes = Vec::with_capacity(text.len() + 16);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(page_index.to_string().as_bytes());
    xxh3_64(&bytes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayoutMode {
    Auto,
    Bulk,
    MultiColumn,
}

impl LayoutMode {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(LayoutMode::Auto),
            "bulk" | "fast" => Ok(LayoutMode::Bulk),
            "multi_column" | "layout" | "multicolumn" => Ok(LayoutMode::MultiColumn),
            _ => Err(format!("Unknown layout_mode '{}'. Use auto, bulk, or multi_column.", s)),
        }
    }

    fn resolve_simple(self) -> Self {
        match self {
            LayoutMode::Auto => LayoutMode::Bulk,
            other => other,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PdfBackend {
    Auto,
    Pdfium,
    #[cfg(feature = "mupdf")]
    Mupdf,
}

impl PdfBackend {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(PdfBackend::Auto),
            "pdfium" => Ok(PdfBackend::Pdfium),
            #[cfg(feature = "mupdf")]
            "mupdf" => Ok(PdfBackend::Mupdf),
            #[cfg(not(feature = "mupdf"))]
            "mupdf" => Err("BrainPipe built without 'mupdf' feature. Use pdfium or rebuild with --features mupdf.".to_string()),
            _ => Err(format!("Unknown backend '{}'. Use auto, pdfium, or mupdf.", s)),
        }
    }

    fn resolve(self, _simple_text_path: bool) -> PdfBackend {
        match self {
            PdfBackend::Auto => {
                #[cfg(feature = "mupdf")]
                if _simple_text_path {
                    return PdfBackend::Mupdf;
                }
                PdfBackend::Pdfium
            }
            other => other,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IngestStrategy {
    Auto,
    Fast,
    HiRes,
    Ocr,
}

impl IngestStrategy {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(IngestStrategy::Auto),
            "fast" => Ok(IngestStrategy::Fast),
            "hi_res" | "hires" | "hi-res" => Ok(IngestStrategy::HiRes),
            "ocr" => Ok(IngestStrategy::Ocr),
            _ => Err(format!("Unknown strategy '{}'. Use fast, hi_res, ocr, or auto.", s)),
        }
    }
}

struct StrategyFlags {
    layout: LayoutMode,
    use_ocr: bool,
    use_vlm: bool,
    use_embeddings: bool,
    fast_chunk: bool,
}

fn sample_pdf_avg_chars(pdfium: &Pdfium, path: &Path, max_sample: usize) -> Option<f32> {
    let document = pdfium.load_pdf_from_file(path, None).ok()?;
    let n = document.pages().len() as usize;
    if n == 0 {
        return Some(0.0);
    }
    let sample = n.min(max_sample.max(1));
    let mut total = 0usize;
    for i in 0..sample {
        if let Ok(page) = document.pages().get(i as u16) {
            total += page.text().map(|t| t.all().len()).unwrap_or(0);
        }
    }
    Some(total as f32 / sample as f32)
}

fn apply_strategy(strategy: IngestStrategy, base: StrategyFlags) -> StrategyFlags {
    match strategy {
        IngestStrategy::Auto => base,
        IngestStrategy::Fast => StrategyFlags {
            layout: LayoutMode::Bulk,
            use_ocr: false,
            use_vlm: false,
            use_embeddings: false,
            fast_chunk: true,
        },
        IngestStrategy::HiRes => StrategyFlags {
            layout: LayoutMode::MultiColumn,
            use_ocr: false,
            use_vlm: false,
            use_embeddings: base.use_embeddings,
            fast_chunk: false,
        },
        IngestStrategy::Ocr => StrategyFlags {
            layout: base.layout,
            use_ocr: true,
            use_vlm: base.use_vlm,
            use_embeddings: base.use_embeddings,
            fast_chunk: base.fast_chunk,
        },
    }
}

fn extract_chars_from_text_obj(text_obj: &PdfPageText) -> Vec<CharInfo> {
    let mut chars = Vec::new();
    for c in text_obj.chars().iter() {
        if let Ok(bounds) = c.loose_bounds() {
            let ch = c
                .unicode_string()
                .unwrap_or_default()
                .chars()
                .next()
                .unwrap_or(' ');
            chars.push(CharInfo {
                c: ch,
                left: bounds.left().value,
                right: bounds.right().value,
                top: bounds.top().value,
                bottom: bounds.bottom().value,
            });
        }
    }
    chars
}

fn extract_page_raw(
    page: &PdfPage,
    layout: LayoutMode,
    need_chars_for_vlm: bool,
    need_chars_for_ocr_check: bool,
) -> (Option<String>, Vec<CharInfo>) {
    let text_obj = page.text().ok();
    match layout {
        LayoutMode::Bulk => {
            let bulk = text_obj.as_ref().map(|t| t.all()).unwrap_or_default();
            (Some(bulk), Vec::new())
        }
        LayoutMode::MultiColumn => {
            let chars = text_obj
                .as_ref()
                .map(extract_chars_from_text_obj)
                .unwrap_or_default();
            (None, chars)
        }
        LayoutMode::Auto => {
            let bulk = text_obj.as_ref().map(|t| t.all()).unwrap_or_default();
            if need_chars_for_vlm || (need_chars_for_ocr_check && bulk.trim().is_empty()) {
                let chars = text_obj
                    .as_ref()
                    .map(extract_chars_from_text_obj)
                    .unwrap_or_default();
                if need_chars_for_vlm && is_complex_layout(&chars) {
                    (None, chars)
                } else {
                    (Some(bulk), if need_chars_for_vlm { chars } else { Vec::new() })
                }
            } else {
                (Some(bulk), Vec::new())
            }
        }
    }
}

fn page_text_from_raw(raw: &RawPage) -> String {
    if let Some(ref bulk) = raw.bulk_text {
        if raw.chars.is_empty() || !is_complex_layout(&raw.chars) {
            return bulk.clone();
        }
    }
    extract_page_content_multi_column(&raw.chars)
}

fn page_cache_hash_raw(raw: &RawPage) -> u64 {
    if let Some(ref t) = raw.bulk_text {
        if raw.chars.is_empty() {
            return page_cache_hash_text(t, raw.index);
        }
    }
    page_cache_hash(&raw.chars, raw.index)
}

fn recursive_chunk_slices<'a>(text: &'a str, separators: &[&str], max_size: usize) -> Vec<&'a str> {
    if text.is_empty() {
        return Vec::new();
    }
    if text.len() <= max_size {
        return vec![text];
    }

    if separators.is_empty() {
        let mut chunks = Vec::new();
        let mut byte_idx = 0;
        let bytes = text.as_bytes();
        while byte_idx < bytes.len() {
            let mut end = (byte_idx + max_size).min(bytes.len());
            while end > byte_idx && !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == byte_idx {
                end = (byte_idx + max_size).min(bytes.len());
                while end < bytes.len() && !text.is_char_boundary(end) {
                    end += 1;
                }
            }
            chunks.push(&text[byte_idx..end]);
            byte_idx = end;
        }
        return chunks;
    }

    let sep = separators[0];
    let next_seps = &separators[1..];

    if sep.is_empty() {
        return recursive_chunk_slices(text, &[], max_size);
    }

    if !text.contains(sep) {
        return recursive_chunk_slices(text, next_seps, max_size);
    }

    let parts: Vec<&str> = text.split(sep).collect();
    let mut final_chunks = Vec::new();
    
    let text_start_ptr = text.as_ptr() as usize;
    let mut current_start_offset: Option<usize> = None;
    let mut current_end_offset = 0;

    for (i, part) in parts.iter().enumerate() {
        let part_start_offset = (part.as_ptr() as usize) - text_start_ptr;
        let part_end_offset = part_start_offset + part.len();

        let actual_part = if i > 0 {
            let sep_start_offset = part_start_offset - sep.len();
            &text[sep_start_offset..part_end_offset]
        } else {
            *part
        };

        if actual_part.is_empty() {
            continue;
        }

        let sub_chunks = if actual_part.len() > max_size {
            recursive_chunk_slices(actual_part, next_seps, max_size)
        } else {
            vec![actual_part]
        };

        for sub in sub_chunks {
            if sub.is_empty() {
                continue;
            }
            
            let sub_start_offset = (sub.as_ptr() as usize) - text_start_ptr;
            let sub_end_offset = sub_start_offset + sub.len();

            if let Some(start) = current_start_offset {
                if sub_end_offset - start > max_size {
                    final_chunks.push(&text[start..current_end_offset]);
                    current_start_offset = Some(sub_start_offset);
                }
            } else {
                current_start_offset = Some(sub_start_offset);
            }
            current_end_offset = sub_end_offset;
        }
    }

    if let Some(start) = current_start_offset {
        final_chunks.push(&text[start..current_end_offset]);
    }

    final_chunks
}

fn recursive_structural_chunk(text: &str, max_chunk_size: usize) -> Vec<String> {
    const SEPARATORS: &[&str] = &[
        "\n# ",
        "\n## ",
        "\n### ",
        "\n#### ",
        "\n\n",
        "\n",
        ". ",
        " ",
    ];
    let slices = recursive_chunk_slices(text, SEPARATORS, max_chunk_size);
    slices.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Chunking léger (paragraphes) — évite segment_text_into_units sur texte simple.
fn fast_semantic_chunk(text: &str, max_chunk_size: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let needs_full = memchr::memmem::find(text.as_bytes(), b"|").is_some()
        || text.contains("### VLM Extracted:")
        || text.contains("**Tableau Extrait");
    if needs_full {
        return semantic_chunking(text, max_chunk_size);
    }
    let para_count = text.matches("\n\n").count() + 1;
    let mut chunks = Vec::with_capacity(para_count.min(64));
    let mut current = String::with_capacity(max_chunk_size.min(text.len()));
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if !current.is_empty() && current.len() + para.len() + 2 > max_chunk_size {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
        if current.len() >= max_chunk_size {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() && !text.trim().is_empty() {
        chunks.push(text.trim().to_string());
    }
    chunks
}

fn chunk_page_text(
    page_text: &str,
    chunk_size: usize,
    use_embeddings: bool,
    fast_chunk: bool,
    pool: Option<&Arc<EmbeddingSessionPool>>,
    tok: Option<&Arc<WordPieceTokenizer>>,
    db: Option<&sled::Db>,
) -> Vec<String> {
    if use_embeddings {
        if let (Some(pool), Some(tok)) = (pool, tok) {
            return semantic_chunking_with_embeddings(page_text, chunk_size, pool, tok, db);
        }
    }
    if fast_chunk {
        fast_semantic_chunk(page_text, chunk_size)
    } else {
        semantic_chunking(page_text, chunk_size)
    }
}

/// Répartit les chunks documentaires entre pages proportionnellement à la taille du texte.
fn assign_doc_chunks_to_pages(page_texts: &[(usize, String)], doc_chunks: &[String]) -> Vec<(usize, Vec<String>)> {
    if page_texts.is_empty() {
        return Vec::new();
    }
    if doc_chunks.is_empty() {
        return page_texts
            .iter()
            .map(|(i, t)| (*i, vec![t.clone()]))
            .collect();
    }
    let total_len: usize = page_texts.iter().map(|(_, t)| t.len().max(1)).sum();
    let mut out = Vec::with_capacity(page_texts.len());
    let mut chunk_i = 0usize;
    for (idx, text) in page_texts {
        let share = ((doc_chunks.len() as f64) * (text.len().max(1) as f64) / (total_len as f64))
            .round() as usize;
        let share = share.max(1).min(doc_chunks.len().saturating_sub(chunk_i));
        let end = (chunk_i + share).min(doc_chunks.len());
        let mut page_chunks: Vec<String> = doc_chunks[chunk_i..end].to_vec();
        chunk_i = end;
        if page_chunks.is_empty() {
            page_chunks.push(text.clone());
        }
        out.push((*idx, page_chunks));
    }
    if chunk_i < doc_chunks.len() {
        if let Some(last) = out.last_mut() {
            last.1.extend(doc_chunks[chunk_i..].iter().cloned());
        }
    }
    out
}

/// Libère un slot du pool d'images (backpressure Opt-2).
struct ImagePermit {
    return_tx: crossbeam_channel::Sender<()>,
}

impl Drop for ImagePermit {
    fn drop(&mut self) {
        let _ = self.return_tx.send(());
    }
}

struct ImagePermitPool {
    available: crossbeam_channel::Receiver<()>,
    return_tx: crossbeam_channel::Sender<()>,
}

impl ImagePermitPool {
    fn new(max_slots: usize) -> Arc<Self> {
        let (return_tx, available) = crossbeam_channel::bounded(max_slots);
        for _ in 0..max_slots {
            let _ = return_tx.send(());
        }
        Arc::new(ImagePermitPool { available, return_tx })
    }

    fn acquire(self: &Arc<Self>) -> Option<ImagePermit> {
        self.available.recv().ok()?;
        Some(ImagePermit {
            return_tx: self.return_tx.clone(),
        })
    }
}

fn sled_get_page(db: &sled::Db, key: &[u8]) -> Option<DocumentPage> {
    let bytes = db.get(key).ok()??;
    if let Ok(decompressed) = lz4_flex::decompress_size_prepended(&bytes) {
        if let Ok(doc) = bincode::deserialize(&decompressed) {
            return Some(doc);
        }
    }
    bincode::deserialize(&bytes).ok()
}

fn sled_put_page(db: &sled::Db, key: &[u8], doc: &DocumentPage) {
    if let Ok(bytes) = bincode::serialize(doc) {
        let compressed = lz4_flex::compress_prepend_size(&bytes);
        let _ = db.insert(key, compressed);
    }
}

fn sled_get_embedding(db: &sled::Db, key: &[u8]) -> Option<(String, Vec<f32>)> {
    let bytes = db.get(key).ok()??;
    if let Ok(decompressed) = lz4_flex::decompress_size_prepended(&bytes) {
        if let Ok(v) = bincode::deserialize(&decompressed) {
            return Some(v);
        }
    }
    bincode::deserialize(&bytes).ok()
}

fn sled_put_embedding(db: &sled::Db, key: &[u8], val: &(String, Vec<f32>)) {
    if let Ok(bytes) = bincode::serialize(val) {
        let compressed = lz4_flex::compress_prepend_size(&bytes);
        let _ = db.insert(key, compressed);
    }
}

/// Pool de sessions ONNX pour éviter la contention sur un Mutex global (Opt-5).
struct EmbeddingSessionPool {
    available: crossbeam_channel::Receiver<Session>,
    return_tx: crossbeam_channel::Sender<Session>,
}

impl EmbeddingSessionPool {
    fn new(model_path: &Path, pool_size: usize) -> Result<Self, String> {
        let pool_size = pool_size.max(1).min(8);
        let (return_tx, available) = crossbeam_channel::bounded(pool_size);
        for _ in 0..pool_size {
            let session = Session::builder()
                .map_err(|e| format!("ORT build: {:?}", e))?
                .with_execution_providers([
                    CUDAExecutionProvider::default().build(),
                    DirectMLExecutionProvider::default().build(),
                    CoreMLExecutionProvider::default().build(),
                ])
                .map_err(|e| format!("ORT EP: {:?}", e))?
                .commit_from_file(model_path)
                .map_err(|e| format!("ORT load: {:?}", e))?;
            return_tx.send(session).map_err(|e| format!("pool init: {:?}", e))?;
        }
        Ok(EmbeddingSessionPool { available, return_tx })
    }

    fn with_session<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut session = self.available.recv().expect("embedding pool recv");
        let result = f(&mut session);
        let _ = self.return_tx.send(session);
        result
    }
}

thread_local! {
    static THREAD_PDFIUM: RefCell<Option<Pdfium>> = const { RefCell::new(None) };
}

fn with_thread_pdfium<F, R>(lib_path: &str, f: F) -> R
where
    F: FnOnce(&Pdfium) -> R,
{
    THREAD_PDFIUM.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let bindings = Pdfium::bind_to_library(lib_path)
                .or_else(|_| Pdfium::bind_to_system_library())
                .expect("Pdfium bind failed");
            *slot = Some(Pdfium::new(bindings));
        }
        f(slot.as_ref().unwrap())
    })
}

static PDFIUM_LIB_PATH: LazyLock<String> = LazyLock::new(|| {
    let lib_name = Pdfium::pdfium_platform_library_name_at_path("");
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(&lib_name);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    lib_name.to_string_lossy().to_string()
});

fn resolve_pdfium_path() -> &'static str {
    &PDFIUM_LIB_PATH
}

fn default_max_concurrent_images(use_vlm: bool) -> usize {
    if use_vlm {
        2
    } else {
        let n = num_cpus::get().max(2);
        ((n + 1) / 2).clamp(2, 6)
    }
}

thread_local! {
    static OCR_RGB_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn download_ocr_models_if_needed(py: Python<'_>) -> PyResult<(PathBuf, PathBuf)> {
    let cache_dir = Path::new(".brainpipe_cache");
    std::fs::create_dir_all(cache_dir).unwrap();
    let det_path = cache_dir.join("text-detection.rten");
    let rec_path = cache_dir.join("text-recognition.rten");
    if !det_path.exists() || !rec_path.exists() {
        let py_code = c"
import urllib.request, os
os.makedirs('.brainpipe_cache', exist_ok=True)
if not os.path.exists('.brainpipe_cache/text-detection.rten'): urllib.request.urlretrieve('https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten', '.brainpipe_cache/text-detection.rten')
if not os.path.exists('.brainpipe_cache/text-recognition.rten'): urllib.request.urlretrieve('https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten', '.brainpipe_cache/text-recognition.rten')
";
        py.run(py_code, None, None)?;
    }
    Ok((det_path, rec_path))
}

fn download_embedding_model_if_needed(py: Python<'_>) -> PyResult<(PathBuf, PathBuf)> {
    let cache_dir = Path::new(".brainpipe_cache");
    std::fs::create_dir_all(cache_dir).unwrap();
    let model_path = cache_dir.join("all-MiniLM-L6-v2.onnx");
    let vocab_path = cache_dir.join("vocab.txt");
    if !model_path.exists() || !vocab_path.exists() {
        let py_code = c"
import urllib.request, os
os.makedirs('.brainpipe_cache', exist_ok=True)
if not os.path.exists('.brainpipe_cache/all-MiniLM-L6-v2.onnx'): urllib.request.urlretrieve('https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx', '.brainpipe_cache/all-MiniLM-L6-v2.onnx')
if not os.path.exists('.brainpipe_cache/vocab.txt'): urllib.request.urlretrieve('https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/vocab.txt', '.brainpipe_cache/vocab.txt')
";
        py.run(py_code, None, None)?;
    }
    Ok((model_path, vocab_path))
}

fn download_nano_model_if_needed(_py: Python<'_>) -> PyResult<PathBuf> {
    let model_path = Path::new("models").join("nano_layout.onnx");
    if !model_path.exists() {
        return Err(PyValueError::new_err(format!("Model not found at {:?}. Run export_yolo.py first.", model_path)));
    }
    Ok(model_path)
}

struct WordPieceTokenizer {
    vocab: HashMap<String, i32>,
    unk_id: i32,
    cls_id: i32,
    sep_id: i32,
}
impl WordPieceTokenizer {
    fn new(vocab_str: &str) -> Self {
        let mut vocab = HashMap::new();
        let (mut unk_id, mut cls_id, mut sep_id) = (100, 101, 102);
        for (idx, line) in vocab_str.lines().enumerate() {
            let token = line.trim();
            vocab.insert(token.to_string(), idx as i32);
            if token == "[UNK]" { unk_id = idx as i32; } else if token == "[CLS]" { cls_id = idx as i32; } else if token == "[SEP]" { sep_id = idx as i32; }
        }
        WordPieceTokenizer { vocab, unk_id, cls_id, sep_id }
    }
    fn tokenize(&self, text: &str) -> Vec<i32> {
        let mut input_ids = vec![self.cls_id];
        let clean_text = text.to_lowercase();
        for word in clean_text.split_whitespace() {
            let chars: Vec<char> = word.chars().collect();
            let mut start = 0;
            while start < chars.len() {
                let mut end = chars.len();
                let mut cur_subtoken = None;
                while start < end {
                    let mut substr: String = chars[start..end].iter().collect();
                    if start > 0 { substr = format!("##{}", substr); }
                    if let Some(&id) = self.vocab.get(&substr) { cur_subtoken = Some(id); break; }
                    end -= 1;
                }
                if let Some(id) = cur_subtoken { input_ids.push(id); start = end; } else { input_ids.push(self.unk_id); break; }
            }
        }
        input_ids.push(self.sep_id);
        input_ids
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0; let mut norm_a = 0.0; let mut norm_b = 0.0;
    for i in 0..a.len() { dot += a[i] * b[i]; norm_a += a[i] * a[i]; norm_b += b[i] * b[i]; }
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a.sqrt() * norm_b.sqrt()) }
}

struct NanoLayoutEngine {
    session: Mutex<Session>,
}
impl NanoLayoutEngine {
    fn new(model_path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| format!("ORT build error: {:?}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("ORT opt error: {:?}", e))?
            .with_execution_providers([
                CUDAExecutionProvider::default().build(),
                DirectMLExecutionProvider::default().build(),
                CoreMLExecutionProvider::default().build(),
            ])
            .map_err(|e| format!("ORT execution provider error: {:?}", e))?
            .commit_from_file(model_path)
            .map_err(|e| format!("Failed to load Nano ONNX model: {:?}", e))?;
        Ok(NanoLayoutEngine { session: Mutex::new(session) })
    }
    fn extract_table(&self, img_bytes: &[u8], _w: u32, _h: u32, chars: &[CharInfo]) -> Result<String, String> {
        // Load image from raw bytes
        let img = image::load_from_memory(img_bytes).map_err(|e| format!("Image load error: {}", e))?;
        let (orig_w, orig_h) = img.dimensions();
        // Resize to 640x640 for YOLO input
        let resized = img.resize_exact(640, 640, image::imageops::FilterType::Triangle);
        let rgb = resized.to_rgb8();
        
        let mut input_tensor = vec![0.0f32; 3 * 640 * 640];
        for y in 0..640 {
            for x in 0..640 {
                let pixel = rgb.get_pixel(x as u32, y as u32);
                input_tensor[0 * 640 * 640 + y * 640 + x] = pixel[0] as f32 / 255.0;
                input_tensor[1 * 640 * 640 + y * 640 + x] = pixel[1] as f32 / 255.0;
                input_tensor[2 * 640 * 640 + y * 640 + x] = pixel[2] as f32 / 255.0;
            }
        }
        
        let input_tensor_val = ort::value::Tensor::from_array((vec![1, 3, 640, 640], input_tensor)).unwrap();
        let inputs = ort::inputs![input_tensor_val];
        let mut session_guard = self.session.lock().unwrap();
        let outputs = session_guard.run(inputs)
            .map_err(|e| format!("ONNX inference error: {}", e))?;
            
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(|e| format!("Tensor extract error: {}", e))?;
        // shape should be [1, 6, 8400]
        
        let num_anchors = shape[2] as usize;
        let num_features = shape[1] as usize;
        
        let mut boxes: Vec<(f32, f32, f32, f32, f32)> = Vec::new(); // x1,y1,x2,y2,conf
        
        for i in 0..num_anchors {
            let mut max_conf = 0.0f32;
            for c in 4..num_features {
                let conf = data[c * num_anchors + i];
                if conf > max_conf { max_conf = conf; }
            }
            if max_conf < 0.25 { continue; }
            
            let xc = data[0 * num_anchors + i];
            let yc = data[1 * num_anchors + i];
            let w = data[2 * num_anchors + i];
            let h = data[3 * num_anchors + i];
            
            let x1 = xc - w / 2.0;
            let y1 = yc - h / 2.0;
            let x2 = xc + w / 2.0;
            let y2 = yc + h / 2.0;
            
            boxes.push((x1, y1, x2, y2, max_conf));
        }
        
        // NMS
        boxes.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
        let mut kept: Vec<(f32,f32,f32,f32,f32)> = Vec::new();
        for (x1,y1,x2,y2,conf) in boxes {
            let mut overlap = false;
            for (kx1,ky1,kx2,ky2,_) in &kept {
                let ix1 = x1.max(*kx1);
                let iy1 = y1.max(*ky1);
                let ix2 = x2.min(*kx2);
                let iy2 = y2.min(*ky2);
                if ix2 > ix1 && iy2 > iy1 {
                    let inter = (ix2 - ix1) * (iy2 - iy1);
                    let area1 = (x2 - x1) * (y2 - y1);
                    let area2 = (kx2 - kx1) * (ky2 - ky1);
                    let iou = inter / (area1 + area2 - inter);
                    if iou > 0.45 { overlap = true; break; }
                }
            }
            if !overlap { kept.push((x1,y1,x2,y2,conf)); }
        }
        
        // Scale back to original image size
        let scale_x = orig_w as f32 / 640.0;
        let scale_y = orig_h as f32 / 640.0;
        let scaled: Vec<(f32, f32, f32, f32, f32)> = kept
            .iter()
            .map(|(x1, y1, x2, y2, conf)| {
                (
                    x1 * scale_x,
                    y1 * scale_y,
                    x2 * scale_x,
                    y2 * scale_y,
                    *conf,
                )
            })
            .collect();
        let yolo_boxes: Vec<reading_order::BBox> = scaled
            .iter()
            .map(|(x1, y1, x2, y2, _)| reading_order::BBox::new(*x1, *y1, *x2, *y2))
            .collect();
        let box_order = reading_order::xy_cut_reading_order(
            &yolo_boxes,
            orig_w as f32,
            orig_h as f32,
            true,
        );
        let mut markdown = String::new();
        for &bi in &box_order {
            let (ox1, oy1, ox2, oy2, conf) = scaled[bi];
            
            // Collect chars inside box
            let mut chars_in_box: Vec<&CharInfo> = Vec::new();
            for c in chars {
                let cx = (c.left + c.right) / 2.0;
                let cy = (c.top + c.bottom) / 2.0;
                if cx >= ox1 && cx <= ox2 && cy >= oy1 && cy <= oy2 {
                    chars_in_box.push(c);
                }
            }
            // Group by row (tolerance of 5.0 in Y)
            chars_in_box.sort_by(|a, b| a.top.partial_cmp(&b.top).unwrap_or(std::cmp::Ordering::Equal));
            
            let mut rows: Vec<Vec<&CharInfo>> = Vec::new();
            for c in chars_in_box {
                if let Some(last_row) = rows.last_mut() {
                    let avg_y = last_row.iter().map(|ch| ch.top).sum::<f32>() / last_row.len() as f32;
                    if (c.top - avg_y).abs() <= 5.0 {
                        last_row.push(c);
                    } else {
                        rows.push(vec![c]);
                    }
                } else {
                    rows.push(vec![c]);
                }
            }
            
            let mut md_table = String::new();
            for row in rows {
                let mut sorted_row = row;
                sorted_row.sort_by(|a, b| a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal));
                
                let mut cells: Vec<String> = Vec::new();
                let mut current_cell = String::new();
                let mut last_x = -1.0;
                for c in sorted_row {
                    if last_x >= 0.0 && c.left - last_x > 15.0 {
                        cells.push(current_cell);
                        current_cell = String::new();
                    }
                    current_cell.push(c.c);
                    last_x = c.right;
                }
                if !current_cell.is_empty() {
                    cells.push(current_cell);
                }
                
                md_table.push_str("| ");
                md_table.push_str(&cells.join(" | "));
                md_table.push_str(" |\n");
            }
            
            markdown.push_str(&format!("**Tableau Extrait (Confiance {:.0}%)**\n", conf * 100.0));
            if md_table.is_empty() {
                markdown.push_str("| (Vide) |\n|---|\n");
            } else {
                let lines: Vec<&str> = md_table.trim().split('\n').collect();
                markdown.push_str(lines[0]);
                markdown.push_str("\n");
                let num_cols = lines[0].matches('|').count().saturating_sub(1);
                let num_cols = if num_cols == 0 { 1 } else { num_cols };
                markdown.push_str("|");
                for _ in 0..num_cols {
                    markdown.push_str("---|");
                }
                markdown.push_str("\n");
                for line in lines.into_iter().skip(1) {
                    markdown.push_str(line);
                    markdown.push_str("\n");
                }
            }
        }
        
        if markdown.is_empty() {
            return Ok(String::new());
        }
        
        Ok(markdown)
    }
}

#[derive(Clone, Debug)]
pub struct CharInfo {
    pub c: char,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

struct CellInfo {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    text: String,
}

fn is_complex_layout(chars: &[CharInfo]) -> bool {
    if chars.is_empty() { return false; }
    
    // Very simple heuristic for early-exit:
    // 1. If we have a huge number of characters, maybe it's complex? Actually no, long text is fine.
    // 2. We can check if characters heavily overlap.
    // 3. Or we check standard deviation of line heights.
    // For now, if we have very few characters but a large area, or many overlapping characters, we say true.
    let mut overlap_count = 0;
    let limit = chars.len().min(1000);
    for i in 0..limit {
        for j in (i+1)..limit.min(i+10) {
            let c1 = &chars[i];
            let c2 = &chars[j];
            // Check bounding box intersection
            if !(c1.right < c2.left || c1.left > c2.right || c1.bottom > c2.top || c1.top < c2.bottom) {
                overlap_count += 1;
            }
        }
    }
    // If more than 5% of characters overlap, it's likely a complex layout (table, chart, etc.)
    overlap_count > limit / 20
}

fn extract_page_content_multi_column(chars: &[CharInfo]) -> String {
    if chars.is_empty() { return String::new(); }
    
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    for c in chars {
        if c.left < min_x { min_x = c.left; }
        if c.right > max_x { max_x = c.right; }
    }
    let page_width = max_x - min_x;
    
    let mut sorted_chars: Vec<&CharInfo> = chars.iter().collect();
    sorted_chars.sort_by(|a, b| {
        let ay = (a.top + a.bottom) / 2.0;
        let by = (b.top + b.bottom) / 2.0;
        by.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut line_map: BTreeMap<i32, Vec<CharInfo>> = BTreeMap::new();
    for c in sorted_chars {
        let cy = (c.top + c.bottom) / 2.0;
        let h = c.top - c.bottom;
        let y_bucket = ((cy / (h * 0.45).max(4.0)).round()) as i32;
        let mut matched_bucket: Option<i32> = None;
        'outer: for delta in 0..=2i32 {
            for sign in [-1i32, 1i32] {
                let b = if delta == 0 { y_bucket } else { y_bucket + sign * delta };
                if let Some(line) = line_map.get(&b) {
                    if let Some(first) = line.first() {
                        let ly = (first.top + first.bottom) / 2.0;
                        let lh = first.top - first.bottom;
                        let tolerance = (h.max(lh) * 0.45).max(4.0);
                        if (cy - ly).abs() < tolerance {
                            matched_bucket = Some(b);
                            break 'outer;
                        }
                    }
                }
            }
        }
        let bucket = matched_bucket.unwrap_or(y_bucket);
        line_map.entry(bucket).or_default().push(c.clone());
    }

    let mut lines: Vec<Vec<CharInfo>> = line_map.into_values().collect();
    for line in lines.iter_mut() {
        line.sort_by(|a, b| a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal));
    }

    let mut all_cells = Vec::new();
    for line in lines {
        if line.is_empty() { continue; }
        let mut cells: Vec<CellInfo> = Vec::new();
        let mut current_text = String::new();
        let mut current_left = line[0].left;
        let mut current_right = line[0].right;
        let mut current_top = line[0].top;
        let mut current_bottom = line[0].bottom;
        current_text.push(line[0].c);
        
        for i in 1..line.len() {
            let prev = &line[i - 1];
            let curr = &line[i];
            let h = (prev.top - prev.bottom).max(curr.top - curr.bottom);
            let gap = curr.left - prev.right;
            let gap_threshold = (h * 1.5).max(12.0);
            
            if gap > gap_threshold {
                cells.push(CellInfo {
                    left: current_left,
                    right: current_right,
                    top: current_top,
                    bottom: current_bottom,
                    text: current_text.trim().to_string(),
                });
                current_text = String::new();
                current_left = curr.left;
                current_top = curr.top;
                current_bottom = curr.bottom;
            } else {
                let word_gap = (h * 0.25).max(2.0);
                if gap > word_gap { current_text.push(' '); }
            }
            current_text.push(curr.c);
            current_right = curr.right;
            current_top = current_top.max(curr.top);
            current_bottom = current_bottom.min(curr.bottom);
        }
        cells.push(CellInfo {
            left: current_left,
            right: current_right,
            top: current_top,
            bottom: current_bottom,
            text: current_text.trim().to_string(),
        });
        cells.retain(|c| !c.text.is_empty());
        all_cells.push(cells);
    }
    
    let flat_cells: Vec<CellInfo> = all_cells.into_iter().flatten().collect();
    if flat_cells.is_empty() {
        return String::new();
    }

    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for c in &flat_cells {
        if c.bottom < min_y {
            min_y = c.bottom;
        }
        if c.top > max_y {
            max_y = c.top;
        }
    }
    let page_height = (max_y - min_y).max(1.0);

    let bboxes: Vec<reading_order::BBox> = flat_cells
        .iter()
        .map(|c| reading_order::BBox::new(c.left, c.bottom, c.right, c.top))
        .collect();
    let order = reading_order::xy_cut_reading_order(&bboxes, page_width, page_height, false);

    let mut final_content = String::new();
    for &idx in &order {
        let cell = &flat_cells[idx];
        let cell_width = cell.right - cell.left;
        let text = &cell.text;
        if cell_width > page_width * 0.6 {
            let is_all_caps =
                text.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) && text.len() > 2;
            if is_all_caps && !text.ends_with('.') && text.len() < 80 {
                final_content.push_str("\n# ");
                final_content.push_str(text);
                final_content.push_str("\n\n");
                continue;
            }
        }
        final_content.push_str(text);
        final_content.push('\n');
    }

    final_content
}


fn split_into_sentences(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut sentences = Vec::new();
    let mut boundaries = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let punct = memchr::memchr3(b'.', b'!', b'?', &bytes[pos..]);
        let newline = memchr::memchr(b'\n', &bytes[pos..]);
        let found = match (punct, newline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        match found {
            Some(offset) => {
                let abs = pos + offset;
                if abs + 1 < bytes.len() && bytes[abs + 1].is_ascii_whitespace() {
                    boundaries.push(abs + 1);
                }
                pos = abs + 1;
            }
            None => break,
        }
    }
    let mut last_end = 0;
    for &boundary in &boundaries {
        let sentence = text[last_end..boundary].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_string());
        }
        last_end = boundary;
    }
    if last_end < text.len() {
        let sentence = text[last_end..].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_string());
        }
    }
    sentences
}

fn segment_text_into_units(text: &str) -> Vec<String> {
    let mut units = Vec::new();
    let mut current_table = String::new();
    let mut in_table = false;
    
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        
        let is_table_line = trimmed.starts_with("### VLM Extracted:") || 
                            trimmed.starts_with("**Tableau Extrait") ||
                            trimmed.starts_with("|");
        
        if is_table_line {
            in_table = true;
            if !current_table.is_empty() {
                current_table.push('\n');
            }
            current_table.push_str(line);
            i += 1;
        } else {
            if in_table {
                if !current_table.is_empty() {
                    units.push(std::mem::take(&mut current_table));
                }
                in_table = false;
            }
            
            let mut normal_block = String::new();
            while i < lines.len() {
                let next_line = lines[i];
                let next_trimmed = next_line.trim();
                let next_is_table = next_trimmed.starts_with("### VLM Extracted:") || 
                                    next_trimmed.starts_with("**Tableau Extrait") ||
                                    next_trimmed.starts_with("|");
                if next_is_table {
                    break;
                }
                if !normal_block.is_empty() {
                    normal_block.push('\n');
                }
                normal_block.push_str(next_line);
                i += 1;
            }
            
            if !normal_block.is_empty() {
                let sentences = split_into_sentences(&normal_block);
                units.extend(sentences);
            }
        }
    }
    
    if in_table && !current_table.is_empty() {
        units.push(current_table);
    }
    
    units
}

fn chunk_normal_sentences_with_embeddings(
    sentences: &[String],
    max_chunk_size: usize,
    pool: &EmbeddingSessionPool,
    tokenizer: &WordPieceTokenizer,
    db: Option<&sled::Db>,
) -> Vec<String> {
    if sentences.is_empty() { return Vec::new(); }
    
    let mut embeddings = vec![vec![0.0f32; 384]; sentences.len()];
    let mut uncached_indices = Vec::new();
    
    for (i, s) in sentences.iter().enumerate() {
        let mut cached = false;
        if let Some(ref store) = db {
            let hash_val = xxh3_64(s.as_bytes());
            let key = format!("se:{}", hash_val);
            if let Some((cached_sent, cached_emb)) = sled_get_embedding(store, key.as_bytes()) {
                if cached_sent == *s { embeddings[i] = cached_emb; cached = true; }
            }
        }
        if !cached { uncached_indices.push(i); }
    }
    
    let batch_size = 16;
    for chunk_indices in uncached_indices.chunks(batch_size) {
        let mut batch_input_ids = Vec::new();
        let mut max_len = 0;
        
        for &idx in chunk_indices {
            let ids = tokenizer.tokenize(&sentences[idx]);
            max_len = max_len.max(ids.len());
            batch_input_ids.push(ids);
        }
        
        if max_len < 3 { continue; }
        
        let batch_len = chunk_indices.len();
        let mut flat_input_ids = vec![0i64; batch_len * max_len];
        let mut flat_attention_mask = vec![0i64; batch_len * max_len];
        let flat_token_type_ids = vec![0i64; batch_len * max_len];
        
        for (b, ids) in batch_input_ids.iter().enumerate() {
            for (seq, &id) in ids.iter().enumerate() {
                flat_input_ids[b * max_len + seq] = id as i64;
                flat_attention_mask[b * max_len + seq] = 1i64;
            }
        }
        
        let input_ids_tensor = ort::value::Tensor::from_array((vec![batch_len, max_len], flat_input_ids)).unwrap();
        let attention_mask_tensor = ort::value::Tensor::from_array((vec![batch_len, max_len], flat_attention_mask)).unwrap();
        let token_type_ids_tensor = ort::value::Tensor::from_array((vec![batch_len, max_len], flat_token_type_ids)).unwrap();
        
        let inputs = ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor,
        ];
        
        pool.with_session(|session| {
            let outputs_res = session.run(inputs);
            if let Ok(outputs) = outputs_res {
                if let Ok((_, data)) = outputs["last_hidden_state"].try_extract_tensor::<f32>() {
                    for (b, &idx) in chunk_indices.iter().enumerate() {
                        let seq_len = batch_input_ids[b].len();
                        let mut emb = vec![0.0f32; 384];
                        for seq_idx in 0..seq_len {
                            for dim_idx in 0..384 {
                                emb[dim_idx] += data[b * max_len * 384 + seq_idx * 384 + dim_idx];
                            }
                        }
                        for dim_idx in 0..384 { emb[dim_idx] /= seq_len as f32; }
                        
                        embeddings[idx] = emb.clone();
                        
                        if let Some(ref store) = db {
                            let hash_val = xxh3_64(sentences[idx].as_bytes());
                            let key = format!("se:{}", hash_val);
                            sled_put_embedding(store, key.as_bytes(), &(sentences[idx].clone(), emb));
                        }
                    }
                }
            }
        });
    }
    
    let mut similarities = Vec::with_capacity(sentences.len().saturating_sub(1));
    for i in 0..sentences.len().saturating_sub(1) { similarities.push(cosine_similarity(&embeddings[i], &embeddings[i+1])); }
    
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    
    for i in 0..sentences.len() {
        let sentence = &sentences[i];
        let mut split = false;
        if i > 0 && similarities[i-1] < 0.65 { split = true; }
        if !current_chunk.is_empty() && (current_chunk.len() + sentence.len() + 1 > max_chunk_size) { split = true; }
        if split && !current_chunk.is_empty() { chunks.push(std::mem::take(&mut current_chunk)); }
        if !current_chunk.is_empty() { current_chunk.push(' '); }
        current_chunk.push_str(sentence);
    }
    if !current_chunk.is_empty() { chunks.push(current_chunk); }
    chunks
}

fn semantic_chunking_with_embeddings(
    text: &str,
    max_chunk_size: usize,
    pool: &EmbeddingSessionPool,
    tokenizer: &WordPieceTokenizer,
    db: Option<&sled::Db>,
) -> Vec<String> {
    let units = segment_text_into_units(text);
    let mut chunks = Vec::new();
    let mut normal_group = Vec::new();
    
    for unit in units {
        let trimmed = unit.trim();
        let is_table = trimmed.starts_with("### VLM Extracted:") || 
                       trimmed.starts_with("**Tableau Extrait") ||
                       trimmed.starts_with("|");
        if is_table {
            if !normal_group.is_empty() {
                let sub_chunks = chunk_normal_sentences_with_embeddings(&normal_group, max_chunk_size, pool, tokenizer, db);
                chunks.extend(sub_chunks);
                normal_group.clear();
            }
            chunks.push(unit);
        } else {
            normal_group.push(unit);
        }
    }
    
    if !normal_group.is_empty() {
        let sub_chunks = chunk_normal_sentences_with_embeddings(&normal_group, max_chunk_size, pool, tokenizer, db);
        chunks.extend(sub_chunks);
    }
    
    chunks
}

fn semantic_chunking(text: &str, max_chunk_size: usize) -> Vec<String> {
    recursive_structural_chunk(text, max_chunk_size)
}

// ─── TXT Extractor ───────────────────────────────────────────

fn extract_text_from_txt(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("Read error '{}': {}", path.display(), e))
}

/// Non-PDF / non-image pages for ingest + ingest_text.
fn extract_non_pdf_pages(path: &Path, ext: &str) -> (Vec<office::OfficePage>, Vec<String>) {
    let mut warnings = Vec::new();
    let pages = match ext {
        "txt" | "md" => vec![office::OfficePage {
            label: None,
            text: extract_text_from_txt(path).unwrap_or_default(),
        }],
        "docx" | "xlsx" | "pptx" | "xls" | "odt" | "ods" | "odp" | "epub" | "doc" | "ppt" => {
            office::extract_office_pages(path, ext).unwrap_or_default()
        }
        "html" | "htm" => match formats::extract_html(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "csv" => match formats::extract_csv(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "json" => match formats::extract_json(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "xml" => match extra_formats::extract_xml_as_text(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "ndjson" => match extra_formats::extract_ndjson(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "rtf" => match extra_formats::extract_rtf(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "eml" => match extra_formats::extract_eml(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "mbox" => match extra_formats::extract_mbox(path) {
            Ok(t) => vec![office::OfficePage { label: None, text: t }],
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        "msg" => match crate::msg::extract_msg_pages(path) {
            Ok(p) => p,
            Err(e) => {
                warnings.push(e);
                Vec::new()
            }
        },
        _ if formats::is_plain_text_extension(ext) => vec![office::OfficePage {
            label: None,
            text: extra_formats::extract_plain(path).unwrap_or_default(),
        }],
        _ => Vec::new(),
    };
    if (ext == "doc" || ext == "ppt") && !pages.is_empty() {
        warnings.push("legacy_office_deferred".to_string());
    }
    (pages, warnings)
}




struct RawPage {
    index: usize,
    chars: Vec<CharInfo>,
    bulk_text: Option<String>,
    image_bytes: Option<Arc<[u8]>>,
    width: u32,
    height: u32,
    /// Libère un slot du pool d'images quand la page est traitée (backpressure Opt-2).
    _image_permit: Option<ImagePermit>,
}

fn run_ocr_on_bytes(
    ocr: &OcrEngine,
    img_bytes: &[u8],
    width: u32,
    height: u32,
    ocr_mode: ocr_enhance::OcrMode,
) -> Option<String> {
    let (text, _) = run_ocr_on_bytes_with_meta(ocr, img_bytes, width, height, ocr_mode);
    text
}

fn ocr_once(ocr: &OcrEngine, bytes: &mut [u8], width: u32, height: u32, mode: ocr_enhance::OcrMode) -> Option<String> {
    ocr_enhance::preprocess_rgb_for_ocr(bytes, mode);
    let src = ImageSource::from_bytes(bytes, (width, height)).ok()?;
    let input = ocr.prepare_input(src).ok()?;
    ocr.get_text(&input).ok()
}

fn run_ocr_on_bytes_with_meta(
    ocr: &OcrEngine,
    img_bytes: &[u8],
    width: u32,
    height: u32,
    ocr_mode: ocr_enhance::OcrMode,
) -> (Option<String>, Vec<String>) {
    OCR_RGB_BUF.with(|cell| {
        let mut bytes = cell.borrow_mut();
        bytes.clear();
        bytes.extend_from_slice(img_bytes);
        if ocr_mode == ocr_enhance::OcrMode::Vision {
            let mut candidates: Vec<(f32, String)> = Vec::new();
            if let Some(t) = ocr_once(ocr, &mut bytes, width, height, ocr_enhance::OcrMode::Quality) {
                candidates.push((1.0, t));
            }
            bytes.clear();
            bytes.extend_from_slice(img_bytes);
            let w2 = ((width as f32) * 1.5) as u32;
            let h2 = ((height as f32) * 1.5) as u32;
            if let Some(img) = image::RgbImage::from_raw(width, height, bytes.clone()) {
                let up = image::imageops::resize(
                    &img,
                    w2,
                    h2,
                    image::imageops::FilterType::Triangle,
                );
                let up_raw = up.into_raw();
                bytes.clear();
                bytes.extend_from_slice(&up_raw);
                if let Some(t) = ocr_once(ocr, &mut bytes, w2, h2, ocr_enhance::OcrMode::Quality) {
                    candidates.push((1.5, t));
                }
            }
            let (best, scales) = ocr_enhance::pick_best_ocr_text(&candidates);
            return (
                if best.is_empty() { None } else { Some(best) },
                scales,
            );
        }
        let text = ocr_once(ocr, &mut bytes, width, height, ocr_mode);
        let scales = if ocr_mode.uses_multiscale() {
            vec!["1.0".to_string()]
        } else {
            Vec::new()
        };
        (text, scales)
    })
}

#[allow(clippy::too_many_arguments)]
fn process_raw_page(
    raw_page: RawPage,
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    path_str: &str,
    metadata_arc: &Arc<HashMap<String, String>>,
    form_fields_arc: &Arc<HashMap<String, String>>,
    use_ocr: bool,
    use_vlm: bool,
    use_embeddings: bool,
    fast_chunk: bool,
    chunk_size: usize,
    child_chunk_size: Option<usize>,
    db_arc: &Arc<Option<sled::Db>>,
    ocr_arc: &Arc<Option<OcrEngine>>,
    vlm_arc: &Arc<Option<NanoLayoutEngine>>,
    emb_pool_arc: &Option<Arc<EmbeddingSessionPool>>,
    emb_tok_arc: &Option<Arc<WordPieceTokenizer>>,
    precomputed_chunks: Option<Vec<String>>,
    chunk_pages: bool,
    ocr_mode: ocr_enhance::OcrMode,
    strategy_ocr: bool,
    pii_mode: pii::PiiMode,
) {
    let mut cached_doc = None;
    let mut hash = 0u64;
    if let Some(ref db) = **db_arc {
        hash = page_cache_hash_raw(&raw_page);
        let key = format!("page_{}_{}", path_str, hash);
        cached_doc = sled_get_page(db, key.as_bytes());
    }

    if let Some(doc) = cached_doc {
        let _ = tx.send(Ok(doc));
        return;
    }

    let mut page_text = page_text_from_raw(&raw_page);
    let mut ocr_used = false;
    let mut warnings = Vec::new();
    let native_len = page_text.len();
    let layout_complex = is_complex_layout(&raw_page.chars);
    let density = page_quality::text_density(native_len, raw_page.width, raw_page.height);
    let garbled = page_quality::looks_garbled(&page_text);
    let hybrid = ocr_enhance::needs_hybrid_ocr(
        native_len,
        density,
        layout_complex,
        ocr_mode,
        strategy_ocr,
    );
    let broken = ocr_enhance::needs_broken_page_recovery(native_len, density, garbled, ocr_mode);
    let run_ocr = use_ocr && (page_text.trim().is_empty() || hybrid || broken);

    if run_ocr {
        if let Some(ref img_bytes) = raw_page.image_bytes {
            if let Some(ref ocr) = **ocr_arc {
                let ocr_mode_run = if broken && ocr_mode == ocr_enhance::OcrMode::Fast {
                    ocr_enhance::OcrMode::Quality
                } else {
                    ocr_mode
                };
                let (txt_opt, scales) = run_ocr_on_bytes_with_meta(
                    ocr,
                    img_bytes,
                    raw_page.width,
                    raw_page.height,
                    ocr_mode_run,
                );
                if let Some(txt) = txt_opt {
                    if !txt.trim().is_empty() {
                        if hybrid && !page_text.trim().is_empty() {
                            page_text.push_str("\n\n### OCR supplement:\n");
                            page_text.push_str(&txt);
                            warnings.push("hybrid_ocr_merged".to_string());
                        } else {
                            page_text = txt;
                        }
                        ocr_used = true;
                        if broken {
                            warnings.push("page_recovered".to_string());
                        }
                        if !scales.is_empty() {
                            warnings.push(format!("ocr_scales_tried:{}", scales.join(",")));
                        }
                    }
                }
            } else {
                warnings.push("ocr_requested_but_engine_unavailable".to_string());
            }
        } else if use_ocr {
            warnings.push("ocr_skipped_no_image".to_string());
        }
    }
    if garbled && !ocr_used {
        warnings.push("garbled_native_text".to_string());
    }

    if use_vlm && is_complex_layout(&raw_page.chars) {
        if let Some(ref img_bytes) = raw_page.image_bytes {
            if let Some(ref vlm) = **vlm_arc {
                if let Ok(tbl) = vlm.extract_table(
                    img_bytes,
                    raw_page.width,
                    raw_page.height,
                    &raw_page.chars,
                ) {
                    page_text.push_str("\n\n### VLM Extracted:\n");
                    page_text.push_str(&tbl);
                    page_text.push_str("\n\n");
                }
            }
        }
    }

    let chunks = if let Some(c) = precomputed_chunks {
        c
    } else if !chunk_pages {
        vec![page_text.clone()]
    } else {
        chunk_page_text(
            &page_text,
            chunk_size,
            use_embeddings,
            fast_chunk,
            emb_pool_arc.as_ref(),
            emb_tok_arc.as_ref(),
            db_arc.as_ref().as_ref(),
        )
    };

    let mut child_chunks = Vec::new();
    if let Some(child_size) = child_chunk_size {
        for (parent_idx, parent_text) in chunks.iter().enumerate() {
            let sub_chunks = if fast_chunk {
                fast_semantic_chunk(parent_text, child_size)
            } else {
                semantic_chunking(parent_text, child_size)
            };
            for sub_text in sub_chunks {
                child_chunks.push(ChildChunk {
                    text: sub_text,
                    parent_index: parent_idx,
                });
            }
        }
    }

    let doc = finish_document_page(
        path_str.to_string(),
        raw_page.index,
        page_text,
        chunks.clone(),
        child_chunks,
        metadata_arc.as_ref().clone(),
        form_fields_arc.as_ref().clone(),
        None,
        ocr_used,
        raw_page.width,
        raw_page.height,
        warnings,
        pii_mode,
    );

    if let Some(ref db) = **db_arc {
        let key = format!("page_{}_{}", path_str, hash);
        sled_put_page(db, key.as_bytes(), &doc);
    }
    let _ = tx.send(Ok(doc));
}

/// Chemin ultra-rapide : bulk text PDFium, pas de métadonnées ni RawPage intermédiaire.
#[allow(clippy::too_many_arguments)]
fn process_pdf_ultrafast(
    pdfium: &Pdfium,
    path: &Path,
    path_str: &str,
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    chunk_pages: bool,
    chunk_size: usize,
    max_pages: Option<usize>,
    parallel_pages: bool,
    repair_pdf: bool,
    repair_level: pdf_repair::RepairLevel,
    pdf_password: Option<&str>,
    pii_mode: pii::PiiMode,
) {
    let (document, file_meta, temp_path) = if repair_pdf {
        match pdf_repair::load_pdf(pdfium, path, true, repair_level, pdf_password) {
            Ok((doc, plan)) => (doc, plan.metadata, plan.temp_path),
            Err(plan) => {
                if let Some(temp) = plan.temp_path {
                    let _ = std::fs::remove_file(temp);
                }
                send_error_page(
                    tx,
                    path_str,
                    0,
                    plan.open_error
                        .unwrap_or_else(|| "pdfium failed to open PDF".to_string()),
                    plan.metadata,
                );
                return;
            }
        }
    } else if let Some(doc) = pdf_repair::try_open_pdf(pdfium, path, pdf_password) {
        (doc, HashMap::new(), None)
    } else {
        send_error_page(
            tx,
            path_str,
            0,
            "pdfium failed to open PDF".to_string(),
            HashMap::new(),
        );
        return;
    };
    let path_arc: Arc<str> = path_str.into();

    let num_pages = document.pages().len() as usize;
    let limit = if let Some(mp) = max_pages {
        num_pages.min(mp)
    } else {
        num_pages
    };

    let mut pages_data = Vec::with_capacity(limit);
    for i in 0..limit {
        if let Ok(page) = document.pages().get(i as u16) {
            let page_text = page.text().map(|t| t.all()).unwrap_or_default();
            pages_data.push((i, page_text));
        }
    }
    drop(document);
    if let Some(temp) = temp_path {
        let _ = std::fs::remove_file(temp);
    }

    let meta_page0 = if file_meta.is_empty() {
        None
    } else {
        Some(&file_meta)
    };
    let path_arc = path_arc.clone();

    if parallel_pages && chunk_pages {
        let meta_arc = Arc::new(file_meta);
        pages_data.into_par_iter().for_each_with(tx.clone(), |tx, (i, page_text)| {
            let chunks = fast_semantic_chunk(&page_text, chunk_size);
            let meta = if i == 0 {
                Some(meta_arc.as_ref())
            } else {
                None
            };
            let doc = finish_document_page_bulk(
                path_arc.clone(),
                i,
                page_text,
                chunks,
                meta,
                pii_mode,
            );
            let _ = tx.send(Ok(doc));
        });
    } else {
        for (i, page_text) in pages_data {
            let (content, chunks) = if chunk_pages {
                let chunks = fast_semantic_chunk(&page_text, chunk_size);
                (page_text, chunks)
            } else {
                (page_text, Vec::new())
            };
            let meta = if i == 0 { meta_page0 } else { None };
            let doc = finish_document_page_bulk(path_arc.clone(), i, content, chunks, meta, pii_mode);
            let _ = tx.send(Ok(doc));
        }
    }
}

/// Extract plain page text from a PDF (single pdfium open). Used by `ingest_text`.
fn extract_pdf_page_texts(
    pdfium: &Pdfium,
    path: &Path,
    max_pages: Option<usize>,
    repair_pdf: bool,
    repair_level: pdf_repair::RepairLevel,
    pdf_password: Option<&str>,
) -> Result<Vec<String>, String> {
    let (document, temp_path) = if repair_pdf {
        let (document, plan) =
            pdf_repair::load_pdf(pdfium, path, true, repair_level, pdf_password).map_err(|plan| {
                if let Some(temp) = plan.temp_path {
                    let _ = std::fs::remove_file(temp);
                }
                plan.open_error
                    .unwrap_or_else(|| "pdfium failed to open PDF".to_string())
            })?;
        (document, plan.temp_path)
    } else {
        let document = pdf_repair::try_open_pdf(pdfium, path, pdf_password)
            .ok_or_else(|| "pdfium failed to open PDF".to_string())?;
        (document, None)
    };
    let num_pages = document.pages().len() as usize;
    let limit = max_pages.map_or(num_pages, |mp| num_pages.min(mp));
    let mut texts = Vec::with_capacity(limit);
    for i in 0..limit {
        if let Ok(page) = document.pages().get(i as u16) {
            texts.push(page.text().map(|t| t.all()).unwrap_or_default());
        }
    }
    drop(document);
    if let Some(temp) = temp_path {
        let _ = std::fs::remove_file(temp);
    }
    Ok(texts)
}

#[cfg(feature = "mupdf")]
#[allow(clippy::too_many_arguments)]
fn process_pdf_mupdf(
    path: &Path,
    path_str: &str,
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    chunk_pages: bool,
    chunk_size: usize,
    max_pages: Option<usize>,
) -> Result<(), String> {
    use mupdf::{Document, TextPageFlags};
    let doc = Document::open(path).map_err(|e| format!("MuPDF open: {}", e))?;
    let empty_meta = HashMap::new();
    for (i, page) in doc.pages().enumerate() {
        if let Some(mp) = max_pages {
            if i >= mp {
                break;
            }
        }
        let page = page.map_err(|e| format!("MuPDF page {}: {}", i, e))?;
        let text_page = page
            .to_text_page(TextPageFlags::empty())
            .map_err(|e| format!("MuPDF text {}: {}", i, e))?;
        let page_text = text_page
            .to_text()
            .map_err(|e| format!("MuPDF to_text {}: {}", i, e))?;
        let (content, chunks) = if chunk_pages {
            let chunks = fast_semantic_chunk(&page_text, chunk_size);
            (page_text, chunks)
        } else {
            (page_text.clone(), vec![page_text])
        };
        let doc_page = finish_document_page(
            path_str.to_string(),
            i,
            content,
            chunks,
            Vec::new(),
            empty_meta.clone(),
            HashMap::new(),
            None,
            false,
            0,
            0,
            Vec::new(),
            pii::PiiMode::Off,
        );
        let _ = tx.send(Ok(doc_page));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_pdf_document(
    pdfium: &Pdfium,
    path: &Path,
    path_str: &str,
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    backend: PdfBackend,
    layout: LayoutMode,
    use_ocr: bool,
    use_vlm: bool,
    use_embeddings: bool,
    fast_chunk: bool,
    doc_chunking: bool,
    chunk_pages: bool,
    chunk_size: usize,
    child_chunk_size: Option<usize>,
    max_pages: Option<usize>,
    max_concurrent_images: usize,
    db_arc: &Arc<Option<sled::Db>>,
    ocr_arc: &Arc<Option<OcrEngine>>,
    vlm_arc: &Arc<Option<NanoLayoutEngine>>,
    emb_pool_arc: &Option<Arc<EmbeddingSessionPool>>,
    emb_tok_arc: &Option<Arc<WordPieceTokenizer>>,
    parallel_pages: bool,
    repair_pdf: bool,
    repair_level: pdf_repair::RepairLevel,
    pdf_password: Option<&str>,
    ocr_mode: ocr_enhance::OcrMode,
    ocr_render_scale: f32,
    strategy_ocr: bool,
    pii_mode: pii::PiiMode,
) {
    let heavy_pipeline =
        use_ocr || use_vlm || use_embeddings || child_chunk_size.is_some() || db_arc.is_some();
    let simple_text = !heavy_pipeline && layout != LayoutMode::MultiColumn;
    #[cfg(feature = "mupdf")]
    {
        let resolved = backend.resolve(simple_text);
        if resolved == PdfBackend::Mupdf && simple_text {
            if process_pdf_mupdf(path, path_str, tx, chunk_pages, chunk_size, max_pages).is_ok() {
                return;
            }
        }
    }

    let _resolved = backend.resolve(simple_text);

    if simple_text {
        process_pdf_ultrafast(
            pdfium,
            path,
            path_str,
            tx,
            chunk_pages,
            chunk_size,
            max_pages,
            parallel_pages,
            repair_pdf,
            repair_level,
            pdf_password,
            pii_mode,
        );
        return;
    }

    let (document, plan) =
        match pdf_repair::load_pdf(pdfium, path, repair_pdf, repair_level, pdf_password) {
        Ok(pair) => pair,
        Err(plan) => {
            let pdf_repair::PdfOpenPlan {
                metadata,
                temp_path,
                open_error,
                ..
            } = plan;
            if let Some(temp) = temp_path {
                let _ = std::fs::remove_file(temp);
            }
            if metadata.get("pdf_encrypted").is_some() {
                send_error_page(
                    tx,
                    path_str,
                    0,
                    open_error.unwrap_or_else(|| "encrypted PDF".to_string()),
                    metadata,
                );
                return;
            }
            send_error_page(
                tx,
                path_str,
                0,
                open_error.unwrap_or_else(|| "pdfium failed to open PDF".to_string()),
                metadata,
            );
            return;
        }
    };
    let pdf_repair::PdfOpenPlan {
        open_path,
        temp_path: repair_temp,
        mut metadata,
        ..
    } = plan;
    for tag in document.metadata().iter() {
        metadata.insert(format!("{:?}", tag.tag_type()), tag.value().to_string());
    }
    let mut form_fields = HashMap::new();
    if let Some(form) = document.form() {
        for (key, value) in form.field_values(&document.pages()).iter() {
            form_fields.insert(key.clone(), value.as_deref().unwrap_or("").to_string());
        }
    }
    let metadata_arc = Arc::new(metadata);
    let form_fields_arc = Arc::new(form_fields);

    let heavy_pipeline = use_ocr || use_vlm || use_embeddings || child_chunk_size.is_some();
    let use_doc_chunking = chunk_pages
        && doc_chunking
        && !heavy_pipeline
        && fast_chunk
        && layout != LayoutMode::MultiColumn;

    let mut raw_pages: Vec<RawPage> = Vec::new();
    let image_pool = ImagePermitPool::new(max_concurrent_images);

    if parallel_pages {
        let num_pages = document.pages().len() as usize;
        let limit = if let Some(mp) = max_pages {
            num_pages.min(mp)
        } else {
            num_pages
        };
        let lib_path = resolve_pdfium_path();
        let render_w = ocr_enhance::render_scale_to_width(1500, ocr_render_scale);
        let open_path_par = open_path.clone();
        
        raw_pages = (0..limit).into_par_iter().map(|i| {
            let mut raw_page = RawPage {
                index: i,
                chars: Vec::new(),
                bulk_text: None,
                image_bytes: None,
                width: 0,
                height: 0,
                _image_permit: None,
            };
            with_thread_pdfium(&lib_path, |pdfium| {
                if let Ok(doc) = pdfium.load_pdf_from_file(&open_path_par, pdf_password) {
                    if let Ok(page) = doc.pages().get(i as u16) {
                        let (bulk_text, chars) = extract_page_raw(&page, layout, use_vlm, use_ocr);
                        let mut image_bytes = None;
                        let mut width = 0;
                        let mut height = 0;
                        let mut image_permit = None;
                        
                        let page_empty = bulk_text.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
                            && chars.is_empty();
                        let native_len = bulk_text.as_ref().map(|s| s.len()).unwrap_or(0) + chars.len();
                        let layout_complex = is_complex_layout(&chars);
                        let density = page_quality::text_density(native_len, 1500, 1500);
                        let garbled = bulk_text
                            .as_ref()
                            .map(|t| page_quality::looks_garbled(t))
                            .unwrap_or(false);
                        let hybrid = ocr_enhance::needs_hybrid_ocr(
                            native_len,
                            density,
                            layout_complex,
                            ocr_mode,
                            strategy_ocr,
                        );
                        let broken = ocr_enhance::needs_broken_page_recovery(
                            native_len, density, garbled, ocr_mode,
                        );
                        let needs_image =
                            (use_ocr && (page_empty || hybrid || broken)) || use_vlm;
                        if needs_image {
                            if let Some(permit) = image_pool.acquire() {
                                let rc = PdfRenderConfig::new()
                                    .set_target_width(render_w)
                                    .set_maximum_height(render_w);
                                if let Ok(img) = page.render_with_config(&rc) {
                                    let rgb = img.as_image().into_rgb8();
                                    width = rgb.width();
                                    height = rgb.height();
                                    let raw = rgb.into_raw();
                                    image_bytes = Some(Arc::from(raw.into_boxed_slice()));
                                    image_permit = Some(permit);
                                }
                            }
                        }
                        
                        raw_page.bulk_text = bulk_text;
                        raw_page.chars = chars;
                        raw_page.image_bytes = image_bytes;
                        raw_page.width = width;
                        raw_page.height = height;
                        raw_page._image_permit = image_permit;
                    }
                }
            });
            raw_page
        }).collect();
    } else {
        for (i, page) in document.pages().iter().enumerate() {
            if let Some(mp) = max_pages {
                if i >= mp {
                    break;
                }
            }
            let (bulk_text, chars) = extract_page_raw(&page, layout, use_vlm, use_ocr);

            let mut image_bytes = None;
            let mut width = 0u32;
            let mut height = 0u32;
            let mut image_permit = None;
            let page_empty = bulk_text.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
                && chars.is_empty();
            let native_len = bulk_text.as_ref().map(|s| s.len()).unwrap_or(0) + chars.len();
            let layout_complex = is_complex_layout(&chars);
            let density = page_quality::text_density(native_len, 1500, 1500);
            let garbled = bulk_text
                .as_ref()
                .map(|t| page_quality::looks_garbled(t))
                .unwrap_or(false);
            let hybrid = ocr_enhance::needs_hybrid_ocr(
                native_len,
                density,
                layout_complex,
                ocr_mode,
                strategy_ocr,
            );
            let broken =
                ocr_enhance::needs_broken_page_recovery(native_len, density, garbled, ocr_mode);
            let needs_image = (use_ocr && (page_empty || hybrid || broken)) || use_vlm;
            if needs_image {
                if let Some(permit) = image_pool.acquire() {
                    let render_w = ocr_enhance::render_scale_to_width(1500, ocr_render_scale);
                    let rc = PdfRenderConfig::new()
                        .set_target_width(render_w)
                        .set_maximum_height(render_w);
                    if let Ok(img) = page.render_with_config(&rc) {
                        let rgb = img.as_image().into_rgb8();
                        width = rgb.width();
                        height = rgb.height();
                        let raw = rgb.into_raw();
                        image_bytes = Some(Arc::from(raw.into_boxed_slice()));
                        image_permit = Some(permit);
                    }
                }
            }

            raw_pages.push(RawPage {
                index: i,
                chars,
                bulk_text,
                image_bytes,
                width,
                height,
                _image_permit: image_permit,
            });
        }
    }
    drop(document);
    if let Some(temp) = repair_temp {
        let _ = std::fs::remove_file(temp);
    }

    if use_doc_chunking && !raw_pages.is_empty() {
        let mut page_texts: Vec<(usize, String)> = Vec::with_capacity(raw_pages.len());
        for raw in &raw_pages {
            page_texts.push((raw.index, page_text_from_raw(raw)));
        }
        let full = page_texts
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let doc_chunks = fast_semantic_chunk(&full, chunk_size);
        let assigned = assign_doc_chunks_to_pages(&page_texts, &doc_chunks);
        let chunk_map: HashMap<usize, Vec<String>> = assigned.into_iter().collect();
        let run = |tx: &mpsc::Sender<PyResult<DocumentPage>>, raw_page: RawPage| {
            let pre = chunk_map.get(&raw_page.index).cloned();
            process_raw_page(
                raw_page,
                tx,
                path_str,
                &metadata_arc,
                &form_fields_arc,
                use_ocr,
                use_vlm,
                use_embeddings,
                fast_chunk,
                chunk_size,
                child_chunk_size,
                db_arc,
                ocr_arc,
                vlm_arc,
                emb_pool_arc,
                emb_tok_arc,
                pre,
                chunk_pages,
                ocr_mode,
                strategy_ocr,
                pii_mode,
            );
        };
        if parallel_pages {
            raw_pages.into_par_iter().for_each_with(tx.clone(), |tx, raw_page| {
                run(tx, raw_page);
            });
        } else {
            for raw_page in raw_pages {
                run(&tx, raw_page);
            }
        }
        return;
    }

    if heavy_pipeline {
        let (page_tx, page_rx) = crossbeam_channel::bounded::<RawPage>(32);
        let tx_c = tx.clone();
        let db_arc = db_arc.clone();
        let ocr_arc = ocr_arc.clone();
        let vlm_arc = vlm_arc.clone();
        let emb_pool_arc = emb_pool_arc.clone();
        let emb_tok_arc = emb_tok_arc.clone();
        let path_str = path_str.to_string();
        let metadata_arc = metadata_arc.clone();
        let form_fields_arc = form_fields_arc.clone();
        let ocr_mode_c = ocr_mode;
        let strategy_ocr_c = strategy_ocr;
        let pii_mode_c = pii_mode;
        let consumer = thread::spawn(move || {
            page_rx.into_iter().par_bridge().for_each_with(tx_c, |tx, raw_page| {
                process_raw_page(
                    raw_page,
                    tx,
                    &path_str,
                    &metadata_arc,
                    &form_fields_arc,
                    use_ocr,
                    use_vlm,
                    use_embeddings,
                    fast_chunk,
                    chunk_size,
                    child_chunk_size,
                    &db_arc,
                    &ocr_arc,
                    &vlm_arc,
                    &emb_pool_arc,
                    &emb_tok_arc,
                    None,
                    chunk_pages,
                    ocr_mode_c,
                    strategy_ocr_c,
                    pii_mode_c,
                );
            });
        });
        for raw in raw_pages {
            let _ = page_tx.send(raw);
        }
        drop(page_tx);
        let _ = consumer.join();
    } else {
        raw_pages.into_par_iter().for_each_with(tx.clone(), |tx, raw_page| {
            process_raw_page(
                raw_page,
                tx,
                path_str,
                &metadata_arc,
                &form_fields_arc,
                use_ocr,
                use_vlm,
                use_embeddings,
                fast_chunk,
                chunk_size,
                child_chunk_size,
                db_arc,
                ocr_arc,
                vlm_arc,
                emb_pool_arc,
                emb_tok_arc,
                None,
                chunk_pages,
                ocr_mode,
                strategy_ocr,
                pii_mode,
            );
        });
    }
}

fn process_image_file(
    path: &Path,
    path_str: &str,
    tx: &mpsc::Sender<PyResult<DocumentPage>>,
    use_ocr: bool,
    ocr_arc: &Arc<Option<OcrEngine>>,
    ocr_mode: ocr_enhance::OcrMode,
    chunk_size: usize,
    fast_chunk: bool,
    chunk_pages: bool,
    pii_mode: pii::PiiMode,
) {
    let mut metadata = HashMap::new();
    metadata.insert("source_type".to_string(), "image".to_string());
    let img = match formats::load_image_for_ocr(path) {
        Ok(d) => d,
        Err(e) => {
            send_error_page(tx, path_str, 0, e, metadata);
            return;
        }
    };
    let mut content = String::new();
    let mut ocr_used = false;
    if use_ocr {
        if let Some(ref ocr) = **ocr_arc {
            if let Some(txt) = run_ocr_on_bytes(ocr, &img.rgb_bytes, img.width, img.height, ocr_mode) {
                content = txt;
                ocr_used = true;
            }
        }
    }
    let chunks = if chunk_pages {
        if fast_chunk {
            fast_semantic_chunk(&content, chunk_size)
        } else {
            semantic_chunking(&content, chunk_size)
        }
    } else {
        vec![content.clone()]
    };
    let doc = finish_document_page(
        path_str.to_string(),
        0,
        content,
        chunks,
        Vec::new(),
        metadata,
        HashMap::new(),
        None,
        ocr_used,
        img.width,
        img.height,
        if !use_ocr {
            vec!["image_without_ocr".to_string()]
        } else {
            Vec::new()
        },
        pii_mode,
    );
    let _ = tx.send(Ok(doc));
}

/// Turbo collector: no channels, no per-page Python __next__ lock — bulk PDF text only.
fn collect_turbo_pages(
    paths: &[PathBuf],
    max_pages: Option<usize>,
    repair_pdf: bool,
    repair_level: pdf_repair::RepairLevel,
    pdf_password: Option<&str>,
    parallel_files: bool,
    pii_mode: pii::PiiMode,
) -> Vec<DocumentPage> {
    let lib_path = resolve_pdfium_path();

    let pdf_pages = |pdfium: &Pdfium, path: &Path| -> Vec<DocumentPage> {
        let path_arc: Arc<str> = path.to_string_lossy().into_owned().into();
        match extract_pdf_page_texts(pdfium, path, max_pages, repair_pdf, repair_level, pdf_password) {
            Ok(texts) => texts
                .into_iter()
                .enumerate()
                .map(|(i, content)| {
                    finish_document_page_bulk(path_arc.clone(), i, content, Vec::new(), None, pii_mode)
                })
                .collect(),
            Err(err) => {
                let mut d = finish_document_page_bulk(
                    path_arc,
                    0,
                    String::new(),
                    Vec::new(),
                    None,
                    pii_mode,
                );
                d.error = Some(err);
                d.warnings = vec!["pdf_open_failed".to_string()];
                d.extraction_confidence = 0.1;
                vec![d]
            }
        }
    };

    let non_pdf_pages = |path: &Path| -> Vec<DocumentPage> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if formats::is_image_extension(&ext) {
            return Vec::new();
        }
        let (pages, _) = extract_non_pdf_pages(path, &ext);
        let path_arc: Arc<str> = path.to_string_lossy().into_owned().into();
        pages
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let mut metadata = HashMap::new();
                metadata.insert("source_type".to_string(), ext.to_string());
                if let Some(label) = p.label {
                    if ext == "xlsx" || ext == "xls" || ext == "ods" {
                        metadata.insert("sheet".to_string(), label);
                    } else if ext == "pptx" || ext == "odp" {
                        metadata.insert("slide".to_string(), label);
                    } else if ext == "epub" {
                        metadata.insert("chapter".to_string(), label);
                    }
                }
                let mut doc = finish_document_page_bulk(path_arc.clone(), i, p.text, Vec::new(), Some(&metadata), pii_mode);
                doc.metadata = metadata;
                doc
            })
            .collect()
    };

    let use_parallel = parallel_files && paths.len() > 1 && !cfg!(target_os = "windows");
    if use_parallel {
        paths
            .par_iter()
            .flat_map(|path| {
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext.eq_ignore_ascii_case("pdf") {
                    with_thread_pdfium(&lib_path, |pdfium| pdf_pages(pdfium, path))
                } else {
                    non_pdf_pages(path)
                }
            })
            .collect()
    } else {
        let mut out = Vec::new();
        with_thread_pdfium(&lib_path, |pdfium| {
            for path in paths {
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext.eq_ignore_ascii_case("pdf") {
                    out.extend(pdf_pages(pdfium, path));
                } else {
                    out.extend(non_pdf_pages(path));
                }
            }
        });
        out
    }
}

#[pyfunction]
#[pyo3(signature = (directory, chunk_size=None, child_chunk_size=None, use_cache=None, use_ocr=None, pattern=None, use_embeddings=None, use_vlm=None, max_pages=None, max_concurrent_images=None, layout_mode=None, parallel_files=None, fast_chunk=None, doc_chunking=None, chunk_pages=None, backend=None, strategy=None, repair_pdf=None, ocr_mode=None, ocr_render_scale=None, repair_level=None, pdf_password=None, ingest_archives=None, pii_mode=None))]
fn ingest(
    py: Python<'_>,
    directory: &str,
    chunk_size: Option<usize>,
    child_chunk_size: Option<usize>,
    use_cache: Option<bool>,
    use_ocr: Option<bool>,
    pattern: Option<&str>,
    use_embeddings: Option<bool>,
    use_vlm: Option<bool>,
    max_pages: Option<usize>,
    max_concurrent_images: Option<usize>,
    layout_mode: Option<&str>,
    parallel_files: Option<bool>,
    fast_chunk: Option<bool>,
    doc_chunking: Option<bool>,
    chunk_pages: Option<bool>,
    backend: Option<&str>,
    strategy: Option<&str>,
    repair_pdf: Option<bool>,
    ocr_mode: Option<&str>,
    ocr_render_scale: Option<f32>,
    repair_level: Option<&str>,
    pdf_password: Option<&str>,
    ingest_archives: Option<bool>,
    pii_mode: Option<&str>,
) -> PyResult<ChunkStream> {
    let chunk_size = chunk_size.unwrap_or(1000);
    let use_cache = use_cache.unwrap_or(true);
    let repair_pdf = repair_pdf.unwrap_or(false);
    let repair_level = match repair_level {
        Some(s) => pdf_repair::RepairLevel::from_str(s).map_err(PyValueError::new_err)?,
        None => pdf_repair::RepairLevel::Normal,
    };
    let ingest_archives = ingest_archives.unwrap_or(false);
    let pdf_password_owned: Option<String> = pdf_password.filter(|p| !p.is_empty()).map(|s| s.to_string());
    let ocr_render_scale = ocr_render_scale.unwrap_or(1.0);
    let ocr_mode = match ocr_mode {
        Some(s) => ocr_enhance::OcrMode::from_str(s).map_err(PyValueError::new_err)?,
        None => ocr_enhance::OcrMode::Fast,
    };
    let pii_mode = match pii_mode {
        Some(s) => pii::PiiMode::from_str(s).map_err(PyValueError::new_err)?,
        None => pii::PiiMode::Off,
    };
    let mut use_ocr = use_ocr.unwrap_or(false);
    let mut use_embeddings = use_embeddings.unwrap_or(false);
    let mut use_vlm = use_vlm.unwrap_or(false);
    let max_concurrent_images =
        max_concurrent_images.unwrap_or_else(|| default_max_concurrent_images(use_vlm));
    let mut layout = match layout_mode {
        Some(s) => LayoutMode::from_str(s).map_err(PyValueError::new_err)?,
        None => LayoutMode::Auto,
    };
    let mut fast_chunk = fast_chunk.unwrap_or(!use_embeddings);
    let doc_chunking = doc_chunking.unwrap_or(false);
    let chunk_pages = chunk_pages.unwrap_or(true);
    let parallel_files = parallel_files.unwrap_or(
        !use_ocr && !use_vlm && !use_embeddings && !cfg!(target_os = "windows"),
    );
    let parallel_pages = !parallel_files;

    let pdf_backend = match backend {
        Some(s) => PdfBackend::from_str(s).map_err(PyValueError::new_err)?,
        None => PdfBackend::Auto,
    };
    let mut ingest_strategy = IngestStrategy::Fast;
    if let Some(s) = strategy {
        ingest_strategy = IngestStrategy::from_str(s).map_err(PyValueError::new_err)?;
        let flags = apply_strategy(
            ingest_strategy,
            StrategyFlags {
                layout,
                use_ocr,
                use_vlm,
                use_embeddings,
                fast_chunk,
            },
        );
        layout = flags.layout;
        use_ocr = flags.use_ocr;
        use_vlm = flags.use_vlm;
        use_embeddings = flags.use_embeddings;
        fast_chunk = flags.fast_chunk;
    }
    let strategy_ocr = ingest_strategy == IngestStrategy::Ocr;

    let filter_regex = if let Some(pat) = pattern {
        Some(regex::Regex::new(pat).map_err(|e| PyValueError::new_err(format!("Invalid regex: {}", e)))?)
    } else { None };

    let (paths, archive_temp_dirs) =
        archives::collect_ingest_paths(directory, filter_regex.as_ref(), ingest_archives);

    let mut auto_needs_ocr = false;
    if ingest_strategy == IngestStrategy::Auto {
        let lib_path = resolve_pdfium_path();
        for path in &paths {
            if path.extension().and_then(|s| s.to_str()) == Some("pdf") {
                with_thread_pdfium(&lib_path, |pdfium| {
                    if let Some(avg) = sample_pdf_avg_chars(pdfium, path, 5) {
                        if page_quality::looks_scanned(avg) {
                            auto_needs_ocr = true;
                        }
                    }
                });
            }
        }
    }

    let (det_model_path, rec_model_path) = if use_ocr || auto_needs_ocr {
        let (d, r) = download_ocr_models_if_needed(py)?;
        (Some(d), Some(r))
    } else {
        (None, None)
    };

    let nano_model_path = if use_vlm {
        Some(download_nano_model_if_needed(py)?)
    } else {
        None
    };

    let (_emb_pool_opt, _emb_tok_opt) = if use_embeddings {
        let (model_path, vocab_path) = download_embedding_model_if_needed(py)?;
        let pool_size = std::cmp::min(4, num_cpus::get().max(1));
        let pool = EmbeddingSessionPool::new(&model_path, pool_size)
            .map_err(|e| PyRuntimeError::new_err(e))?;
        let vocab_str = std::fs::read_to_string(&vocab_path).unwrap();
        let tokenizer = WordPieceTokenizer::new(&vocab_str);
        (Some(Arc::new(pool)), Some(Arc::new(tokenizer)))
    } else {
        (None, None)
    };

    let layout_resolved = layout.resolve_simple();
    let turbo_mode = chunk_pages == false
        && !use_cache
        && !use_ocr
        && !auto_needs_ocr
        && !use_vlm
        && !use_embeddings
        && child_chunk_size.is_none()
        && layout_resolved != LayoutMode::MultiColumn
        && ingest_strategy != IngestStrategy::HiRes
        && ingest_strategy != IngestStrategy::Ocr;

    if turbo_mode {
        let repair_level = repair_level;
        let pdf_password_owned = pdf_password_owned.clone();
        let paths = paths.clone();
        let archive_temp_dirs = archive_temp_dirs.clone();
        let pages = py.allow_threads(move || {
            collect_turbo_pages(
                &paths,
                max_pages,
                repair_pdf,
                repair_level,
                pdf_password_owned.as_deref(),
                parallel_files,
                pii_mode,
            )
        });
        archives::cleanup_temp_dirs(&archive_temp_dirs);
        return Ok(ChunkStream::from_vec(pages));
    }

    let (tx, rx) = mpsc::channel();
    let repair_level = repair_level;
    let pdf_password_owned = pdf_password_owned;
    let pii_mode = pii_mode;
    let small_batch = paths.len() <= 2 && !use_ocr && !use_vlm && !use_embeddings;

    let spawn_ingest = move || {
        let db = if use_cache { sled::open(".brainpipe_cache").ok() } else { None };

        let ocr_engine = if let (Some(dp), Some(rp)) = (&det_model_path, &rec_model_path) {
            if let (Ok(det_m), Ok(rec_m)) = (RtenModel::load_file(dp), RtenModel::load_file(rp)) {
                OcrEngine::new(OcrEngineParams { detection_model: Some(det_m), recognition_model: Some(rec_m), ..Default::default() }).ok()
            } else { None }
        } else { None };

        let vlm_engine = if let Some(np) = &nano_model_path {
            NanoLayoutEngine::new(np).ok()
        } else { None };

        let db_arc = Arc::new(db);
        let ocr_arc = Arc::new(ocr_engine);
        let vlm_arc = Arc::new(vlm_engine);
        let emb_pool_arc = _emb_pool_opt.clone();
        let emb_tok_arc = _emb_tok_opt.clone();
        
        let lib_path = resolve_pdfium_path();
        let pdf_backend = pdf_backend;
        let ocr_mode = ocr_mode;
        let strategy_ocr = strategy_ocr;
        let ingest_strategy = ingest_strategy;
        let repair_level = repair_level;
        let pdf_password = pdf_password_owned.as_deref();

        let process_path = |path: PathBuf| {
            let path_str = path.to_string_lossy().to_string();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();

            if ext == "pdf" {
                let file_use_ocr = if ingest_strategy == IngestStrategy::Auto {
                    let mut scanned = false;
                    with_thread_pdfium(&lib_path, |pdfium| {
                        if let Some(avg) = sample_pdf_avg_chars(pdfium, &path, 5) {
                            scanned = page_quality::looks_scanned(avg);
                        }
                    });
                    scanned
                } else {
                    use_ocr
                };
                with_thread_pdfium(&lib_path, |pdfium| {
                    process_pdf_document(
                        pdfium,
                        &path,
                        &path_str,
                        &tx,
                        pdf_backend,
                        layout,
                        file_use_ocr,
                        use_vlm,
                        use_embeddings,
                        fast_chunk,
                        doc_chunking,
                        chunk_pages,
                        chunk_size,
                        child_chunk_size,
                        max_pages,
                        max_concurrent_images,
                        &db_arc,
                        &ocr_arc,
                        &vlm_arc,
                        &emb_pool_arc,
                        &emb_tok_arc,
                        parallel_pages,
                        repair_pdf,
                        repair_level,
                        pdf_password,
                        ocr_mode,
                        ocr_render_scale,
                        strategy_ocr,
                        pii_mode,
                    );
                });
            } else if formats::is_image_extension(&ext) {
                process_image_file(
                    &path,
                    &path_str,
                    &tx,
                    use_ocr,
                    &ocr_arc,
                    ocr_mode,
                    chunk_size,
                    fast_chunk,
                    chunk_pages,
                    pii_mode,
                );
            } else {
                let (pages, file_warnings) = extract_non_pdf_pages(&path, &ext);
                if pages.is_empty() && !file_warnings.is_empty() {
                    send_error_page(
                        &tx,
                        &path_str,
                        0,
                        file_warnings.join("; "),
                        HashMap::new(),
                    );
                    return;
                }

                for (page_index, page) in pages.into_iter().enumerate() {
                    let content = page.text;
                    let chunks = chunk_page_text(
                        &content,
                        chunk_size,
                        use_embeddings,
                        fast_chunk,
                        emb_pool_arc.as_ref(),
                        emb_tok_arc.as_ref(),
                        db_arc.as_ref().as_ref(),
                    );
                    let mut child_chunks = Vec::new();
                    if let Some(child_size) = child_chunk_size {
                        for (parent_idx, parent_text) in chunks.iter().enumerate() {
                            let sub_chunks = if fast_chunk {
                                fast_semantic_chunk(parent_text, child_size)
                            } else {
                                semantic_chunking(parent_text, child_size)
                            };
                            for sub_text in sub_chunks {
                                child_chunks.push(ChildChunk {
                                    text: sub_text,
                                    parent_index: parent_idx,
                                });
                            }
                        }
                    }

                    let mut metadata = HashMap::new();
                    metadata.insert("source_type".to_string(), ext.to_string());
                    if let Some(label) = page.label {
                        if ext == "xlsx" || ext == "xls" || ext == "ods" {
                            metadata.insert("sheet".to_string(), label);
                        } else if ext == "pptx" || ext == "odp" {
                            metadata.insert("slide".to_string(), label);
                        } else if ext == "epub" {
                            metadata.insert("chapter".to_string(), label);
                        }
                    }

                    let doc = finish_document_page(
                        path_str.clone(),
                        page_index,
                        content,
                        chunks.clone(),
                        child_chunks,
                        metadata,
                        HashMap::new(),
                        None,
                        false,
                        0,
                        0,
                        file_warnings.clone(),
                        pii_mode,
                    );
                    let _ = tx.send(Ok(doc));
                }
            }
        };

        if parallel_files && paths.len() > 1 {
            paths.into_par_iter().for_each(process_path);
        } else {
            for path in paths {
                process_path(path);
            }
        }
        archives::cleanup_temp_dirs(&archive_temp_dirs);
    };

    if small_batch {
        spawn_ingest();
    } else {
        thread::spawn(spawn_ingest);
    }

    Ok(ChunkStream::from_channel(rx))
}

#[pyfunction]
fn build_ingest_report(pages: Vec<DocumentPage>) -> ingest_report::IngestReport {
    ingest_report::build_ingest_report(&pages)
}

/// Standalone PII sanitize (same engine as ingest `pii_mode=redact`).
#[pyfunction]
#[pyo3(signature = (text, mode=None))]
fn sanitize_pii(text: &str, mode: Option<&str>) -> PyResult<(String, usize, Vec<String>)> {
    let pii_mode = match mode {
        Some(s) => pii::PiiMode::from_str(s).map_err(PyValueError::new_err)?,
        None => pii::PiiMode::Redact,
    };
    let (out, count, types) = pii::sanitize_text(text, pii_mode);
    Ok((out, count, types))
}

/// Extraction texte seule — remplacement drop-in PyMuPDF (liste de str par page).
/// Bypasses `ingest`/`DocumentPage` allocation; one pdfium open per PDF.
#[pyfunction]
#[pyo3(signature = (directory, pattern=None, max_pages=None, backend=None, parallel_files=None, repair_pdf=None, pii_mode=None))]
fn ingest_text(
    py: Python<'_>,
    directory: &str,
    pattern: Option<&str>,
    max_pages: Option<usize>,
    backend: Option<&str>,
    parallel_files: Option<bool>,
    repair_pdf: Option<bool>,
    pii_mode: Option<&str>,
) -> PyResult<Vec<String>> {
    let _ = backend;
    let repair_pdf = repair_pdf.unwrap_or(false);
    let pii_mode = match pii_mode {
        Some(s) => pii::PiiMode::from_str(s).map_err(PyValueError::new_err)?,
        None => pii::PiiMode::Off,
    };

    let filter_regex = if let Some(pat) = pattern {
        Some(regex::Regex::new(pat).map_err(|e| PyValueError::new_err(format!("Invalid regex: {}", e)))?)
    } else {
        None
    };

    let (paths, archive_temp_dirs) =
        archives::collect_ingest_paths(directory, filter_regex.as_ref(), false);

    let lib_path = resolve_pdfium_path();
    const REPAIR_LEVEL: pdf_repair::RepairLevel = pdf_repair::RepairLevel::Normal;

    let paths_len = paths.len();
    let parallel_files = parallel_files.unwrap_or(
        paths_len > 1 && !cfg!(target_os = "windows"),
    );

    py.allow_threads(|| {
        let extract_pdf = |pdfium: &Pdfium, path: &Path| -> Vec<String> {
            extract_pdf_page_texts(pdfium, path, max_pages, repair_pdf, REPAIR_LEVEL, None)
                .unwrap_or_default()
                .into_iter()
                .map(|t| {
                    let (s, _, _) = pii::sanitize_text(&t, pii_mode);
                    s
                })
                .collect()
        };

        let extract_non_pdf = |path: &Path| -> Vec<String> {
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if formats::is_image_extension(&ext) {
                return Vec::new();
            }
            let (pages, _) = extract_non_pdf_pages(path, &ext);
            pages
                .into_iter()
                .map(|p| {
                    let (s, _, _) = pii::sanitize_text(&p.text, pii_mode);
                    s
                })
                .collect()
        };

        let texts: Vec<String> = if parallel_files {
            paths
                .par_iter()
                .flat_map(|p| {
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext.eq_ignore_ascii_case("pdf") {
                        with_thread_pdfium(&lib_path, |pdfium| extract_pdf(pdfium, p))
                    } else {
                        extract_non_pdf(p)
                    }
                })
                .collect()
        } else {
            let mut out = Vec::new();
            with_thread_pdfium(&lib_path, |pdfium| {
                for path in &paths {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext.eq_ignore_ascii_case("pdf") {
                        out.extend(extract_pdf(pdfium, path));
                    } else {
                        out.extend(extract_non_pdf(path));
                    }
                }
            });
            out
        };
        archives::cleanup_temp_dirs(&archive_temp_dirs);
        Ok(texts)
    })
}

#[pymodule]
fn brainpipe(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ingest, m)?)?;
    m.add_function(wrap_pyfunction!(ingest_text, m)?)?;
    m.add_function(wrap_pyfunction!(build_ingest_report, m)?)?;
    m.add_function(wrap_pyfunction!(sanitize_pii, m)?)?;
    m.add_class::<DocumentPage>()?;
    m.add_class::<ChildChunk>()?;
    m.add_class::<ChunkStream>()?;
    m.add_class::<ingest_report::IngestReport>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recursive_structural_chunking() {
        let text = "# Header 1\nThis is a paragraph. It has some text.\n\n## Header 2\nThis is another paragraph.\nIt spans multiple lines.\n\nAnd a list:\n- Item 1\n- Item 2\n";
        let chunks = recursive_structural_chunk(text, 50);
        
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.contains("# Header 1")));
        assert!(chunks.iter().any(|c| c.contains("## Header 2")));
        
        for chunk in &chunks {
            assert!(chunk.len() <= 50, "Chunk exceeded max size: {:?}", chunk);
        }
    }
}
