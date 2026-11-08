# Recorded local validation

Validated on Linux / CPython 3.12.14. Remote CI matrices are configured; their results are separate evidence.

- Native release build completed; 73 regression tests passed.
- Wheel installed into a clean virtual environment: base import, offline tokenizer, isolated text ingestion, typing marker, demo assets and dependency consistency passed.
- Wheel and sdist contents include the pipeline, native code, UI, tokenizer and notices. The locally built wheel targets CPython 3.12 / manylinux 2.39 x86_64; it is not a universal binary.
- Chromium desktop/mobile smoke: upload, job polling, source highlighting and responsive overflow checks passed. See demo-desktop.png.
- synthetic-evaluation.json: five documents, seven logical pages; expected phrase recall, exact spans and non-whitespace coverage passed. This does not measure OCR accuracy or geometric table/reading-order fidelity.
- mutation-smoke.json: 40 deterministic mutated inputs, no worker crash/timeout findings and no resource-limit-only substitutions. This is a smoke test, not exhaustive fuzzing.

## Release-build benchmark

Eight deterministic PDFs × 25 pages, 35 lines/page. Three fresh-process runs per mode, sequential mode measurement, cache disabled. Imports and the actual API path are included in timing. OS filesystem caches are not flushed. Native mode uses the text-only bulk path; isolated mode additionally snapshots/hashes inputs, supervises workers and creates chunks. These modes perform different work.

| Mode | Median elapsed (s) | Median pages/s | Pages | Errors |
|---|---:|---:|---:|---:|
| native | 0.280 | 713 | 200 | 0 |
| pymupdf | 0.279 | 716 | 200 | 0 |
| isolated | 0.814 | 246 | 200 | 0 |

All runs produced the same normalized text hash. JSON records include corpus, source and engine-binary fingerprints, first-page latency and sampled process-tree RSS. RSS sums can double-count shared mappings; 10 ms sampling can miss peaks. This small synthetic workload does not establish a general speedup.

Reproduce from the repository root:

```sh
maturin develop --release --extras benchmark,tokens
python tools/generate_benchmark_corpus.py /tmp/brainpipe-pdf-corpus
python tools/benchmark.py /tmp/brainpipe-pdf-corpus --mode native --runs 3 --build-label release
python tools/benchmark.py /tmp/brainpipe-pdf-corpus --mode pymupdf --runs 3 --build-label release
python tools/benchmark.py /tmp/brainpipe-pdf-corpus --mode isolated --runs 3 --build-label release
```

The code source fingerprint covers Python/Rust modules and Cargo/Python build configuration. Evidence JSON/PNG and documentation are not part of that fingerprint. Benchmarks were run from the unreleased working tree represented by this PR.
