"""Child-process entrypoint. Never parse untrusted documents in the supervisor."""

import hashlib
import json
import math
import os
import sys
from pathlib import Path
import shutil

from . import cache
from .configuration import IngestConfig
from .records import ExtractionError, PageResult, TextBlock, TextSpan
from .geometry import pdf_pages, locate_chunks
from .chunking import chunk_text, child_chunks


class LimitExceeded(Exception):
    pass


def apply_resource_limits(config):
    if os.name != "posix":
        return ["os_resource_limits_unavailable"]
    import resource

    resource.setrlimit(
        resource.RLIMIT_CPU, (config.cpu_seconds, config.cpu_seconds + 1)
    )
    warnings = []
    if config.memory_mb is not None and sys.platform.startswith("linux"):
        limit = config.memory_mb * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
    elif config.memory_mb is not None:
        warnings.append("os_memory_limit_unavailable")
    if hasattr(resource, "RLIMIT_CORE"):
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    return warnings


def classify_error(message):
    lower = message.lower()
    if "pdfium unavailable" in lower:
        return ExtractionError("engine_unavailable", "PDFium is not installed.")
    if any(
        marker in lower
        for marker in (
            "onnx",
            "ort build",
            "urlopen error",
            "http error",
            "model download",
        )
    ):
        return ExtractionError(
            "engine_unavailable",
            "A required extraction engine or model is unavailable.",
            True,
        )
    if "password" in lower or "encrypt" in lower:
        return ExtractionError(
            "password_required", "A valid document password is required."
        )
    if "limit" in lower or "too large" in lower:
        return ExtractionError(
            "resource_limit", "Document exceeds configured resource limits."
        )
    return ExtractionError("invalid_document", "The document could not be extracted.")


def run_file(path, display_name, staging, config_dict, output):
    """Spawn-compatible target; only versioned plain dictionaries cross the boundary."""
    source_id = ""
    try:
        # Native/library diagnostics must never corrupt a parent JSONL export.
        with open(os.devnull, "w") as sink:
            os.dup2(sink.fileno(), 1)
        if os.name == "posix":
            os.setsid()  # Repair subprocesses belong to the same killable process group.
        config = IngestConfig(**config_dict)
        warnings = apply_resource_limits(config)
        # Use a private snapshot so hashing and extraction see identical bytes.
        destination = Path(staging) / ("document" + Path(path).suffix.lower())
        digest = hashlib.sha256()
        size = 0
        with open(path, "rb") as source, destination.open("wb") as target:
            while block := source.read(1024 * 1024):
                size += len(block)
                if size > config.max_file_bytes:
                    raise LimitExceeded("input size limit")
                digest.update(block)
                target.write(block)
        source_id = digest.hexdigest()
        output.put(("source", source_id))
        if not size:
            raise ValueError("empty document")
        from . import brainpipe as native

        if destination.suffix in {
            ".png",
            ".jpg",
            ".jpeg",
            ".tif",
            ".tiff",
            ".webp",
            ".bmp",
        }:
            width, height = native.image_dimensions(str(destination))
            if width * height > config.max_image_pixels:
                raise LimitExceeded("image pixel limit")
        if destination.suffix in {".doc", ".ppt"}:
            output.put(
                (
                    "error",
                    {
                        "code": "unsupported_format",
                        "message": "Convert legacy Office documents to DOCX or PPTX.",
                    },
                )
            )
            return
        key = (
            cache.cache_key(source_id, destination.suffix, config)
            if config.cache_dir
            else ""
        )
        raw_pages = cache.load(config.cache_dir, key, config.max_output_bytes)
        hit = raw_pages is not None
        if raw_pages is None:
            geometric = (
                destination.suffix == ".pdf"
                and config.pdf_layout == "reading_order"
                and config.strategy in {"fast", "hi_res"}
                and not any(
                    (
                        config.use_ocr,
                        config.use_vlm,
                        config.use_embeddings,
                        config.repair_pdf,
                    )
                )
            )
            if geometric:
                raw_pages = pdf_pages(destination, source_id, config)
            else:
                if (
                    destination.suffix == ".pdf"
                    and config.pdf_layout == "reading_order"
                ):
                    warnings.append("geometry_unavailable_for_selected_strategy")
                stream = native.ingest(str(staging), **config.native_options())
                raw_pages = (_native_page(p) for p in stream)
        saved = []
        total = 0
        count = 0
        has_content = False
        has_error = False
        for raw in raw_pages:
            if count >= config.max_pages:
                break
            page = PageResult(
                source=display_name,
                source_id=source_id,
                page_index=raw["page_index"],
                content=raw["content"],
                form_fields=raw.get("form_fields", {}),
                blocks=[TextBlock.from_dict(b) for b in raw.get("blocks", [])],
                source_spans=[
                    TextSpan.from_dict(s) for s in raw.get("source_spans", [])
                ],
                page_width=raw.get("page_width"),
                page_height=raw.get("page_height"),
                metadata=raw.get("metadata", {}),
                warnings=raw.get("warnings", []) + warnings,
                error=ExtractionError(**raw["error"]) if raw.get("error") else None,
                extraction_confidence=raw.get("extraction_confidence", 0),
                ocr_used=raw.get("ocr_used", False),
                cache_hit=hit,
            )
            has_content |= bool(page.content.strip())
            has_error |= page.error is not None
            if config.pii_mode != "off":
                page.form_fields = {
                    native.sanitize_pii(k, config.pii_mode)[0]: native.sanitize_pii(
                        v, config.pii_mode
                    )[0]
                    for k, v in page.form_fields.items()
                }
            page.chunks = chunk_text(page.content, source_id, page.page_index, config)
            page.child_chunks = child_chunks(page.content, page.chunks, config)
            page.chunks = locate_chunks(
                page.chunks, page.source_spans, page.blocks, page.content
            )
            page.child_chunks = locate_chunks(
                page.child_chunks, page.source_spans, page.blocks, page.content
            )
            if any(c.oversized for c in page.chunks):
                page.warnings.append("atomic_block_exceeds_chunk_budget")
            if count + 1 == config.max_pages:
                page.warnings.append("page_limit_reached")
            record = page.to_dict()
            total += len(json.dumps(record, ensure_ascii=False).encode())
            if total > config.max_output_bytes:
                raise LimitExceeded("output size limit")
            output.put(("page", record))
            count += 1
            if config.cache_dir and not hit:
                saved.append(raw)
        if not count or (not has_content and not has_error):
            output.put(
                (
                    "error",
                    {
                        "code": "no_content",
                        "message": "No extractable pages were produced.",
                    },
                )
            )
        elif not hit:
            try:
                cache.save(config.cache_dir, key, saved, config.max_output_bytes)
            except OSError:
                pass  # Cache availability must not change extraction success.
        output.put(("done", None))
    except MemoryError:
        # Do not allocate or start a Queue feeder while already out of memory.
        # The supervisor recognizes this reserved exit status and owns cleanup.
        os._exit(74)
    except LimitExceeded:
        output.put(
            (
                "error",
                {
                    "code": "resource_limit",
                    "message": "Document exceeds configured resource limits.",
                },
            )
        )
    except BaseException as exc:
        error = classify_error(str(exc))
        output.put(
            (
                "error",
                {
                    "code": error.code,
                    "message": error.message,
                    "retryable": error.retryable,
                },
            )
        )
    finally:
        # The supervisor owns staging cleanup, including after forced termination.
        # Avoid leaking file descriptors into later work in this process.
        shutil.rmtree(staging, ignore_errors=True)


def _native_page(page):
    error = classify_error(page.error) if page.error else None
    confidence = page.extraction_confidence
    return {
        "page_index": page.page_index,
        "content": page.content,
        "form_fields": dict(page.form_fields),
        "metadata": {
            k: v for k, v in page.metadata.items() if k not in {"path", "source"}
        },
        "warnings": list(page.warnings),
        "error": error.__dict__ if error else None,
        "extraction_confidence": confidence if math.isfinite(confidence) else 0.0,
        "ocr_used": page.ocr_used,
    }
