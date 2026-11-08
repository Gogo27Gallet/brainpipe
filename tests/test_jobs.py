import time

from fastapi.testclient import TestClient

from brainpipe import api


def wait_for_job(client, job_id):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        job = client.get(f"/jobs/{job_id}").json()
        if job["finished_at"] is not None:
            return job
        time.sleep(0.03)
    raise AssertionError("job did not finish")


def test_jobs_progress_and_all_download_formats():
    with TestClient(api.app) as client:
        response = client.post(
            "/jobs", files={"file": ("guide.md", b"# Guide\n\nHello world.")}
        )
        assert response.status_code == 202
        job = wait_for_job(client, response.json()["id"])
        assert job["status"] == "completed" and job["pages"] == 1
        result = client.get(f"/jobs/{job['id']}/results?format=json")
        page = result.json()[0]
        assert page["source"] == "guide.md" and page["chunks"][0]["kind"] == "heading"
        assert page["schema_version"] == 1
        for format in ("jsonl", "markdown"):
            response = client.get(f"/jobs/{job['id']}/results?format={format}")
            assert (
                response.status_code == 200
                and "attachment" in response.headers["content-disposition"]
            )
        assert client.get("/health").json()["process_isolation"] is True


def test_job_cancellation():
    with TestClient(api.app) as client:
        response = client.post("/jobs", files={"file": ("a.txt", b"hello")})
        job_id = response.json()["id"]
        cancelled = client.delete(f"/jobs/{job_id}")
        assert cancelled.status_code == 200
        job = wait_for_job(client, job_id)
        assert job["status"] == "cancelled" and job["cancel_requested"]


def test_unknown_jobs_and_invalid_config():
    with TestClient(api.app) as client:
        assert client.get("/jobs/no-such-job").status_code == 404
        assert client.delete("/jobs/no-such-job").status_code == 404
        response = client.post(
            "/jobs",
            data={"chunk_size": "10", "chunk_overlap": "20"},
            files={"file": ("a.txt", b"hello")},
        )
        assert response.status_code == 422


def test_request_body_limit_before_multipart_parsing(monkeypatch):
    monkeypatch.setattr(api, "MAX_UPLOAD_BYTES", 4)
    # Valid multipart framing with more than the total allowed body size.
    boundary = "test-boundary"
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="a.txt"\r\n\r\n'.encode()
        + b"x" * (1024 * 1024 + 100)
        + f"\r\n--{boundary}--\r\n".encode()
    )
    with TestClient(api.app) as client:
        response = client.post(
            "/jobs",
            content=body,
            headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
        )
        assert response.status_code == 413


def test_demo_is_served_with_no_external_scripts():
    with TestClient(api.app) as client:
        page = client.get("/demo")
        assert page.status_code == 200 and 'id="upload"' in page.text
        assert "script-src 'self'" in page.headers["content-security-policy"]
        assert client.get("/demo.js").status_code == 200


def test_job_store_capacity_and_retention(tmp_path, monkeypatch):
    from contextlib import contextmanager
    import threading
    from brainpipe import IngestConfig
    from brainpipe import jobs
    import pytest

    entered, release = threading.Event(), threading.Event()

    @contextmanager
    def hold_stream(*args):
        class Stream:
            def __iter__(self):
                entered.set()
                release.wait(3)
                return iter([])

            def cancel(self):
                release.set()

        yield Stream()

    monkeypatch.setattr(jobs, "ingest_isolated", hold_stream)
    source = tmp_path / "a.txt"
    source.write_text("hello")
    store = jobs.JobStore(workers=1, capacity=1, retention_seconds=0.1)
    try:
        job = store.submit("a.txt", source, IngestConfig())
        assert entered.wait(3)
        with pytest.raises(jobs.JobCapacityError):
            store.submit("b.txt", source, IngestConfig())
        job_ref = store._jobs[job["id"]]
        store.cancel(job["id"])
        deadline = time.monotonic() + 3
        while job_ref.finished_at is None and time.monotonic() < deadline:
            time.sleep(0.01)
        folder = job_ref.folder.name
        from pathlib import Path

        deadline = time.monotonic() + 3
        while Path(folder).exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        assert not Path(folder).exists()
        assert store.get(job["id"]) is None
    finally:
        release.set()
        store.close()
