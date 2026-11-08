"""Bounded PDF rasterization in a disposable process."""

import multiprocessing as mp
import os
import queue
import time

from .configuration import IngestConfig
from .worker import apply_resource_limits


class PreviewError(Exception):
    pass


def _render(path, page_index, password, output):
    try:
        apply_resource_limits(IngestConfig(cpu_seconds=10, memory_mb=2048))
        from . import brainpipe as native

        png = native.render_pdf_page(path, page_index, 1000, password)
        if len(png) > 16 * 1024 * 1024:
            raise ValueError("Preview output limit exceeded")
        output.put((True, png))
    except MemoryError:
        os._exit(74)
    except BaseException:
        output.put((False, None))


def render_preview(path, page_index, password=None, timeout=15):
    context = mp.get_context("spawn")
    output = context.Queue(maxsize=1)
    process = context.Process(
        target=_render, args=(str(path), page_index, password, output)
    )
    try:
        process.start()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                ok, payload = output.get(timeout=0.05)
            except queue.Empty:
                if not process.is_alive():
                    break
                continue
            if ok and payload.startswith(b"\x89PNG\r\n\x1a\n"):
                return payload
            break
        raise PreviewError(
            "The PDF page preview could not be rendered within resource limits."
        )
    finally:
        if process.pid is not None:
            process.join(timeout=0.2)
            if process.is_alive():
                process.terminate()
                process.join(timeout=0.5)
            if process.is_alive():
                process.kill()
                process.join(timeout=1)
            process.close()
        output.close()
        output.cancel_join_thread()
