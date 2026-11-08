"""HTTP boundary regression tests; native ingestion is covered separately."""

from pathlib import Path
from contextlib import contextmanager

import pytest
from fastapi.testclient import TestClient

from brainpipe import api


@pytest.fixture
def client():
    return TestClient(api.app)


def test_upload_uses_server_filename_and_cleans_up(client, monkeypatch):
    seen = []

    @contextmanager
    def ingest(path, config):
        folder = Path(path).parent
        seen.append(folder)
        assert [p.name for p in folder.iterdir()] == ["document.txt"]
        assert (folder / "document.txt").read_bytes() == b"hello"
        assert config.cache_dir is None
        yield iter([])

    monkeypatch.setattr(api, "ingest_isolated", ingest)
    response = client.post("/ingest", files={"file": ("../../escaped.txt", b"hello")})
    assert response.status_code == 200
    assert seen and not seen[0].exists()


@pytest.mark.parametrize(
    "options",
    [
        {"chunk_size": "0"},
        {"child_chunk_size": "-1"},
        {"max_pages": "0"},
        {"strategy": "oops"},
        {"ocr_mode": "oops"},
        {"repair_level": "oops"},
    ],
)
def test_invalid_options_rejected(client, options):
    response = client.post("/ingest", data=options, files={"file": ("a.txt", b"hello")})
    assert response.status_code == 422


def test_upload_size_and_empty_rejected(client, monkeypatch):
    monkeypatch.setattr(api, "MAX_UPLOAD_BYTES", 4)
    for body, status in [(b"12345", 413), (b"", 400)]:
        response = client.post("/ingest", files={"file": ("a.txt", body)})
        assert response.status_code == status


def test_errors_do_not_expose_internal_paths(client, monkeypatch):
    def ingest(path, config):
        raise RuntimeError("/private/secret/file")

    monkeypatch.setattr(api, "ingest_isolated", ingest)
    response = client.post("/ingest", files={"file": ("a.txt", b"hello")})
    assert response.status_code == 500
    assert "secret" not in response.text


def test_unsupported_file_rejected(client):
    assert (
        client.post("/ingest", files={"file": ("a.exe", b"hello")}).status_code == 400
    )


def test_real_text_upload(client):
    response = client.post("/ingest", files={"file": ("hello.txt", b"Hello BrainPipe")})
    assert response.status_code == 200
    result = response.json()
    assert result["success"] is True
    assert result["total_pages"] == 1
    assert result["pages"][0]["content"] == "Hello BrainPipe"
    assert result["pages"][0]["path"] == "hello.txt"
    assert result["ingest_report"]["files_ok"] == 1


def test_legacy_hierarchical_parameters_still_work(client):
    response = client.post(
        "/ingest",
        data={"chunk_size": "120", "child_chunk_size": "30"},
        files={"file": ("a.txt", b"A longer paragraph about source references. " * 8)},
    )
    assert response.status_code == 200
    page = response.json()["pages"][0]
    assert page["child_chunks"]
    assert all(
        0 <= child["parent_index"] < len(page["chunks"])
        for child in page["child_chunks"]
    )
