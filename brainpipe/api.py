import os
import shutil
import tempfile
from typing import Optional
from fastapi import FastAPI, File, UploadFile, Form, HTTPException
from pydantic import BaseModel

import brainpipe

# Keep in sync with brainpipe.cli.SUPPORTED_EXTS (without leading dot for checks)
ALLOWED_EXTENSIONS = {
    ".pdf", ".txt", ".md", ".docx", ".xlsx", ".pptx", ".xls", ".doc", ".ppt",
    ".odt", ".ods", ".odp", ".rtf", ".epub", ".tex", ".rst",
    ".png", ".jpg", ".jpeg", ".tiff", ".tif", ".webp", ".bmp",
    ".html", ".htm", ".csv", ".json", ".xml", ".yaml", ".yml", ".ndjson", ".log",
    ".eml", ".msg",
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
}

app = FastAPI(
    title="BrainPipe API",
    description="Fast document ingestion API (PDF, Office, images, web, code).",
    version="0.1.0"
)

class HealthResponse(BaseModel):
    status: str
    version: str
    available_providers: list[str]

@app.get("/", tags=["General"])
async def root():
    return {
        "message": "Welcome to BrainPipe API! Use /docs or POST /ingest.",
        "version": "0.1.0",
        "supported_extensions": sorted(ALLOWED_EXTENSIONS),
    }

@app.get("/health", response_model=HealthResponse, tags=["General"])
async def health():
    providers = []
    try:
        import onnxruntime as ort
        providers = ort.get_available_providers()
    except Exception:
        pass
    return {
        "status": "healthy",
        "version": "0.1.0",
        "available_providers": providers
    }

@app.post("/ingest", tags=["Ingestion"])
async def ingest_file(
    file: UploadFile = File(..., description="Document to ingest."),
    chunk_size: int = Form(1000),
    child_chunk_size: Optional[int] = Form(None),
    use_cache: bool = Form(True),
    use_ocr: bool = Form(False),
    use_embeddings: bool = Form(False),
    use_vlm: bool = Form(False),
    max_pages: Optional[int] = Form(None),
    strategy: str = Form("fast"),
    repair_pdf: bool = Form(True),
    ocr_mode: str = Form("fast"),
    repair_level: str = Form("normal"),
    pdf_password: Optional[str] = Form(None),
):
    ext = os.path.splitext(file.filename or "")[1].lower()
    if ext not in ALLOWED_EXTENSIONS:
        raise HTTPException(
            status_code=400,
            detail=f"Unsupported extension: {ext}. Allowed: {', '.join(sorted(ALLOWED_EXTENSIONS))}"
        )

    temp_dir = tempfile.mkdtemp()
    try:
        file_path = os.path.join(temp_dir, file.filename)
        with open(file_path, "wb") as buffer:
            shutil.copyfileobj(file.file, buffer)

        import inspect
        ingest_kw = {
            "directory": temp_dir,
            "chunk_size": chunk_size,
            "child_chunk_size": child_chunk_size,
            "use_cache": use_cache,
            "use_ocr": use_ocr,
            "use_embeddings": use_embeddings,
            "use_vlm": use_vlm,
            "max_pages": max_pages,
            "strategy": strategy,
            "repair_pdf": repair_pdf,
            "ocr_mode": ocr_mode,
            "repair_level": repair_level,
            "pdf_password": pdf_password,
        }
        sig = inspect.signature(brainpipe.ingest)
        stream = brainpipe.ingest(**{k: v for k, v in ingest_kw.items() if k in sig.parameters})

        page_objs = list(stream)
        pages = []
        for p in page_objs:
            pages.append({
                "path": file.filename,
                "page_index": p.page_index,
                "content": p.content,
                "chunks": p.chunks,
                "child_chunks": [{"text": cc.text, "parent_index": cc.parent_index} for cc in p.child_chunks],
                "metadata": p.metadata,
                "form_fields": p.form_fields,
                "error": getattr(p, "error", None),
                "extraction_confidence": getattr(p, "extraction_confidence", None),
                "ocr_used": getattr(p, "ocr_used", False),
                "warnings": getattr(p, "warnings", []),
            })

        report = None
        if hasattr(brainpipe, "build_ingest_report"):
            r = brainpipe.build_ingest_report(page_objs)
            report = {
                "files_ok": r.files_ok,
                "files_failed": r.files_failed,
                "pages_ocr": r.pages_ocr,
                "avg_confidence": r.avg_confidence,
                "repair_count": r.repair_count,
            }

        return {
            "success": True,
            "filename": file.filename,
            "total_pages": len(pages),
            "pages": pages,
            "ingest_report": report,
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Ingestion failed: {str(e)}")
    finally:
        shutil.rmtree(temp_dir)
