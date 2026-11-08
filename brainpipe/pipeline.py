"""Bounded, cancellable per-file process supervision."""

from dataclasses import dataclass
import multiprocessing as mp
import os
from pathlib import Path
import queue
import signal
import tempfile
import threading
import time

from .configuration import IngestConfig
from .records import ExtractionError, PageResult
from .worker import run_file

SUPPORTED_EXTENSIONS = frozenset(
    "pdf txt md docx xlsx pptx xls doc ppt odt ods odp rtf epub tex rst png jpg jpeg tiff tif webp bmp html htm csv json xml yaml yml ndjson log eml msg mbox py js ts rs go java c cpp h cs rb php".split()
)


def discover_files(path: str | Path, max_files: int):
    root = Path(path).expanduser().resolve()
    if root.is_file():
        if root.suffix.lower().lstrip(".") not in SUPPORTED_EXTENSIONS:
            raise ValueError(f"Unsupported file extension: {root.suffix}")
        return [(root, root.name)]
    if not root.is_dir():
        raise ValueError("Input must be an existing file or directory")
    files = []
    for directory, dirs, names in os.walk(root, followlinks=False):
        dirs[:] = sorted(
            d
            for d in dirs
            if not d.startswith(".") and not Path(directory, d).is_symlink()
        )
        for name in sorted(names):
            candidate = Path(directory, name)
            if (
                candidate.is_symlink()
                or candidate.suffix.lower().lstrip(".") not in SUPPORTED_EXTENSIONS
            ):
                continue
            files.append((candidate, str(candidate.relative_to(root))))
            if len(files) > max_files:
                raise ValueError("Input exceeds max_files")
    return files


@dataclass
class _Task:
    process: object
    output: object
    staging: object
    source: str
    deadline: float
    source_id: str = ""


class IsolatedStream:
    """Use as a context manager, or call close() when stopping before exhaustion.

    Ordering across files depends on completion. page_index gives logical order.
    At most config.workers processes and queue_size pages per process are queued.
    """

    def __init__(self, path, config=None):
        self.config = config or IngestConfig()
        self._pending = iter(discover_files(path, self.config.max_files))
        self._tasks = []
        self._cancelled = threading.Event()
        self._lock = threading.RLock()
        self._closed = False
        self._exhausted = False
        self._context = mp.get_context("spawn")
        self.files_completed = 0
        self.pages_emitted = 0

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def __iter__(self) -> "IsolatedStream":
        return self

    def _fill(self):
        while not self._exhausted and len(self._tasks) < self.config.workers:
            try:
                path, source = next(self._pending)
            except StopIteration:
                self._exhausted = True
                break
            staging = tempfile.TemporaryDirectory(prefix="brainpipe_worker_")
            output = self._context.Queue(maxsize=self.config.queue_size)
            process = self._context.Process(
                target=run_file,
                args=(str(path), source, staging.name, self.config.to_dict(), output),
                daemon=False,
            )
            try:
                process.start()
            except BaseException:
                output.close()
                staging.cleanup()
                raise
            self._tasks.append(
                _Task(
                    process,
                    output,
                    staging,
                    source,
                    time.monotonic() + self.config.timeout_seconds,
                )
            )

    @staticmethod
    def _stop(task):
        process = task.process

        def send_signal(sig):
            try:
                if os.name == "posix" and os.getpgid(process.pid) == process.pid:
                    os.killpg(process.pid, sig)
                elif sig == signal.SIGTERM:
                    process.terminate()
                else:
                    process.kill()
            except ProcessLookupError:
                pass

        try:
            if process.is_alive():
                send_signal(signal.SIGTERM)
                process.join(timeout=0.5)
                if process.is_alive():
                    send_signal(getattr(signal, "SIGKILL", signal.SIGTERM))
            process.join(timeout=1)
            process.close()
        finally:
            task.output.close()
            task.output.cancel_join_thread()
            task.staging.cleanup()

    def _finish(self, task):
        self._tasks.remove(task)
        self._stop(task)
        self.files_completed += 1

    def _failure(self, task, error):
        page = PageResult(
            task.source, task.source_id, -1, error=ExtractionError(**error)
        )
        self._finish(task)
        self.pages_emitted += 1
        return page

    def __next__(self) -> PageResult:
        with self._lock:
            if self._closed or self._cancelled.is_set():
                self.close()
                raise StopIteration
            try:
                self._fill()
                while self._tasks:
                    if self._cancelled.is_set():
                        self.close()
                        raise StopIteration
                    for task in list(self._tasks):
                        if time.monotonic() >= task.deadline:
                            return self._failure(
                                task,
                                {
                                    "code": "timeout",
                                    "message": "Document exceeded its time limit.",
                                    "retryable": True,
                                },
                            )
                        try:
                            kind, payload = task.output.get(timeout=0.02)
                        except queue.Empty:
                            if not task.process.is_alive():
                                code = task.process.exitcode
                                error_code = (
                                    "resource_limit"
                                    if code in (74, -getattr(signal, "SIGXCPU", 999))
                                    else "worker_crashed"
                                )
                                return self._failure(
                                    task,
                                    {
                                        "code": error_code,
                                        "message": "Worker exceeded an OS resource limit."
                                        if error_code == "resource_limit"
                                        else f"Document worker exited unexpectedly (status {code}).",
                                        "retryable": error_code != "resource_limit",
                                    },
                                )
                            continue
                        if kind == "source":
                            task.source_id = payload
                        elif kind == "page":
                            self.pages_emitted += 1
                            return PageResult.from_dict(payload)
                        elif kind == "error":
                            return self._failure(task, payload)
                        elif kind == "done":
                            self._finish(task)
                    self._fill()
                self.close()
                raise StopIteration
            except StopIteration:
                raise
            except BaseException:
                self.close()
                raise

    def cancel(self) -> None:
        """Signal cancellation from any thread; the next poll releases workers."""
        self._cancelled.set()

    def close(self) -> None:
        self._cancelled.set()
        with self._lock:
            if self._closed:
                return
            self._closed = True
            for task in list(self._tasks):
                self._tasks.remove(task)
                self._stop(task)

    def __del__(self):
        # Best effort only: deterministic cleanup uses the context manager.
        if hasattr(self, "_closed") and not self._closed:
            self.close()


def ingest_isolated(
    path: str | Path, config: IngestConfig | None = None
) -> IsolatedStream:
    return IsolatedStream(path, config)
