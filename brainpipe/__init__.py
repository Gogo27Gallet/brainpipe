"""Document ingestion: native compatibility API and isolated, typed pipeline."""

from .brainpipe import (
    ChildChunk,
    ChunkStream,
    DocumentPage,
    IngestReport,
    build_ingest_report,
    ingest,
    ingest_text,
    sanitize_pii,
)
from .configuration import IngestConfig
from .records import Chunk, ExtractionError, PageResult
from .pipeline import IsolatedStream, ingest_isolated

__all__ = [
    "ingest",
    "ingest_text",
    "build_ingest_report",
    "sanitize_pii",
    "DocumentPage",
    "ChildChunk",
    "ChunkStream",
    "IngestReport",
    "IngestConfig",
    "Chunk",
    "ExtractionError",
    "PageResult",
    "IsolatedStream",
    "ingest_isolated",
]
