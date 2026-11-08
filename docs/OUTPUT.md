# Output schema v1

The isolated Python API, job downloads and `ingest-safe` emit the same page records.
Native `DocumentPage` and the legacy `/ingest` response remain compatibility APIs.

| Field | Meaning |
|---|---|
| `schema_version` | `1`; breaking output changes require another version |
| `source` | Relative input path, or the original HTTP upload name |
| `source_id` | SHA-256 of the private input snapshot; may be empty if reading failed |
| `page_index` | Zero-based page/sheet/slide index; `-1` for a file-level failure |
| `content` | Extracted text, after requested PII sanitization |
| `chunks` | Text spans with IDs, offsets, kind, section and optional token counts |
| `child_chunks` | Optional hierarchical chunks; `parent_id` links to a parent chunk |
| `blocks`, `source_spans` | Optional geometric text blocks and word spans in native PDFs |
| `page_width`, `page_height` | Display-oriented page dimensions in PDF points, or null |
| `metadata` | Parser-provided metadata; fields vary by format |
| `error` | `null` or `{code, message, retryable}` |
| `warnings` | Non-fatal caveats, including oversize atomic blocks and page limits |
| `extraction_confidence` | Heuristic, not a calibrated probability of correctness |
| `ocr_used`, `cache_hit` | Actual extraction/cache path used |

Chunk offsets count Unicode scalar values in the returned page text, end
exclusive. Always `chunk.text == page.content[chunk.start_char:chunk.end_char]`.
They do not refer to PDF bytes, original pre-redaction text or UTF-16 offsets.
Chunk IDs hash source ID, page index, offsets and text. Changing text or chunk
boundaries changes IDs. A `section` follows Markdown headings when present;
In geometric PDF mode, larger-font lines can be classified as headings and supply
a section label; these are heuristic labels, not recovered semantic PDF tags.
`bbox` envelopes the located word regions of a chunk. `regions` retains the
individual word boxes so multi-line citations do not require one oversized
rectangle. All boxes use top-left display coordinates in points (1/72 inch),
after intrinsic page rotation/cropping. Unsupported extraction paths keep them
empty/null. A single source word can be a coarse region for a chunk ending
inside that word.

Set `pdf_layout="reading_order"` for this native-PDF mode. OCR/model/repair
combinations fall back with `geometry_unavailable_for_selected_strategy`.
Changed PII-redacted pages discard geometry with
`geometry_removed_after_pii_redaction`; original PDF previews are disabled for
all PII modes. No original word text is retained in exported geometry.

Markdown tables and fenced code blocks are atomic. They can exceed the budget,
in which case `oversized` is true. A single Unicode scalar needing more tokens
than a tiny requested budget is also emitted as oversized rather than discarded.
Token budgets use the bundled `cl100k_base` vocabulary and require the tokens extra.

Error codes include `invalid_document`, `password_required`, `engine_unavailable`,
`unsupported_format`, `resource_limit`, `timeout`, `worker_crashed` and `no_content`.
`retryable` indicates a transient/worker failure could be retried; it is not an
automatic retry policy. A worker killed by an unknown external cause is reported
as `worker_crashed`; the supervisor does not guess that it was an OOM kill.

Files can emit valid pages before later failing. Always inspect the complete
stream and job state. Reaching `max_pages` adds `page_limit_reached` to the last
page: output may be truncated. File ordering is completion-based; page indices
are the source of logical order. A cancelled stream stops yielding and releases
its processes; a cancelled HTTP job retains already-written partial results.
