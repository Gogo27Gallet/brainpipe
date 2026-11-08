import io
import json
import os
import subprocess
import sys

from PIL import Image
from PIL import ImageChops
from pypdf import PdfReader, PdfWriter
import pytest
from reportlab.pdfgen import canvas

from brainpipe import IngestConfig, ingest_isolated, PageResult
from brainpipe import brainpipe as native
from brainpipe.geometry import build_page


@pytest.fixture
def column_pdf(tmp_path):
    path = tmp_path / "columns.pdf"
    pdf = canvas.Canvas(str(path), pagesize=(600, 800), invariant=1)
    pdf.setFont("Helvetica-Bold", 18)
    pdf.drawString(40, 750, "A WIDE DOCUMENT TITLE")
    pdf.setFont("Helvetica", 10)
    # Deliberately interleave the PDF's content stream across columns.
    for i in range(5):
        pdf.drawString(40, 690 - i * 16, f"Left column paragraph {i}.")
        pdf.drawString(330, 690 - i * 16, f"Right column paragraph {i}.")
    pdf.drawString(40, 45, "DOCUMENT FOOTER")
    pdf.save()
    return path


def geometry(path):
    stream = native.pdf_geometry(str(path))
    try:
        return build_page(json.loads(next(stream)), "source")
    finally:
        stream.close()


def test_spanning_title_columns_and_footer(column_pdf):
    page = geometry(column_pdf)
    content = page["content"]
    expected = (
        ["A WIDE DOCUMENT TITLE"]
        + [f"Left column paragraph {i}." for i in range(5)]
        + [f"Right column paragraph {i}." for i in range(5)]
        + ["DOCUMENT FOOTER"]
    )
    positions = [content.index(phrase) for phrase in expected]
    assert positions == sorted(positions)
    assert len(page["blocks"]) == 4  # heading, two paragraphs, footer
    assert page["blocks"][0]["kind"] == "heading"
    assert all(
        0 <= s["start_char"] < s["end_char"] <= len(content)
        for s in page["source_spans"]
    )


@pytest.mark.parametrize("rotation", [0, 90, 180, 270])
def test_geometry_matches_rendered_ink_after_rotation(column_pdf, tmp_path, rotation):
    reader = PdfReader(column_pdf)
    reader.pages[0].rotate(rotation)
    writer = PdfWriter()
    writer.add_page(reader.pages[0])
    path = tmp_path / f"rotated-{rotation}.pdf"
    with path.open("wb") as output:
        writer.write(output)
    page = geometry(path)
    assert "Left column paragraph 0." in page["content"]
    bitmap = Image.open(io.BytesIO(native.render_pdf_page(str(path))))
    scale_x, scale_y = (
        bitmap.width / page["page_width"],
        bitmap.height / page["page_height"],
    )
    # Check actual ink behind every cited word, not just shape or range validity.
    for span in page["source_spans"]:
        x0, y0, x1, y1 = span["bbox"]
        assert (
            0 <= x0 <= x1 <= page["page_width"] and 0 <= y0 <= y1 <= page["page_height"]
        )
        crop = bitmap.crop(
            (
                int(x0 * scale_x),
                int(y0 * scale_y),
                int(x1 * scale_x) + 1,
                int(y1 * scale_y) + 1,
            )
        ).convert("RGB")
        assert (
            ImageChops.difference(crop, Image.new("RGB", crop.size, "white")).getbbox()
            is not None
        )


def test_crop_box_coordinates(column_pdf, tmp_path):
    reader = PdfReader(column_pdf)
    reader.pages[0].cropbox.lower_left = (20, 20)
    reader.pages[0].cropbox.upper_right = (580, 780)
    writer = PdfWriter()
    writer.add_page(reader.pages[0])
    path = tmp_path / "cropped.pdf"
    with path.open("wb") as out:
        writer.write(out)
    page = geometry(path)
    assert page["page_width"] == 560 and page["page_height"] == 760
    first = page["source_spans"][0]["bbox"]
    assert 19 <= first[0] <= 23 and 12 <= first[1] <= 24


def test_isolated_geometry_regions_and_cache(column_pdf, tmp_path):
    config = IngestConfig(
        pdf_layout="reading_order",
        cache_dir=str(tmp_path / ".cache"),
        child_chunk_size=40,
    )
    with ingest_isolated(column_pdf, config) as stream:
        first = list(stream)[0]
    with ingest_isolated(column_pdf, config) as stream:
        second = list(stream)[0]
    assert first.error is None and first.blocks and first.source_spans
    assert second.cache_hit
    assert first.blocks == second.blocks and first.source_spans == second.source_spans
    assert first.chunks == second.chunks
    assert all(c.bbox and c.regions for c in first.chunks + first.child_chunks)
    assert PageResult.from_dict(json.loads(json.dumps(first.to_dict()))) == first


def test_redaction_does_not_leave_raw_text_in_geometry(tmp_path):
    path = tmp_path / "private.pdf"
    pdf = canvas.Canvas(str(path))
    pdf.drawString(40, 750, "Contact secret@example.com")
    pdf.save()
    with ingest_isolated(
        path, IngestConfig(pdf_layout="reading_order", pii_mode="redact")
    ) as stream:
        page = list(stream)[0]
    assert not page.error
    assert "secret@example.com" not in json.dumps(page.to_dict())
    assert not page.blocks and not page.source_spans
    assert all(c.bbox is None for c in page.chunks)
    assert "geometry_removed_after_pii_redaction" in page.warnings


def test_bundled_pdfium_works_outside_checkout(column_pdf, tmp_path):
    folder = tmp_path / "elsewhere"
    folder.mkdir()
    environment = {k: v for k, v in os.environ.items() if k != "BRAINPIPE_PDFIUM_PATH"}
    script = 'import sys; import brainpipe; from brainpipe.brainpipe import runtime_info; assert runtime_info()["pdfium_available"]=="true"; assert len(brainpipe.ingest_text(sys.argv[1], parallel_files=False))==1'
    source = folder / "input"
    source.mkdir()
    (source / "a.pdf").write_bytes(column_pdf.read_bytes())
    subprocess.run(
        [sys.executable, "-c", script, str(source)],
        cwd=folder,
        env=environment,
        check=True,
        timeout=30,
    )


def test_geometry_stream_close(column_pdf):
    stream = native.pdf_geometry(str(column_pdf))
    next(stream)
    stream.close()
    assert list(stream) == []


def slow_preview(*args):
    import time

    time.sleep(30)


def test_preview_timeout_cleans_process(column_pdf, monkeypatch):
    import multiprocessing
    import brainpipe.preview as preview

    before = {p.pid for p in multiprocessing.active_children()}
    monkeypatch.setattr(preview, "_render", slow_preview)
    with pytest.raises(preview.PreviewError):
        preview.render_preview(column_pdf, 0, timeout=0.2)
    assert {p.pid for p in multiprocessing.active_children()} == before


def test_heading_sections_are_attached_to_pdf_chunks(column_pdf):
    with ingest_isolated(
        column_pdf, IngestConfig(pdf_layout="reading_order")
    ) as stream:
        page = list(stream)[0]
    assert page.chunks[0].kind == "heading"
    assert page.chunks[1].section == ("A WIDE DOCUMENT TITLE",)
