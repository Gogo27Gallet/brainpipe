# BrainPipe: Fast document ingestion for AI pipelines (PDF, Office, images, web, code).
#
# Formats: pdf, docx/xlsx/pptx, odt/ods/odp, xls, rtf, epub, html, csv/json/xml/yaml,
# eml, images, plain text & source code extensions. Legacy .doc/.ppt and .msg are stubbed.
#
# Key params on ingest(): strategy (fast|hi_res|ocr|auto), repair_pdf, repair_level,
# ocr_mode (fast|quality|hybrid|vision), ocr_render_scale, pdf_password, ingest_archives.
# After a run: build_ingest_report(list(stream)) for files_ok, pages_ocr, avg_confidence.
from .brainpipe import *

__all__ = [
    "ingest",
    "ingest_text",
    "build_ingest_report",
    "DocumentPage",
    "ChildChunk",
    "ChunkStream",
    "IngestReport",
]
