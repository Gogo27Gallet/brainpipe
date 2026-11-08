# Security

Use `ingest_isolated` or `ingest-safe` for untrusted documents. The original
native API and framework adapters execute parsers inside the caller's process.

## Isolated defaults

- Two concurrent file workers, with two queued output records per worker.
- A 60-second wall deadline and 45-second CPU limit per document.
- 2,048 MiB virtual address-space limit per worker on Linux, including mapped
  libraries; this is not an RSS quota. Very low budgets may reject ordinary files.
- 32 MiB input, 32 MiB serialized output, 500 pages, 1,000 files per invocation.
- 25 million pixels per input image header; PDF-render allocations are separately
  contained by the worker memory/deadline limits.

POSIX CPU and Linux address-space limits are applied before parsing. Windows does not
currently enforce those OS limits and emits `os_resource_limits_unavailable`;
wall deadlines, file/output/page limits and worker termination still apply.
POSIX cancellation terminates the worker's process group, including repair
subprocesses. Windows descendant-process cleanup is not guaranteed.

Process isolation is not a filesystem/network sandbox. Workers inherit their
service account's permissions and may download OCR/model assets on demand.
For hostile inputs, additionally restrict the account/container, filesystem,
network and service concurrency. Native parser dependencies require updates too.

## HTTP service

Bind to localhost for personal use. There is no built-in authentication or
multi-tenant authorization. Job IDs are random identifiers, not an authentication
system. Put an authenticated gateway in front of remote use. Enforce connection,
request-time, concurrency and disk quotas at that gateway.

The application bounds the raw request body before multipart parsing (32 MiB
plus 1 MiB framing allowance), spills it to temporary storage after 1 MiB and
bounds the copied document separately. Upload names never become server paths.
At most eight queued/active jobs and two simultaneous synchronous `/ingest`
requests are admitted; overload returns 429. The two executing jobs and two
synchronous slots can therefore run up to four parser workers in total. Results and preview-enabled original PDFs are
private temporary files retained for 15 minutes, or removed when the service
stops. A hard server/host crash can leave OS temporary directories; operational
cleanup of abandoned `brainpipe_*` directories is still required.

## Cache, PII and archives

Persistent text caching is disabled by default in isolated/API mode. Explicitly
enabled cache files contain document text and need access/retention controls.
The native compatibility API and CLI `ingest` default to caching; disable it with
`use_cache=False` or `--no-cache`. PII matching is heuristic and does not guarantee
removal of every sensitive value. Chunk offsets refer to returned sanitized text.

Native ZIP expansion and internal Office/EPUB archive readers bound entry counts
and decompressed bytes. Nested user ZIP expansion is not supported. These limits
are independent of the file input size; ZIP bombs can be small on disk.

## Reporting

Use GitHub private vulnerability reporting if enabled; otherwise contact the
maintainer privately before publishing exploit details. Include the version,
platform and a minimal synthetic reproduction without credentials/private files.

Source previews return PNG only, through disposable workers capped at 2,000 ×
2,000 pixels and a 15-second wall deadline, with `Cache-Control: no-store`.
Preview-enabled encrypted PDFs retain their password in job memory until expiry;
it is not written to disk or returned in job metadata. All PII modes disable
original previews, and changed sanitized text drops source geometry rather than
leaving stale offsets or original words in the output.

The pinned pypdfium2 runtime dependency supplies PDFium and its third-party
notices. Keep it updated through reviewed dependency bumps and rerun geometry,
rendering and packaging tests when changing its version. An explicit
`BRAINPIPE_PDFIUM_PATH` is a trusted administrator override.
