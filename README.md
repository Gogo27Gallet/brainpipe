# BrainPipe (Alpha) 🧠⚡

**Fast document ingestion library for AI pipelines (PDF, Office, images, HTML, CSV, JSON).**

BrainPipe is an early-stage library built in Rust to help developers ingest PDF and Microsoft Office documents (DOCX, XLSX, PPTX) more efficiently for AI and LLM applications.

> **Note:** This is an experimental alpha project. We are looking for feedback and real-world edge cases to improve its robustness.

## Why BrainPipe?

Ingesting large volumes of PDFs in Python can be:
- **Slow**: Especially with large datasets and multithreading limitations.
- **Memory-Heavy**: Often leading to unstable memory footprints.

BrainPipe uses a Rust core powered by `pdfium` (C++) to provide a simple, parallelized, and memory-efficient ingestion path.

## Quickstart

```python
from brainpipe import ingest

# ingest() returns a ChunkStream iterator — materialize with list()
stream = ingest("./data")
docs = list(stream)

for doc in docs:
    print(f"{doc.path}: {doc.num_chunks} chunks")

# Drop-in PyMuPDF-style plain text per page / sheet / slide
from brainpipe import ingest_text
pages = ingest_text("./data")  # list[str]: PDF pages, XLSX sheets, PPTX slides, one DOCX/txt per file

# Advanced ingestion: strategy, repair, OCR modes, quality metadata
docs = list(ingest(
    "./data",
    pattern=".*_confidential\\.pdf$",
    strategy="auto",         # fast | hi_res | ocr | auto (auto enables OCR on scanned PDFs)
    backend="pdfium",        # auto | pdfium | mupdf (optional build feature)
    repair_pdf=True,         # qpdf/gs repair on pdfium open failure only
    ocr_mode="hybrid",       # fast | quality | hybrid | vision (opt-in; default fast)
    ocr_render_scale=1.25,   # render DPI scale when OCR runs
    repair_level="normal",   # normal | aggressive (qpdf/gs chain)
    pdf_password=None,       # encrypted PDFs
    ingest_archives=False,   # one-level .zip expansion
    use_ocr=False,           # explicit OCR; strategy=ocr or auto also trigger OCR
    chunk_size=1000,
    child_chunk_size=200,
))
# Per-page quality: doc.extraction_confidence, doc.ocr_used, doc.text_density,
# doc.warnings, doc.language, doc.error (set when PDF open fails)

# Easily navigate the context-rich parent-child mapping for advanced RAG
for doc in docs:
    # 1. Print structured document metadata
    print("Metadata:", doc.metadata)
    
    # 2. Inspect interactive PDF form fields (AcroForms)
    print("Form Fields:", doc.form_fields)
    
    # 3. Process hierarchical chunks
    for child in doc.child_chunks:
        print(f"Child text: {child.text}")
        print(f"Linked Parent context: {doc.chunks[child.parent_index]}")
```


## 🖥️ Command Line Interface (CLI)

BrainPipe comes with a built-in Typer-powered CLI to run ingestion pipelines or launch servers directly from the terminal.

```bash
# General help
brainpipe --help

# Scan directory health (broken PDFs, counts)
brainpipe doctor ./data

# Run summary after ingest
report = build_ingest_report(list(ingest("./data", strategy="fast")))
print(report.files_ok, report.avg_confidence, report.pages_ocr)

# Run ingestion on a directory with progress indicators
brainpipe ingest ./data --no-cache --chunk-size 800 --strategy fast

# Run ingestion and save the chunks directly to a JSON file
brainpipe ingest ./data --output results.json

# Start the FastAPI service
brainpipe serve --host 127.0.0.1 --port 8000
```

## 🌐 FastAPI Integration

Serve your ingestion pipeline as a high-performance microservice.

### HTTP Endpoints:
- `GET /health` - Checks engine status and reports ONNX Runtime execution providers (e.g. GPU).
- `POST /ingest` - Accepts file upload and parses/chunks the document using standard engine parameters.

---

## 🔌 Integrations (Zero-Friction Drop-in)

BrainPipe offers zero-friction drop-in replacements for standard loaders in major AI frameworks, allowing you to instantly boost ingestion speed by 100x with a single line change.

### LangChain Integration
```python
# Replace: from langchain_community.document_loaders import PyPDFLoader
from brainpipe.integrations.langchain import PyPDFLoader

loader = PyPDFLoader("./large_document.pdf", chunk_size=1000)
# Instant load to LangChain Document schema!
documents = loader.load() 
```

### Unstructured-compatible export
```python
from brainpipe.integrations.unstructured import partition

elements = partition("./data", strategy="fast")
for el in elements:
    print(el.category, el.text[:80])
```

### LlamaIndex Integration
```python
# Replace: from llama_index.core import SimpleDirectoryReader
from brainpipe.integrations.llamaindex import SimpleDirectoryReader

reader = SimpleDirectoryReader(input_dir="./data", chunk_size=1000)
# Instant indexation using BrainPipe fast engine
documents = reader.load_data()
```

## Office (DOCX / XLSX / PPTX)

Walk any directory passed to `ingest()` / `ingest_text()` — `.docx`, `.xlsx`, and `.pptx` are discovered alongside PDFs and text files.

- **DOCX / PPTX**: ZIP + streaming XML (`quick-xml`) — text from `w:t` / `a:t` nodes; headers/footers included for DOCX.
- **XLSX**: `calamine` — all sheets, rows streamed as `| cell | cell |` lines (formula values only, not macro/VBA).
- **Output**: one `DocumentPage` per sheet (XLSX) or slide (PPTX); one page per DOCX/txt file. Metadata: `source_type`, `sheet` or `slide` when applicable.

```python
from brainpipe import ingest

for doc in ingest("./reports", strategy="fast", chunk_pages=True):
    if doc.metadata.get("source_type") == "xlsx":
        print(doc.metadata.get("sheet"), doc.num_chunks)
    print(doc.content[:200])
```

## Reading order (multi-column & layout)

For `layout_mode=multi_column` or `strategy=hi_res`, page text is assembled with a **recursive XY-Cut** (pure geometry, no ML) on PDFium character bounding boxes. Blocks are split on the largest horizontal or vertical whitespace gap until reading order is stable — so two-column papers, full-width titles (Z-pattern), and side callouts are read column-by-column instead of left-to-right across the page.

When `use_vlm` is enabled on complex layouts, detected YOLOv8 zones are sorted with the same XY-Cut before table markdown is emitted. The `strategy=fast` path uses bulk PDFium text and skips XY-Cut. Figures without extractable text are unchanged (OCR/`use_ocr` still applies on empty pages).

## Features
- **Parallel Processing**: Powered by Rust's `rayon`.
- **Reliable Engine**: Uses Google's `pdfium`; Office via `quick-xml` + `calamine`.
- **Built-in Cache**: Fast metadata-based caching.
- **Table-Aware Semantic Splitting**: Retains Markdown and VLM-extracted tables as indivisible blocks to protect tabular context.
- **ML-Powered OCR Fallback**: Integrated pure-Rust `ocrs` neural engine that triggers automatically on scanned/textless PDF pages.
- **Custom Document Filters**: Restrict ingestion to specific file names or structures using custom regex patterns.
- **Native Hierarchical (Parent-Child) Chunking**: Generate nested semantic sub-chunks mapped perfectly to larger parent contexts.
- **Enterprise AcroForms & Metadata Extraction**: Extract PDF catalog metadata and interactive form fields automatically.
- **Hardware-Accelerated Inflow**: Configured to run ONNX models with CUDA, DirectML, and CoreML.
- **Rich Observability**: Full terminal logging with live progress bars.

## Supported formats

| Category | Extensions |
|----------|------------|
| PDF | `.pdf` (`pdf_password`, `repair_level`, `repair_pdf`) |
| Office Open XML | `.docx`, `.xlsx`, `.pptx` |
| ODF | `.odt`, `.ods`, `.odp` |
| Legacy Office | `.xls` (calamine); `.doc`/`.ppt` stub with warning |
| Web / data | `.html`, `.csv`, `.json`, `.xml`, `.yaml`, `.ndjson`, `.log` |
| Email | `.eml`; `.msg` stub with warning |
| eBooks / markup | `.epub`, `.rtf`, `.tex`, `.rst`, `.md`, `.txt` |
| Images | `.png`, `.jpg`, `.jpeg`, `.tiff`, `.webp`, `.bmp` |
| Source code | `.py`, `.js`, `.ts`, `.rs`, `.go`, `.java`, `.c`, `.cpp`, `.h`, … |
| Archives | `.zip` one-level extract (`ingest_archives=True`) |

## Benchmarks

```bash
python bench.py                    # BrainPipe vs PyMuPDF vs LangChain (1500 pages)
python bench.py --modes text pymupdf
brainpipe bench --quick            # 100 pages + RAM
brainpipe compare ./data           # per-file vs PyMuPDF
```

Text-only fair compare: `chunk_pages=False`, `strategy="fast"`, `use_cache=False`.

Optional native CPU tuning (not enabled in `Cargo.toml` by default):

```bash
# Linux/macOS example
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## Installation
*(Automated wheels for Mac/Windows/Linux coming soon)*

```bash
git clone https://github.com/brainpipe-ai/brainpipe
cd brainpipe
pip install .

# Rust extension (release)
cargo build --release
# If pip install fails because pdfium.dll is locked, close Python processes and retry.

# Optional MuPDF backend (Linux/macOS or Windows + VS2019 v142 toolset):
cargo build --release --features mupdf
```

## License
MIT
