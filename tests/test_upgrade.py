"""Focused tests for upgrade: metadata, formats, PDF errors."""
import json
import os
import tempfile

import pytest

import brainpipe


def _pages(stream):
    if hasattr(stream, "drain_all"):
        return stream.drain_all()
    return list(stream)


SUPPORTED = {
    ".pdf", ".txt", ".md", ".docx", ".xlsx", ".pptx", ".xls",
    ".odt", ".ods", ".odp", ".rtf", ".epub", ".tex", ".rst",
    ".png", ".jpg", ".html", ".htm", ".csv", ".json", ".xml",
    ".yaml", ".yml", ".ndjson", ".log", ".eml", ".msg",
    ".py", ".js", ".rs", ".go",
}


def test_document_page_quality_fields_exist():
    """DocumentPage exposes new quality fields from Rust."""
    fields = {
        "error", "extraction_confidence", "ocr_used",
        "text_density", "warnings", "language",
    }
    with tempfile.TemporaryDirectory() as tmp:
        p = os.path.join(tmp, "hello.txt")
        with open(p, "w", encoding="utf-8") as f:
            f.write("Hello BrainPipe quality fields test.")
        stream = brainpipe.ingest(tmp, use_cache=False, strategy="fast")
        pages = _pages(stream)
    assert pages, "expected at least one page"
    page = pages[0]
    for name in fields:
        assert hasattr(page, name), f"missing field {name}"
    assert page.extraction_confidence > 0


def test_csv_json_html_ingest():
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, "data.csv"), "w", encoding="utf-8") as f:
            f.write("a,b\n1,2\n")
        with open(os.path.join(tmp, "data.json"), "w", encoding="utf-8") as f:
            json.dump({"title": "x", "n": 1}, f)
        with open(os.path.join(tmp, "page.html"), "w", encoding="utf-8") as f:
            f.write("<html><body><p>Hi</p></body></html>")
        stream = brainpipe.ingest(tmp, use_cache=False, strategy="fast")
        pages = {os.path.basename(p.path): p for p in stream}
    assert any("Hi" in p.content for p in pages.values())


def test_new_format_smoke():
    with tempfile.TemporaryDirectory() as tmp:
        fixtures = {
            "readme.rst": "Title\n=====\n\nBody text.",
            "notes.tex": r"\documentclass{article}\begin{document}Hi\end{document}",
            "config.yaml": "key: value\nlist:\n  - a",
            "feed.ndjson": '{"a":1}\n{"b":2}\n',
            "app.log": "INFO started\nINFO done\n",
            "data.xml": "<root><item>ok</item></root>",
            "mail.eml": (
                "From: a@b.com\nSubject: Test\n\nHello email body.\n"
            ),
            "script.py": "print('hi')\n",
        }
        for name, body in fixtures.items():
            with open(os.path.join(tmp, name), "w", encoding="utf-8") as f:
                f.write(body)
        pages = _pages(brainpipe.ingest(tmp, use_cache=False, strategy="fast"))
    assert len(pages) >= len(fixtures)
    texts = " ".join(p.content for p in pages)
    assert "Body" in texts or "Hi" in texts or "value" in texts or "email" in texts


def test_sanitize_pii_redact():
    if not hasattr(brainpipe, "sanitize_pii"):
        pytest.skip("sanitize_pii not in extension")
    out, n, types = brainpipe.sanitize_pii("Email: secret@corp.com", "redact")
    assert n >= 1
    assert "secret@corp.com" not in out
    assert "EMAIL" in types or n > 0


def test_msg_stub_warning():
    with tempfile.TemporaryDirectory() as tmp:
        p = os.path.join(tmp, "x.msg")
        with open(p, "wb") as f:
            f.write(b"fake msg")
        pages = _pages(brainpipe.ingest(tmp, use_cache=False, strategy="fast"))
    assert pages
    assert any("msg" in w.lower() for p in pages for w in (p.warnings or []))


def test_bad_pdf_surfaces_error_not_silent(tmp_path):
    bad = tmp_path / "corrupt.pdf"
    bad.write_bytes(b"not a real pdf %PDF-1.4 corrupt")
    stream = brainpipe.ingest(str(tmp_path), use_cache=False, strategy="fast", repair_pdf=False)
    pages = _pages(stream)
    assert pages, "expected error page, not silent skip"
    assert pages[0].error is not None or pages[0].warnings


def test_ingest_accepts_new_params():
    import inspect
    sig = inspect.signature(brainpipe.ingest)
    for param in (
        "repair_pdf", "ocr_mode", "ocr_render_scale",
        "repair_level", "pdf_password", "ingest_archives",
    ):
        assert param in sig.parameters, f"ingest missing {param}"


def test_build_ingest_report():
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, "a.txt"), "w", encoding="utf-8") as f:
            f.write("content")
        pages = _pages(brainpipe.ingest(tmp, use_cache=False, strategy="fast"))
    report = brainpipe.build_ingest_report(pages)
    assert report.files_ok >= 1
    assert report.pages_total >= 1
    assert 0.0 <= report.avg_confidence <= 1.0
