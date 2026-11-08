"""Regression tests against the actual compiled Rust extension."""

import subprocess
import os
import sys
import zipfile
from pathlib import Path

import pytest

import brainpipe


@pytest.mark.parametrize(
    "options",
    [
        {"chunk_size": 0},
        {"child_chunk_size": 0},
        {"max_pages": 0},
        {"max_concurrent_images": 0},
        {"ocr_render_scale": float("nan")},
        {"ocr_render_scale": 0},
    ],
)
def test_invalid_native_options(tmp_path, options):
    with pytest.raises(ValueError):
        brainpipe.ingest(str(tmp_path), **options)


def test_missing_directory(tmp_path):
    for fn in (brainpipe.ingest, brainpipe.ingest_text):
        with pytest.raises(ValueError):
            fn(str(tmp_path / "missing"))


def test_long_unicode_paragraph_respects_chunk_size(tmp_path):
    (tmp_path / "a.txt").write_text("bonjour 世界 🧠 " * 100, encoding="utf-8")
    pages = list(brainpipe.ingest(str(tmp_path), chunk_size=40, use_cache=False))
    assert pages and len(pages[0].chunks) > 1
    assert all(0 < len(c.encode("utf-8")) <= 40 for c in pages[0].chunks)
    assert "".join(pages[0].content.split()) == "".join(
        "".join(pages[0].chunks).split()
    )


def test_text_only_without_pdfium(tmp_path):
    (tmp_path / "a.txt").write_text("hello")
    # An explicit unavailable runtime must not break text-only extraction.
    script = "import brainpipe; assert brainpipe.ingest_text('.', parallel_files=False) == ['hello']; assert len(list(brainpipe.ingest('.', use_cache=False, chunk_pages=False))) == 1"
    subprocess.run(
        [sys.executable, "-c", script],
        cwd=tmp_path,
        check=True,
        timeout=30,
        env={**os.environ, "BRAINPIPE_PDFIUM_PATH": str(tmp_path / "missing-library")},
    )


def test_archive_duplicate_names_preserved_and_cleaned(tmp_path):
    with zipfile.ZipFile(tmp_path / "a.zip", "w") as z:
        z.writestr("first/report.txt", "first")
        z.writestr("second/report.txt", "second")
    pages = list(brainpipe.ingest(str(tmp_path), ingest_archives=True, use_cache=False))
    assert {p.content for p in pages} == {"first", "second"}
    assert all(not Path(p.path).exists() for p in pages)


def test_unsafe_archive_is_rejected(tmp_path):
    with zipfile.ZipFile(tmp_path / "a.zip", "w") as z:
        z.writestr("../outside.txt", "unsafe")
    with pytest.raises(ValueError, match="Unsafe ZIP"):
        brainpipe.ingest(str(tmp_path), ingest_archives=True)


def test_archive_entry_limit(tmp_path):
    with zipfile.ZipFile(tmp_path / "a.zip", "w") as z:
        for i in range(1001):
            z.writestr(f"{i}.txt", "x")
    with pytest.raises(ValueError, match="entry count"):
        brainpipe.ingest(str(tmp_path), ingest_archives=True)


@pytest.fixture
def multipage_pdf(tmp_path):
    from reportlab.pdfgen import canvas

    path = tmp_path / "input.pdf"
    pdf = canvas.Canvas(str(path))
    for i in range(3):
        pdf.drawString(
            50, 750, f"Page {i}: contact secret@example.com. " + "Some words here. " * 8
        )
        pdf.showPage()
    pdf.save()
    return path


def test_cached_pages_respect_chunk_size_and_pii(multipage_pdf, monkeypatch, tmp_path):
    # Keep PDFium visible while isolating sled storage.
    import shutil

    library = Path.cwd() / "libpdfium.so"
    if library.exists():
        shutil.copy(library, tmp_path / library.name)
    monkeypatch.chdir(tmp_path)

    def run(**options):
        return list(brainpipe.ingest(str(tmp_path), parallel_files=False, **options))

    original = run(chunk_size=1000)
    smaller = run(chunk_size=30)
    redacted = run(chunk_size=1000, pii_mode="redact")
    assert len(original) == len(smaller) == len(redacted) == 3
    assert len(smaller[0].chunks) > len(original[0].chunks)
    assert "secret@example.com" in original[0].content
    assert all("secret@example.com" not in p.content for p in redacted)


def test_langchain_returns_all_pdf_pages(multipage_pdf):
    pytest.importorskip("langchain_core")
    from brainpipe.integrations.langchain import PyPDFLoader

    docs = PyPDFLoader(str(multipage_pdf), use_cache=False).load()
    assert {d.metadata["page"] for d in docs} == {0, 1, 2}
    assert any("Page 2" in d.page_content for d in docs)


def test_archive_member_size_limit(tmp_path):
    with zipfile.ZipFile(
        tmp_path / "a.zip", "w", compression=zipfile.ZIP_DEFLATED
    ) as z:
        with z.open("oversized.txt", "w") as member:
            block = b"x" * (1024 * 1024)
            for _ in range(129):
                member.write(block)
    with pytest.raises(ValueError, match="uncompressed size"):
        brainpipe.ingest(str(tmp_path), ingest_archives=True)


def test_missing_pdfium_raises_python_exception(tmp_path):
    (tmp_path / "a.pdf").write_bytes(b"%PDF-1.4 invalid")
    script = """
import brainpipe
try:
    list(brainpipe.ingest('.', use_cache=False))
except RuntimeError as exc:
    assert 'PDFium unavailable' in str(exc)
else:
    raise AssertionError('expected missing PDFium error')
"""
    subprocess.run(
        [sys.executable, "-c", script],
        cwd=tmp_path,
        check=True,
        timeout=30,
        env={**os.environ, "BRAINPIPE_PDFIUM_PATH": str(tmp_path / "missing-library")},
    )


def test_repeated_multifile_pdf_ingestion_does_not_deadlock(multipage_pdf):
    import shutil

    for i in range(2):
        shutil.copy(multipage_pdf, multipage_pdf.parent / f"copy{i}.pdf")
    script = """
import brainpipe, sys
for parallel in (False, True, True):
    pages = list(brainpipe.ingest(sys.argv[1], use_cache=False, parallel_files=parallel))
    assert len(pages) == 9, len(pages)
    assert all(p.content and not p.error for p in pages)
"""
    subprocess.run(
        [sys.executable, "-c", script, str(multipage_pdf.parent)],
        check=True,
        timeout=30,
    )


def test_llamaindex_ids_include_page_index(multipage_pdf):
    pytest.importorskip("llama_index.core")
    from brainpipe.integrations.llamaindex import SimpleDirectoryReader

    documents = SimpleDirectoryReader(
        input_files=[str(multipage_pdf)], use_cache=False
    ).load_data()
    assert documents and len({d.id_ for d in documents}) == len(documents)
    assert any("_page_2_" in d.id_ for d in documents)
