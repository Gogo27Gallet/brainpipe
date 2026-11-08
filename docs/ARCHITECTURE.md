# Architecture

## Layers

- `configuration.py`: immutable, validated execution/chunking policy.
- `pipeline.py`: file discovery and spawn-based process supervision, bounded
  admission, polling deadlines, cancellation and cleanup.
- `worker.py`: per-file private snapshots, POSIX limits, native extraction,
  chunk generation and serialization. Only plain records cross the process boundary.
- `_runtime.py`: discover the pinned packaged PDFium shared library without importing a second PDFium wrapper.
- `geometry.py`: geometric line/paragraph assembly, XY-cut order and chunk-to-word region mapping.
- `preview.py`: bounded disposable PDF-to-PNG workers.
- `records.py`, `chunking.py`, `export.py`: schema, exact-span structure/token
  chunks and streaming exporters.
- `cache.py`: opt-in content/configuration/version-addressed extraction cache,
  written atomically with private file permissions.
- `jobs.py`: bounded in-memory API job queue, result files and retention.
- `api.py`, `cli.py`, `static/`: HTTP/CLI entrypoints and the local explorer.
- `src/pdf_geometry.rs`: two-page bounded native geometry stream, device-coordinate conversion and capped PNG rendering.
- `src/stream.rs`: Python iterator boundary and the bounded native page channel.
- `src/archive_limits.rs`: shared actual-byte ZIP validation for Office/EPUB.
- `src/office.rs`, `formats.rs`, `extra_formats.rs`, `msg.rs`: format parsers.
- `src/ocr_enhance.rs`, `reading_order.rs`, `page_quality.rs`, `pii.rs`,
  `pdf_repair.rs`, `ingest_report.rs`: extraction helpers.
- `src/lib.rs`: native orchestration, PDFium extraction and existing chunk/model
  paths. Further extraction into smaller native modules remains possible without
  changing the high-level process boundary.

## Lifetime and backpressure

Each isolated file has a fresh spawned interpreter/process, a private staging
directory and a bounded IPC queue. The supervisor admits at most `workers`
processes, and each queue has at most `queue_size` pending records. The native
producer uses a 16-page sync channel. Parser internals can still buffer document
structures or PDF pages; these queues do not imply constant total parser memory.
The OS address-space limit and per-file output limit are separate controls.

The parent removes staging directories after normal completion, a timeout,
cancellation or a crash. A POSIX worker creates a process group so its repair
subprocesses can be terminated with it. Native PDFium instances are scoped to an
operation: pdfium-render holds a global lock for their lifetime. PDF page reads
are serialized within one process. Separate isolated processes permit independent
PDF processing; scheduling and postprocessing remain bounded.

## Cache and trust

Hashing and extraction use the same snapshot. The cache key includes the content
hash, extension, extraction configuration/limits, package version and cache
schema version. A parser behavior change must bump the package or cache version.
Cache files are local trusted application state, not an ingestion interface.
Native sled page caching is separate and disabled by the isolated worker.

The process boundary contains parser crashes and provides resource supervision.
It is not a filesystem/network sandbox. Workers inherit the service's OS access;
use restricted containers/accounts when the documents or threat model require it.

## HTTP deployment

One Uvicorn process owns an ephemeral job store: two executing jobs, eight total
admitted queued/active jobs, two synchronous `/ingest` slots, 15-minute result retention and at most 100 retained
entries. All parsing happens in isolated children. `/health` is liveness;
`/diagnostics` probes optional dependencies in another disposable process.
Multipart request bodies are bounded before parsing and spilled to temporary
storage after 1 MiB. File bytes are also bounded while copying.

A durable shared queue, authenticated object storage and shared job metadata are
needed before scaling API replicas. The included local service does not claim
those distributed-system properties.

PDF geometry streams keep one native document open while emitting pages through
a two-item channel. Closing the stream disconnects the producer and stops it at
the next send boundary. Word coordinates are transformed with PDFium's own
page-to-device mapping; layout grouping uses unrotated loose font boxes while
citations use tight display boxes. This avoids confusing page rotation with
reading direction.

Preview-enabled PDF job sources remain in private temporary storage until job
expiry. Their password, when required, remains only in job memory for that
lifetime. Preview requests copy the source under the store lock and rasterize it
in a separate resource-limited process, sharing the synchronous admission limit.
PII modes disable source previews and remove geometry when sanitization changes
text, preventing stale offsets or raw words from leaking into results.
