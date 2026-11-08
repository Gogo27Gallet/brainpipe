# Changelog

## Unreleased

- Add typed `IngestConfig`, isolated per-file worker supervision, bounded queues,
  timeouts, cancellation and POSIX CPU and Linux address-space limits.
- Add schema-v1 source/chunk identities, exact text spans, Markdown structure
  preservation and offline `cl100k_base` token chunking.
- Add content/configuration-addressed atomic cache and streaming JSON/JSONL/Markdown.
- Add bounded ephemeral HTTP jobs, progress/cancellation/download endpoints,
  request-body limits and a responsive local document explorer.
- Replace optimistic health checks with liveness plus disposable diagnostics.
- Validate actual decompressed Office/EPUB ZIP bytes and surface swallowed parser
  errors. Auto PDF OCR now considers individual pages instead of document samples.
- Fix PDFium lifetime deadlocks, multi-page framework loading and page cache policy
  isolation; add native stream close/backpressure.
- Add reproducible evaluation/benchmark/mutation tools, release gates and CI checks.
- Base builds dynamically load ONNX Runtime rather than downloading CUDA binaries.
