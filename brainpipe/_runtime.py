"""Locate the PDFium dependency without importing/initializing a second engine."""

import importlib.util
from importlib.metadata import version, PackageNotFoundError
import os
from pathlib import Path
import sys


def configure_pdfium() -> str | None:
    explicit = os.environ.get("BRAINPIPE_PDFIUM_PATH")
    if explicit:
        return explicit
    spec = importlib.util.find_spec("pypdfium2_raw")
    if spec is None:
        return None
    library = (
        "pdfium.dll"
        if sys.platform == "win32"
        else "libpdfium.dylib"
        if sys.platform == "darwin"
        else "libpdfium.so"
    )
    for directory in spec.submodule_search_locations or ():
        candidate = Path(directory) / library
        if candidate.is_file():
            os.environ["BRAINPIPE_PDFIUM_PATH"] = str(candidate)
            return str(candidate)
    return None


def pdfium_distribution() -> dict:
    try:
        return {"package": "pypdfium2", "version": version("pypdfium2")}
    except PackageNotFoundError:
        return {"package": None, "version": None}
