# Contributing

Use synthetic/redacted documents in issues and tests. Do not commit private
files, caches, credentials, native libraries or downloaded model weights.

## Setup

Use Python 3.10–3.13 and stable Rust:

```sh
python -m venv .venv
. .venv/bin/activate
pip install 'maturin>=1.10,<2'
maturin develop --extras dev,api,cli,langchain,llamaindex,tokens
python download_pdfium.py
pytest -q
ruff check brainpipe tools
python tools/evaluate.py
python tools/fuzz_documents.py --rounds 40
```

Windows activation: `.venv\Scripts\activate`. Run from the repository root for
local PDFium discovery, or set `BRAINPIPE_PDFIUM_PATH`. Basic text/Office parsing
requires neither PDFium nor ONNX Runtime. Model-based features have additional
requirements documented in README.md.

Changes affecting parsing, isolation, identity or limits need meaningful
regression cases. Tests should exercise public behavior, failures and cleanup,
not merely duplicate implementation. New output fields must preserve source
span invariants and have documented schema semantics. Never fabricate geometry,
confidence calibration or performance claims.

Use `maturin develop --release` before performance work. `tools/benchmark.py`
records dataset hashes, text equivalence, latency, throughput and sampled RSS.
Small synthetic tests are useful for regressions but are not an accuracy study.
Native PDFium reads serialize inside a process; isolated worker processes permit
independent PDF jobs without retaining the binding's lifetime-wide global lock.

## Browser smoke test

```sh
pip install '.[browser-test]'
python -m playwright install chromium
brainpipe serve --host 127.0.0.1 --port 8000
# In another terminal:
python tools/check_demo.py
```

Pass `--chromium /path/to/chromium` to use a system browser. The script uploads a
synthetic document, checks job completion, highlights a chunk and checks mobile
overflow. Screenshots can be captured with `--screenshot path.png`.

The CI matrix configures Linux/macOS/Windows and Python 3.10/3.12. A configured
matrix is not evidence that an unobserved run passed. See docs/RELEASING.md for
licensing, packaging and platform validation gates before publication.
