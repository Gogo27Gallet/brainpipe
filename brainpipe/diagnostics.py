"""Probe optional native dependencies in a disposable subprocess."""

import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys


def _probe():
    from . import brainpipe as native

    runtime = native.runtime_info()
    from ._runtime import pdfium_distribution

    model_dir = Path(".brainpipe_cache")
    return {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "pdfium": {
            "available": runtime["pdfium_available"] == "true",
            "path": runtime["pdfium_path"],
            "distribution": pdfium_distribution(),
        },
        "onnx": {
            "library_configured": bool(os.environ.get("ORT_DYLIB_PATH")),
            "library_exists": Path(
                os.environ.get("ORT_DYLIB_PATH", "__missing__")
            ).is_file(),
        },
        "ocr_models": {
            name: (model_dir / name).is_file()
            for name in ("text-detection.rten", "text-recognition.rten")
        },
        "token_chunking": importlib.util.find_spec("tiktoken") is not None,
        "repair_tools": {
            name: shutil.which(name) is not None for name in ("qpdf", "gs")
        },
        "process_isolation": True,
        "os_memory_cpu_limits": sys.platform.startswith("linux"),
        "os_cpu_limit": os.name == "posix",
        "os_memory_limit": sys.platform.startswith("linux"),
        "native_stream_capacity": int(runtime["native_stream_capacity"]),
    }


def diagnose():
    try:
        result = subprocess.run(
            [sys.executable, "-m", "brainpipe.diagnostics"],
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
        return json.loads(result.stdout)
    except (subprocess.SubprocessError, ValueError, OSError):
        return {
            "error": "Native dependency probe failed or timed out.",
            "process_isolation": True,
        }


if __name__ == "__main__":
    print(json.dumps(_probe()))
