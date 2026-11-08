"""Download only the PDFium shared library for the current platform."""

import argparse
import io
import platform
import shutil
import tarfile
import urllib.request
from pathlib import Path


def download(destination: Path, release: str = "latest") -> Path:
    system = platform.system()
    systems = {
        "Linux": ("linux", "lib/libpdfium.so"),
        "Darwin": ("mac", "lib/libpdfium.dylib"),
        "Windows": ("win", "bin/pdfium.dll"),
    }
    architectures = {
        "x86_64": "x64",
        "AMD64": "x64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }
    if system not in systems or platform.machine() not in architectures:
        raise RuntimeError(f"Unsupported platform: {system} {platform.machine()}")
    target, member = systems[system]
    arch = architectures[platform.machine()]
    base = "https://github.com/bblanchon/pdfium-binaries/releases"
    route = "latest/download" if release == "latest" else f"download/{release}"
    url = f"{base}/{route}/pdfium-{target}-{arch}.tgz"
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / Path(member).name
    # Never extract archive paths. Copy only the expected regular library.
    with urllib.request.urlopen(url, timeout=60) as response:
        archive_data = response.read(128 * 1024 * 1024 + 1)
    if len(archive_data) > 128 * 1024 * 1024:
        raise RuntimeError("PDFium archive exceeds size limit")
    with tarfile.open(fileobj=io.BytesIO(archive_data), mode="r:gz") as archive:
        info = archive.getmember(member)
        if not info.isfile() or info.size > 256 * 1024 * 1024:
            raise RuntimeError("Invalid PDFium library member")
        with archive.extractfile(info) as source, output.open("wb") as sink:
            shutil.copyfileobj(source, sink)
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=Path.cwd())
    parser.add_argument(
        "--release", default="latest", help="PDFium release tag, or latest"
    )
    args = parser.parse_args()
    print(download(args.destination, args.release))
