"""Bounded, ephemeral job store for a single local API server process."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import json
from pathlib import Path
import tempfile
import threading
import time
import uuid

from .export import iter_export
from .pipeline import ingest_isolated


class JobCapacityError(Exception):
    pass


@dataclass
class Job:
    id: str
    filename: str
    folder: object
    config: object
    input_path: Path
    status: str = "queued"
    pages: int = 0
    errors: int = 0
    created_at: float = field(default_factory=time.time)
    finished_at: float | None = None
    error: str | None = None
    cancel_event: object = field(default_factory=threading.Event)
    stream: object = None
    readers: int = 0
    allow_preview: bool = False
    preview_password: str | None = field(default=None, repr=False)

    @property
    def result_path(self):
        return Path(self.folder.name) / "result.jsonl"

    def snapshot(self):
        return {
            "id": self.id,
            "filename": self.filename,
            "status": self.status,
            "pages": self.pages,
            "errors": self.errors,
            "created_at": self.created_at,
            "finished_at": self.finished_at,
            "error": self.error,
            "cancel_requested": self.cancel_event.is_set(),
            "preview_available": self.allow_preview and self.input_path.is_file(),
        }


class JobStore:
    def __init__(self, workers=2, capacity=8, retention_seconds=900, max_retained=100):
        self._executor = ThreadPoolExecutor(
            max_workers=workers, thread_name_prefix="brainpipe-job"
        )
        self._slots = threading.BoundedSemaphore(capacity)
        self._lock = threading.RLock()
        self._jobs = {}
        self._retention = retention_seconds
        self._max_retained = max_retained
        self._closed = False
        self._reaper_stop = threading.Event()
        self._reaper_thread = threading.Thread(
            target=self._reaper, name="brainpipe-retention", daemon=True
        )
        self._reaper_thread.start()

    def _reaper(self):
        interval = max(0.1, min(self._retention, 30))
        while not self._reaper_stop.wait(interval):
            with self._lock:
                self._reap()

    def _reap(self):
        completed = sorted(
            (
                j
                for j in self._jobs.values()
                if j.finished_at is not None and not j.readers
            ),
            key=lambda j: j.finished_at,
        )
        for job in completed:
            expired = time.time() - job.finished_at > self._retention
            if expired or len(self._jobs) >= self._max_retained:
                self._jobs.pop(job.id)
                job.folder.cleanup()

    def submit(self, filename, source_path, config):
        with self._lock:
            self._reap()
            if self._closed or not self._slots.acquire(blocking=False):
                raise JobCapacityError("The ingestion queue is full")
            folder = tempfile.TemporaryDirectory(prefix="brainpipe_job_")
            destination = Path(folder.name) / ("input" + Path(source_path).suffix)
            try:
                import shutil

                shutil.copyfile(source_path, destination)
                job = Job(uuid.uuid4().hex, filename, folder, config, destination)
                job.allow_preview = (
                    destination.suffix.lower() == ".pdf" and config.pii_mode == "off"
                )
                job.preview_password = (
                    config.pdf_password if job.allow_preview else None
                )
                self._jobs[job.id] = job
                self._executor.submit(self._run, job)
                return job.snapshot()
            except BaseException:
                self._jobs.pop(
                    locals().get("job", None).id if "job" in locals() else "", None
                )
                folder.cleanup()
                self._slots.release()
                raise

    def get(self, job_id):
        with self._lock:
            self._reap()
            return self._jobs.get(job_id)

    def copy_pdf_source(self, job_id, destination):
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None or not job.allow_preview or job.finished_at is None:
                raise FileNotFoundError(job_id)
            import shutil

            shutil.copyfile(job.input_path, destination)
            return job.preview_password

    def open_result(self, job_id):
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None or job.finished_at is None:
                raise FileNotFoundError(job_id)
            reader = job.result_path.open(encoding="utf-8")
            job.readers += 1

        return _ResultReader(reader, job, self._lock)

    def cancel(self, job_id):
        with self._lock:
            job = self.get(job_id)
            if job is None:
                return None
            if job.finished_at is None:
                job.cancel_event.set()
                if job.stream is not None:
                    job.stream.cancel()
            return job.snapshot()

    def _run(self, job):
        try:
            with self._lock:
                job.status = "running"
            with job.result_path.open("w", encoding="utf-8") as target:
                if not job.cancel_event.is_set():
                    with ingest_isolated(job.input_path, job.config) as stream:
                        with self._lock:
                            job.stream = stream
                            if job.cancel_event.is_set():
                                stream.cancel()
                        for page in stream:
                            if job.cancel_event.is_set():
                                break
                            page.source = job.filename
                            for part in iter_export([page]):
                                target.write(part)
                            target.flush()
                            with self._lock:
                                job.pages += 1
                                job.errors += int(page.error is not None)
            with self._lock:
                job.status = (
                    "cancelled"
                    if job.cancel_event.is_set()
                    else "failed"
                    if job.errors
                    else "completed"
                )
        except Exception:
            with self._lock:
                job.status = "cancelled" if job.cancel_event.is_set() else "failed"
                job.error = "The ingestion job could not be completed."
        finally:
            if not job.allow_preview:
                job.input_path.unlink(missing_ok=True)
            with self._lock:
                job.stream = None
                job.finished_at = time.time()
                # Credentials and configuration need not live with retained results.
                job.config = None
            self._slots.release()

    def close(self):
        self._reaper_stop.set()
        self._reaper_thread.join(timeout=1)
        with self._lock:
            self._closed = True
            for job in self._jobs.values():
                if job.finished_at is None:
                    job.cancel_event.set()
                    if job.stream is not None:
                        job.stream.cancel()
        self._executor.shutdown(wait=True)
        with self._lock:
            for job in self._jobs.values():
                job.folder.cleanup()
            self._jobs.clear()


class _ResultReader:
    def __init__(self, reader, job, lock):
        self.reader, self.job, self.lock = reader, job, lock
        self.closed = False

    def __iter__(self):
        return self

    def __next__(self):
        if self.closed:
            raise StopIteration
        while line := self.reader.readline():
            if line.strip():
                return json.loads(line)
        self.close()
        raise StopIteration

    def close(self):
        if not self.closed:
            self.closed = True
            self.reader.close()
            with self.lock:
                self.job.readers -= 1

    def __del__(self):
        self.close()
