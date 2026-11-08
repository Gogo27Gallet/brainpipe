# Format support and limits

| Formats | Extraction | Known limitations |
|---|---|---|
| PDF, native text | PDFium text; opt-in geometric blocks, word boxes and XY-cut order | Complex tables, RTL/vertical writing and unusual typography remain unvalidated; fast/native mode keeps its original order |
| Scanned/mixed PDF | Page-level OCR fallback with RTen models | Model download on first use; language/rotation/handwriting quality not broadly evaluated |
| PNG/JPEG/TIFF/WebP/BMP | OCR when enabled | Isolated header pixel limit; animated/multi-frame semantics not guaranteed |
| DOCX | Main body, headers and footers | Styles/heading semantics, embedded images and complex tables are not reconstructed |
| XLSX/XLS | Sheets and cached cell values as text/Markdown | No macro execution; formula recalculation and merged-cell geometry not provided |
| PPTX | Slide XML text | Reading order/notes/charts/layout not faithfully reconstructed |
| ODT/ODS/ODP | Best-effort content.xml text | Some content uses text nodes outside the current extractor's tags; evaluate before relying on output |
| EPUB | XHTML/HTML chapter text | Basic archive/item ordering; publisher-specific spine/layout semantics need evaluation |
| HTML | Text stripping | No browser rendering or JavaScript execution |
| TXT/MD/code | UTF-8 text | Invalid UTF-8 is reported; encoding autodetection is not implemented |
| CSV/JSON/NDJSON/XML/YAML | Text-oriented extraction | Not a lossless structured-data conversion API |
| EML/MBOX | Best-effort email text | MIME/charset and attachment handling are limited |
| MSG | CFB property streams | Plain/HTML body and selected metadata; attachments not ingested |
| DOC/PPT, legacy binary | Explicit unsupported-format result in isolated mode | Convert to DOCX/PPTX; native compatibility API retains its deferred-format warning |
| ZIP upload | Native Python opt-in archive expansion only | Not accepted by the isolated HTTP/job pipeline; nested archives are not expanded |

Office/EPUB ZIP readers validate entry counts and actual decompressed bytes
before parsing. Current native defaults: 1,000 entries, 128 MiB per member,
512 MiB total. These limits can reject legitimate very large documents.

Markdown structure-aware chunking preserves headings, lists, fenced code and
pipe tables in the *extracted text*. This does not mean PDF/Office visual tables
have been reconstructed accurately. Native model-based table detection remains
an optional experimental feature.

See [synthetic evaluation evidence](evidence/synthetic-evaluation.json) for the
small test corpus. Its phrase recall is not a general OCR accuracy or layout score.

The geometric reading-order mode is opt-in. Its reference evaluation includes
12 PDFs with shuffled drawing operations, one/two/three columns, titles, footers
and page rotations. Tests also check cropped pages and verify ink under cited
regions. This does not establish reading-order quality for every publication or
reconstruct PDF table cells. The local demo can overlay cited regions on an
isolated PNG rendering of the source page.
