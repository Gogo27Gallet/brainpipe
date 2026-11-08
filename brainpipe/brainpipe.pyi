from typing import Any, Iterator

class ChildChunk:
    text: str
    parent_index: int

class DocumentPage:
    path: str
    page_index: int
    content: str
    chunks: list[str]
    child_chunks: list[ChildChunk]
    metadata: dict[str, str]
    form_fields: dict[str, str]
    num_chunks: int
    error: str | None
    extraction_confidence: float
    ocr_used: bool
    text_density: float
    warnings: list[str]
    language: str | None

class IngestReport:
    files_ok: int
    files_failed: int
    pages_total: int
    pages_ocr: int
    avg_confidence: float
    repair_count: int
    by_extension: dict[str, int]

class ChunkStream(Iterator[DocumentPage]):
    def __iter__(self) -> ChunkStream: ...
    def __next__(self) -> DocumentPage: ...
    def drain_all(self) -> list[DocumentPage]: ...
    def close(self) -> None: ...

def ingest(
    directory: str,
    chunk_size: int | None = ...,
    child_chunk_size: int | None = ...,
    use_cache: bool | None = ...,
    use_ocr: bool | None = ...,
    pattern: str | None = ...,
    use_embeddings: bool | None = ...,
    use_vlm: bool | None = ...,
    max_pages: int | None = ...,
    **kwargs: Any,
) -> ChunkStream: ...
def ingest_text(
    directory: str,
    pattern: str | None = ...,
    max_pages: int | None = ...,
    backend: str | None = ...,
    parallel_files: bool | None = ...,
    repair_pdf: bool | None = ...,
    pii_mode: str | None = ...,
) -> list[str]: ...
def build_ingest_report(pages: list[DocumentPage]) -> IngestReport: ...
def sanitize_pii(text: str, mode: str | None = ...) -> tuple[str, int, list[str]]: ...
def runtime_info() -> dict[str, str]: ...
def image_dimensions(path: str) -> tuple[int, int]: ...

class PdfGeometryStream(Iterator[str]):
    def __iter__(self) -> PdfGeometryStream: ...
    def __next__(self) -> str: ...
    def close(self) -> None: ...

def pdf_geometry(
    path: str, max_pages: int = ..., password: str | None = ...
) -> PdfGeometryStream: ...
def render_pdf_page(
    path: str, page_index: int = ..., width: int = ..., password: str | None = ...
) -> bytes: ...
