"""Versioned output schema. Character offsets always refer to page.content."""

from dataclasses import asdict, dataclass, field
from typing import Any

SCHEMA_VERSION = 1


@dataclass(frozen=True)
class ExtractionError:
    code: str
    message: str
    retryable: bool = False


@dataclass(frozen=True)
class Chunk:
    id: str
    source_id: str
    page_index: int
    text: str
    start_char: int
    end_char: int
    kind: str = "paragraph"
    section: tuple[str, ...] = ()
    token_count: int | None = None
    oversized: bool = False
    # Coordinates are absent until a parser provides real geometry.
    bbox: tuple[float, float, float, float] | None = None
    parent_id: str | None = None


@dataclass
class PageResult:
    source: str
    source_id: str
    page_index: int
    content: str = ""
    chunks: list[Chunk] = field(default_factory=list)
    child_chunks: list[Chunk] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)
    form_fields: dict[str, str] = field(default_factory=dict)
    warnings: list[str] = field(default_factory=list)
    error: ExtractionError | None = None
    extraction_confidence: float = 0.0
    ocr_used: bool = False
    cache_hit: bool = False
    schema_version: int = SCHEMA_VERSION

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> "PageResult":
        data = dict(data)
        data["chunks"] = [
            Chunk(**{**c, "section": tuple(c.get("section", ()))})
            for c in data.get("chunks", [])
        ]
        data["child_chunks"] = [
            Chunk(**{**c, "section": tuple(c.get("section", ()))})
            for c in data.get("child_chunks", [])
        ]
        if data.get("error"):
            data["error"] = ExtractionError(**data["error"])
        return cls(**data)
