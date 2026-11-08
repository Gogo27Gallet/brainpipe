"""Local ingestion service. All document parsers run in disposable workers."""

from contextlib import asynccontextmanager
from pathlib import Path
import tempfile
import threading
from typing import Literal

from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.responses import FileResponse, JSONResponse, StreamingResponse, Response

from .configuration import IngestConfig
from .diagnostics import diagnose
from .export import iter_export
from .jobs import JobCapacityError, JobStore
from .pipeline import SUPPORTED_EXTENSIONS, ingest_isolated

MAX_UPLOAD_BYTES = 32 * 1024 * 1024
UPLOAD_BLOCK_BYTES = 1024 * 1024
_SYNC_SLOTS = threading.BoundedSemaphore(2)
ALLOWED_EXTENSIONS = {"." + ext for ext in SUPPORTED_EXTENSIONS}


class BodyLimit:
    """Bound and spool the body before handing it to the multipart parser."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http" or scope["method"] != "POST":
            return await self.app(scope, receive, send)
        limit = MAX_UPLOAD_BYTES + 1024 * 1024

        async def reject():
            response = JSONResponse(
                {"detail": "Request body exceeds upload limit."}, status_code=413
            )
            await response(scope, receive, send)

        length = dict(scope.get("headers", [])).get(b"content-length")
        if length is not None:
            try:
                if int(length) > limit:
                    return await reject()
            except ValueError:
                response = JSONResponse(
                    {"detail": "Invalid Content-Length."}, status_code=400
                )
                return await response(scope, receive, send)
        with tempfile.SpooledTemporaryFile(max_size=1024 * 1024) as body:
            size = 0
            while True:
                message = await receive()
                if message["type"] == "http.disconnect":
                    return
                data = message.get("body", b"")
                size += len(data)
                if size > limit:
                    return await reject()
                body.write(data)
                if not message.get("more_body", False):
                    break
            body.seek(0)
            remaining = size

            async def replay():
                nonlocal remaining
                if remaining > 0:
                    data = body.read(min(64 * 1024, remaining))
                    remaining -= len(data)
                    return {
                        "type": "http.request",
                        "body": data,
                        "more_body": remaining > 0,
                    }
                return {"type": "http.request", "body": b"", "more_body": False}

            return await self.app(scope, replay, send)


@asynccontextmanager
async def lifespan(app):
    app.state.jobs = JobStore()
    try:
        yield
    finally:
        app.state.jobs.close()


app = FastAPI(
    title="BrainPipe API",
    version="0.1.0",
    lifespan=lifespan,
    description="Isolated document ingestion with traceable chunks and cancellable local jobs.",
)
app.add_middleware(BodyLimit)


@app.get("/")
def root():
    return {
        "message": "BrainPipe: open /demo or /docs.",
        "version": "0.1.0",
        "supported_extensions": sorted(ALLOWED_EXTENSIONS),
    }


@app.get("/health")
def health():
    return {
        "status": "healthy",
        "version": "0.1.0",
        "process_isolation": True,
        "available_providers": [],
        "note": "Liveness only; use /diagnostics for dependencies.",
    }


@app.get("/diagnostics")
def diagnostics():
    return diagnose()


@app.get("/demo", include_in_schema=False)
def demo():
    return FileResponse(
        Path(__file__).parent / "static" / "index.html",
        headers={
            "Content-Security-Policy": "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; object-src 'none'; frame-ancestors 'none'"
        },
    )


@app.get("/demo.js", include_in_schema=False)
def demo_script():
    return FileResponse(
        Path(__file__).parent / "static" / "demo.js",
        media_type="application/javascript",
    )


def _copy_upload(file, directory):
    ext = Path(file.filename or "").suffix.lower()
    if ext not in ALLOWED_EXTENSIONS:
        raise HTTPException(400, detail=f"Unsupported extension: {ext}")
    path = Path(directory) / ("document" + ext)
    size = 0
    with path.open("wb") as target:
        while block := file.file.read(UPLOAD_BLOCK_BYTES):
            size += len(block)
            if size > MAX_UPLOAD_BYTES:
                raise HTTPException(413, detail="Upload exceeds 32 MiB limit.")
            target.write(block)
    if not size:
        raise HTTPException(400, detail="Empty document.")
    return path


def _config(**options):
    try:
        use_cache = options.pop("use_cache", False)
        # Keep persistent extracted text opt-in and outside temporary upload paths.
        if use_cache:
            options["cache_dir"] = str(Path(".brainpipe_cache") / "isolated-v1")
        return IngestConfig(workers=1, **options)
    except (ValueError, ImportError) as exc:
        raise HTTPException(422, detail=str(exc)) from exc


def _report(pages):
    failures = sum(p.error is not None for p in pages)
    return {
        "files_ok": int(bool(pages) and not failures),
        "files_failed": int(bool(failures)),
        "pages_total": len(pages),
        "pages_ocr": sum(p.ocr_used for p in pages),
        "avg_confidence": sum(p.extraction_confidence for p in pages)
        / max(len(pages), 1),
        "repair_count": sum("pdf_repaired" in p.metadata for p in pages),
    }


@app.post("/ingest")
def ingest_file(
    file: UploadFile = File(...),
    chunk_size: int = Form(1000, ge=1, le=100_000),
    child_chunk_size: int | None = Form(None, ge=1),
    use_cache: bool = Form(False),
    use_ocr: bool = Form(False),
    use_embeddings: bool = Form(False),
    use_vlm: bool = Form(False),
    max_pages: int = Form(500, ge=1, le=500),
    strategy: Literal["fast", "hi_res", "ocr", "auto"] = Form("fast"),
    pdf_layout: Literal["native", "reading_order"] = Form("native"),
    repair_pdf: bool = Form(False),
    ocr_mode: Literal["fast", "quality", "hybrid", "vision"] = Form("fast"),
    repair_level: Literal["normal", "aggressive"] = Form("normal"),
    pdf_password: str | None = Form(None),
):
    # Legacy response shape remains available; new jobs use the versioned schema.
    config = _config(
        chunk_size=chunk_size,
        child_chunk_size=child_chunk_size,
        repair_level=repair_level,
        chunk_overlap=min(100, chunk_size - 1),
        use_cache=use_cache,
        use_ocr=use_ocr,
        use_embeddings=use_embeddings,
        use_vlm=use_vlm,
        max_pages=max_pages,
        strategy=strategy,
        pdf_layout=pdf_layout,
        repair_pdf=repair_pdf,
        ocr_mode=ocr_mode,
        pdf_password=pdf_password,
    )
    if not _SYNC_SLOTS.acquire(blocking=False):
        raise HTTPException(
            429, detail="Synchronous ingestion is busy. Use /jobs or retry."
        )
    try:
        with tempfile.TemporaryDirectory(prefix="brainpipe_upload_") as directory:
            path = _copy_upload(file, directory)
            try:
                with ingest_isolated(path, config) as stream:
                    pages = list(stream)
            except Exception:
                raise HTTPException(500, detail="Document ingestion failed.") from None
    finally:
        _SYNC_SLOTS.release()
    records = []
    for page in pages:
        record = page.to_dict()
        record.update(
            path=file.filename,
            source=file.filename,
            chunks=[c.text for c in page.chunks],
            chunk_details=record["chunks"],
            child_chunks=[
                {
                    "text": c.text,
                    "parent_index": next(
                        i for i, p in enumerate(page.chunks) if p.id == c.parent_id
                    ),
                }
                for c in page.child_chunks
            ],
            form_fields=page.form_fields,
            error=page.error.message if page.error else None,
            error_code=page.error.code if page.error else None,
        )
        records.append(record)
    return {
        "success": bool(pages) and not any(p.error for p in pages),
        "filename": file.filename,
        "total_pages": len(pages),
        "pages": records,
        "ingest_report": _report(pages),
    }


@app.post("/jobs", status_code=202)
def create_job(
    request: Request,
    file: UploadFile = File(...),
    chunk_size: int = Form(1000, ge=1, le=100_000),
    chunk_overlap: int = Form(100, ge=0),
    chunk_unit: Literal["characters", "tokens"] = Form("characters"),
    strategy: Literal["fast", "hi_res", "ocr", "auto"] = Form("fast"),
    pdf_layout: Literal["native", "reading_order"] = Form("native"),
    pii_mode: Literal["off", "redact", "mask", "tag"] = Form("off"),
    use_cache: bool = Form(False),
    max_pages: int = Form(500, ge=1, le=500),
    timeout_seconds: float = Form(60, gt=0, le=300),
    pdf_password: str | None = Form(None),
):
    config = _config(
        chunk_size=chunk_size,
        chunk_overlap=chunk_overlap,
        chunk_unit=chunk_unit,
        strategy=strategy,
        pdf_layout=pdf_layout,
        pii_mode=pii_mode,
        use_cache=use_cache,
        max_pages=max_pages,
        timeout_seconds=timeout_seconds,
        pdf_password=pdf_password,
    )
    with tempfile.TemporaryDirectory(prefix="brainpipe_upload_") as directory:
        path = _copy_upload(file, directory)
        try:
            return request.app.state.jobs.submit(file.filename, path, config)
        except JobCapacityError:
            raise HTTPException(
                429,
                detail="The ingestion queue is full. Try again later.",
                headers={"Retry-After": "5"},
            ) from None


def _get_job(request, job_id):
    job = request.app.state.jobs.get(job_id)
    if job is None:
        raise HTTPException(404, detail="Job not found or expired.")
    return job


@app.get("/jobs/{job_id}")
def job_status(request: Request, job_id: str):
    return _get_job(request, job_id).snapshot()


@app.delete("/jobs/{job_id}")
def cancel_job(request: Request, job_id: str):
    _get_job(request, job_id)
    return request.app.state.jobs.cancel(job_id)


@app.get("/jobs/{job_id}/results")
def job_results(
    request: Request,
    job_id: str,
    format: Literal["json", "jsonl", "markdown"] = "jsonl",
):
    job = _get_job(request, job_id)
    if job.finished_at is None:
        raise HTTPException(409, detail="Job results are not ready.")
    if not job.result_path.is_file():
        raise HTTPException(404, detail="No result file is available.")
    media = {
        "json": "application/json",
        "jsonl": "application/x-ndjson",
        "markdown": "text/markdown",
    }[format]
    extension = "md" if format == "markdown" else format
    try:
        records = request.app.state.jobs.open_result(job_id)
    except FileNotFoundError:
        raise HTTPException(404, detail="Job results expired.") from None
    return StreamingResponse(
        iter_export(records, format),
        media_type=media,
        headers={
            "Content-Disposition": f'attachment; filename="brainpipe-{job.id}.{extension}"',
            "X-Brainpipe-Status": job.status,
        },
    )


@app.get("/jobs/{job_id}/pages/{page_index}/preview")
def pdf_preview(request: Request, job_id: str, page_index: int):
    from .preview import render_preview, PreviewError

    job = _get_job(request, job_id)
    if job.finished_at is None:
        raise HTTPException(409, detail="The job has not finished.")
    if not 0 <= page_index < job.pages or not job.allow_preview:
        raise HTTPException(
            404, detail="A source preview is not available for this page."
        )
    if not _SYNC_SLOTS.acquire(blocking=False):
        raise HTTPException(429, detail="Preview workers are busy. Try again later.")
    try:
        with tempfile.TemporaryDirectory(prefix="brainpipe_preview_") as directory:
            path = Path(directory) / "source.pdf"
            try:
                password = request.app.state.jobs.copy_pdf_source(job_id, path)
            except FileNotFoundError:
                raise HTTPException(404, detail="Source file expired.") from None
            try:
                png = render_preview(path, page_index, password)
            except PreviewError as exc:
                raise HTTPException(422, detail=str(exc)) from None
            return Response(
                png, media_type="image/png", headers={"Cache-Control": "no-store"}
            )
    finally:
        _SYNC_SLOTS.release()
