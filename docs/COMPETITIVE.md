# BrainPipe vs competitors

## Benchmark (100 pages, Windows, `repair_pdf=False`)

| Engine | Time | Pages/s | RAM delta |
|--------|------|---------|-----------|
| **BrainPipe `ingest_text()`** | ~0.18s | ~548 | **~0.4 MB** |
| PyMuPDF `get_text()` | ~0.20s | ~514 | ~0.5 MB |
| BrainPipe TURBO + `drain_all()` | ~0.19s | ~533 | ~5 MB |
| LangChain PyPDFDirectoryLoader | ~0.66s | ~151 | ~12 MB |

On Windows, keep `parallel_files=False` (default) — parallel Pdfium + Rayon can deadlock. BrainPipe **`ingest_text()` can match or beat PyMuPDF** on the same corpus and is **~5–6× faster than LangChain** with far less RAM on the text-only path.

## Feature matrix

| Capability | BrainPipe | Unstructured | LlamaParse | Docling |
|------------|-----------|--------------|------------|---------|
| Local / offline | Yes (Rust) | Partial | API / liteparse | Yes |
| PII redact in one pass | **`pii_mode`** (Rust regex) | Parse + **Presidio** (2 steps) | No | Parse + Presidio |
| Email `.eml` / `.mbox` | Yes (fast) | Yes | Via upload | Limited |
| Outlook `.msg` | Stub + warning | Varies | Varies | Varies |
| PDF repair (qpdf/gs) | **`repair_pdf`** | External | Cloud | External |
| Broken PDF OCR | **`ocr_mode=vision`** | hi_res + OCR | Agentic | OCR |
| 40+ formats | Yes | 64+ (cloud) | 130+ (cloud) | Many |
| Drop-in LangChain | Yes | Yes | Yes | Yes |

## How to beat Unstructured on PII

Unstructured documents recommend **Microsoft Presidio** after JSON export. BrainPipe:

```python
from brainpipe import ingest, sanitize_pii

# Option A: at ingest time
pages = ingest("./data", pii_mode="redact", strategy="fast").drain_all()

# Option B: standalone
clean, n, types = sanitize_pii("Contact: alice@corp.com", "mask")
```

CLI: `brainpipe sanitize ./data --mode redact`

**Why faster:** zero second pipeline, no spaCy model load, single Rust pass over text.

## How to beat LlamaParse on cost/latency

Use **`ingest_text()`** for RAG text extraction (no cloud API). Use **`strategy=auto` + `ocr_mode=vision`** only for scans.

## How to beat Docling on email + compliance

- `.eml` with multipart bodies
- `.mbox` mail archives
- PII metadata on each page: `pii_redacted`, `pii_types`
