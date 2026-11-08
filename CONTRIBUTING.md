# Contributing

BrainPipe is an experimental Rust/Python document ingestion library. Small,
reproducible fixes and test documents without private data are welcome.

## Development

Install Python 3.10+ and a current stable Rust toolchain, then run:

```sh
python -m venv .venv
. .venv/bin/activate
pip install 'maturin>=1.5,<2'
maturin develop --extras dev,api,cli,langchain
python download_pdfium.py
pytest -q
```

On Windows activate with `.venv\Scripts\activate`. Run PDF tests from the
repository root so the downloaded PDFium shared library can be found.
Basic text/Office extraction does not need PDFium or ONNX Runtime. For
embeddings/table models, install a compatible ONNX Runtime shared library
(API 24 / ONNX Runtime 1.24+) and set `ORT_DYLIB_PATH` to its absolute path.
OCR uses RTen and downloads its models on first use.

Add regression coverage for bug fixes, avoid private documents or model weights
in commits, and describe what changed and how you tested it in the pull request.
Use `maturin develop --release` for performance measurements; compare equivalent
outputs and publish dataset, hardware, settings, elapsed time and memory usage.
The debug build is for development, not performance comparisons.

PDFium access is scoped to each operation because its binding holds a global
lock for the library lifetime. Native PDF page reads are serialized; Rust
postprocessing and file scheduling can still use Rayon workers.
