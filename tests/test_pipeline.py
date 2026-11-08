"""Process-level failures and bounded output against the real supervisor."""

import json
import multiprocessing
import os
import threading
import time

import pytest

from brainpipe import IngestConfig, ingest_isolated
from brainpipe import pipeline
from brainpipe.export import iter_export


def sleepy_worker(*args):
    time.sleep(30)


def crashing_worker(*args):
    os._exit(23)


@pytest.mark.parametrize(
    "options",
    [
        {"chunk_size": 0},
        {"chunk_overlap": 1000},
        {"workers": 17},
        {"timeout_seconds": float("nan")},
        {"memory_mb": 10},
        {"queue_size": 0},
    ],
)
def test_config_rejects_invalid_settings(options):
    with pytest.raises(ValueError):
        IngestConfig(**options)


def test_isolated_multifile_and_traceable_export(tmp_path):
    for name in ("a", "b", "c"):
        (tmp_path / f"{name}.md").write_text(f"# {name}\n\nHello 世界. " * 40)
    with ingest_isolated(
        tmp_path, IngestConfig(chunk_size=80, chunk_overlap=8)
    ) as stream:
        pages = list(stream)
    assert len(pages) == 3 and all(not p.error for p in pages)
    assert {p.source for p in pages} == {"a.md", "b.md", "c.md"}
    for page in pages:
        for chunk in page.chunks:
            assert page.content[chunk.start_char : chunk.end_char] == chunk.text
    exported = "".join(iter_export(pages, "json"))
    assert len(json.loads(exported)) == 3
    assert len("".join(iter_export(pages)).splitlines()) == 3
    assert "# a.md" in "".join(iter_export(pages, "markdown"))


def test_timeout_kills_worker_and_cleans_temp_directory(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("hello")
    monkeypatch.setattr(pipeline, "run_file", sleepy_worker)
    before = {p.pid for p in multiprocessing.active_children()}
    with ingest_isolated(
        tmp_path, IngestConfig(timeout_seconds=0.2, workers=1)
    ) as stream:
        page = next(stream)
        assert page.error.code == "timeout"
        assert not stream._tasks
    assert {p.pid for p in multiprocessing.active_children()} == before


def test_native_crash_becomes_error_record(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("hello")
    monkeypatch.setattr(pipeline, "run_file", crashing_worker)
    with ingest_isolated(tmp_path) as stream:
        pages = list(stream)
    assert len(pages) == 1 and pages[0].error.code == "worker_crashed"


def test_cancellation_from_another_thread(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("hello")
    monkeypatch.setattr(pipeline, "run_file", sleepy_worker)
    stream = ingest_isolated(tmp_path)
    result = []
    thread = threading.Thread(target=lambda: result.extend(stream))
    thread.start()
    time.sleep(0.15)
    stream.cancel()
    thread.join(timeout=3)
    assert not thread.is_alive() and not stream._tasks and result == []


def test_input_and_output_limits(tmp_path):
    path = tmp_path / "a.txt"
    path.write_text("hello " * 200)
    for config in (IngestConfig(max_file_bytes=4), IngestConfig(max_output_bytes=100)):
        with ingest_isolated(path, config) as stream:
            pages = list(stream)
        assert pages[-1].error.code == "resource_limit"


def test_content_cache_invalidates_after_file_changes(tmp_path):
    path = tmp_path / "a.txt"
    path.write_text("Original document")
    config = IngestConfig(cache_dir=str(tmp_path / ".cache"))

    def run():
        with ingest_isolated(path, config) as stream:
            return list(stream)[0]

    first, second = run(), run()
    assert not first.cache_hit and second.cache_hit
    assert first.chunks[0].id == second.chunks[0].id
    path.write_text("Changed document")
    third = run()
    assert not third.cache_hit and third.source_id != first.source_id


def test_cache_separates_pii_and_chunk_config(tmp_path):
    path = tmp_path / "a.txt"
    path.write_text("Contact secret@example.com for details. " * 20)
    cache = str(tmp_path / ".cache")
    with ingest_isolated(path, IngestConfig(cache_dir=cache)) as stream:
        plain = list(stream)[0]
    with ingest_isolated(
        path,
        IngestConfig(
            cache_dir=cache, pii_mode="redact", chunk_size=50, chunk_overlap=5
        ),
    ) as stream:
        redacted = list(stream)[0]
    assert not redacted.cache_hit
    assert (
        "secret@example.com" in plain.content
        and "secret@example.com" not in redacted.content
    )
    assert len(redacted.chunks) > len(plain.chunks)


def test_files_limit_and_symlinks(tmp_path):
    (tmp_path / "a.txt").write_text("hello")
    (tmp_path / "b.txt").write_text("hello")
    with pytest.raises(ValueError, match="max_files"):
        ingest_isolated(tmp_path, IngestConfig(max_files=1))
    if os.name == "posix":
        (tmp_path / "link.txt").symlink_to(tmp_path / "a.txt")
        assert len(pipeline.discover_files(tmp_path, 2)) == 2


def test_non_finite_ocr_and_zero_limits_rejected():
    with pytest.raises(ValueError):
        IngestConfig(ocr_render_scale=float("inf"))


def test_image_pixel_limit(tmp_path):
    from PIL import Image

    path = tmp_path / "a.png"
    Image.new("RGB", (100, 100)).save(path)
    with ingest_isolated(path, IngestConfig(max_image_pixels=100)) as stream:
        page = list(stream)[0]
    assert page.error.code == "resource_limit"


def test_native_bounded_stream_can_be_closed(tmp_path):
    import brainpipe

    for i in range(50):
        (tmp_path / f"{i}.txt").write_text("hello")
    stream = brainpipe.ingest(str(tmp_path), use_cache=False)
    assert next(stream).content == "hello"
    stream.close()
    assert list(stream) == []


def memory_pressure_worker(path, display_name, staging, config, output):
    from brainpipe.worker import apply_resource_limits

    apply_resource_limits(IngestConfig(**config))
    try:
        allocation = bytearray(512 * 1024 * 1024)
        output.put(
            ("error", {"code": "limit_not_applied", "message": str(len(allocation))})
        )
    except MemoryError:
        os._exit(74)


@pytest.mark.skipif(
    not __import__("sys").platform.startswith("linux"),
    reason="Deterministic Linux RLIMIT_AS test",
)
def test_low_memory_budget_does_not_crash_supervisor(tmp_path, monkeypatch):
    path = tmp_path / "a.txt"
    path.write_text("hello")
    monkeypatch.setattr(pipeline, "run_file", memory_pressure_worker)
    with ingest_isolated(path, IngestConfig(memory_mb=256)) as stream:
        pages = list(stream)
    assert pages and pages[-1].error.code == "resource_limit"
