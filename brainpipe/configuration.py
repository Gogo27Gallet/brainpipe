"""Validated, serializable configuration for the isolated ingestion API."""

from dataclasses import asdict, dataclass, field
import math
from pathlib import Path
from typing import Literal


@dataclass(frozen=True)
class IngestConfig:
    strategy: Literal["fast", "hi_res", "ocr", "auto"] = "fast"
    pdf_layout: Literal["native", "reading_order"] = "native"
    use_ocr: bool = False
    use_embeddings: bool = False
    use_vlm: bool = False
    pii_mode: Literal["off", "redact", "mask", "tag"] = "off"
    repair_pdf: bool = False
    repair_level: Literal["normal", "aggressive"] = "normal"
    pdf_password: str | None = field(default=None, repr=False)
    ocr_mode: Literal["fast", "quality", "hybrid", "vision"] = "fast"
    ocr_render_scale: float = 1.0
    chunk_size: int = 1000
    child_chunk_size: int | None = None
    chunk_overlap: int = 100
    chunk_unit: Literal["characters", "tokens"] = "characters"
    token_encoding: str = "cl100k_base"
    max_pages: int = 500
    max_files: int = 1000
    max_file_bytes: int = 32 * 1024 * 1024
    max_output_bytes: int = 32 * 1024 * 1024
    max_image_pixels: int = 25_000_000
    timeout_seconds: float = 60.0
    cpu_seconds: int = 45
    memory_mb: int | None = 2048
    workers: int = 2
    queue_size: int = 2
    cache_dir: str | None = None

    def __post_init__(self):
        for name in (
            "chunk_size",
            "max_pages",
            "max_files",
            "max_file_bytes",
            "max_output_bytes",
            "max_image_pixels",
            "cpu_seconds",
            "workers",
            "queue_size",
        ):
            value = getattr(self, name)
            if type(value) is not int or value < 1:
                raise ValueError(f"{name} must be a positive integer")
        if self.child_chunk_size is not None and (
            type(self.child_chunk_size) is not int or self.child_chunk_size < 1
        ):
            raise ValueError("child_chunk_size must be a positive integer or None")
        if self.workers > 16 or self.queue_size > 32:
            raise ValueError("workers must be <=16 and queue_size <=32")
        if (
            type(self.chunk_overlap) is not int
            or not 0 <= self.chunk_overlap < self.chunk_size
        ):
            raise ValueError("chunk_overlap must be >=0 and smaller than chunk_size")
        if self.memory_mb is not None and (
            type(self.memory_mb) is not int or self.memory_mb < 256
        ):
            raise ValueError("memory_mb must be at least 256, or None")
        for name, choices in {
            "pdf_layout": {"native", "reading_order"},
            "strategy": {"fast", "hi_res", "ocr", "auto"},
            "repair_level": {"normal", "aggressive"},
            "ocr_mode": {"fast", "quality", "hybrid", "vision"},
            "pii_mode": {"off", "redact", "mask", "tag"},
            "chunk_unit": {"characters", "tokens"},
        }.items():
            if getattr(self, name) not in choices:
                raise ValueError(f"Invalid {name}")
        if not math.isfinite(self.timeout_seconds) or self.timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be finite and positive")
        if (
            not math.isfinite(self.ocr_render_scale)
            or not 0.5 <= self.ocr_render_scale <= 3
        ):
            raise ValueError("ocr_render_scale must be between 0.5 and 3")
        if self.chunk_unit == "tokens":
            from .chunking import token_counter

            token_counter(self.token_encoding)  # Fail before starting any workers.
        if self.cache_dir is not None:
            object.__setattr__(
                self, "cache_dir", str(Path(self.cache_dir).expanduser().resolve())
            )

    def to_dict(self) -> dict:
        return asdict(self)

    def native_options(self) -> dict:
        return {
            "strategy": self.strategy,
            "use_ocr": self.use_ocr,
            "use_embeddings": self.use_embeddings,
            "use_vlm": self.use_vlm,
            "pii_mode": self.pii_mode,
            "repair_pdf": self.repair_pdf,
            "pdf_password": self.pdf_password,
            "repair_level": self.repair_level,
            "ocr_mode": self.ocr_mode,
            "ocr_render_scale": self.ocr_render_scale,
            "max_pages": self.max_pages,
            "use_cache": False,
            "parallel_files": False,
            "chunk_size": 1_000_000,
        }
