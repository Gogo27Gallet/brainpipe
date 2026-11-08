"""Reproducible, process-isolated throughput/latency/RSS measurements.

Example: python tools/benchmark.py ./corpus --mode native --runs 3 --output benchmark.json
Compare native and pymupdf text hashes before interpreting speed ratios.
Isolated mode additionally computes hashes, validates limits and creates chunks.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time


def runner(args):
    start = time.perf_counter()
    first = None
    texts = []
    errors = 0
    if args.mode == "isolated":
        from brainpipe import IngestConfig, ingest_isolated

        with ingest_isolated(
            args.directory,
            IngestConfig(workers=args.workers, pdf_layout=args.pdf_layout),
        ) as stream:
            for page in stream:
                first = first or time.perf_counter() - start
                texts.append(page.content)
                errors += int(page.error is not None)
    elif args.mode == "native":
        import brainpipe

        for page in brainpipe.ingest(
            args.directory, use_cache=False, chunk_pages=False
        ):
            first = first or time.perf_counter() - start
            texts.append(page.content)
            errors += int(page.error is not None)
    else:
        import pymupdf

        for path in sorted(Path(args.directory).rglob("*.pdf")):
            try:
                with pymupdf.open(path) as document:
                    for page in document:
                        texts.append(page.get_text())
                        first = first or time.perf_counter() - start
            except Exception:
                errors += 1
    elapsed = time.perf_counter() - start
    normalized = [" ".join(text.split()) for text in texts]
    return {
        "seconds": elapsed,
        "first_page_seconds": first,
        "pages": len(texts),
        "errors": errors,
        "characters": sum(map(len, texts)),
        "pages_per_second": len(texts) / elapsed,
        "normalized_text_sha256": hashlib.sha256(
            json.dumps(sorted(normalized)).encode()
        ).hexdigest(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=str)
    parser.add_argument(
        "--mode", choices=["native", "isolated", "pymupdf"], default="native"
    )
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument(
        "--pdf-layout", choices=["native", "reading_order"], default="native"
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--build-label",
        default="unspecified",
        help="Record actual release/dev build used",
    )
    parser.add_argument("--runner", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.runner:
        print(json.dumps(runner(args)))
        return
    if args.runs < 1 or not Path(args.directory).is_dir():
        parser.error("an existing directory and at least one run are required")
    import psutil
    from importlib.metadata import version
    from brainpipe.pipeline import discover_files

    files = discover_files(args.directory, 100_000)
    if args.mode != "isolated" and any(
        path.suffix.lower() != ".pdf" for path, _ in files
    ):
        parser.error("native/pymupdf comparison requires a PDF-only corpus")
    corpus = hashlib.sha256()
    for path, name in files:
        corpus.update(name.encode())
        corpus.update(hashlib.sha256(path.read_bytes()).digest())
    runs = []
    for _ in range(args.runs):
        command = [
            sys.executable,
            str(Path(__file__).resolve()),
            args.directory,
            "--runner",
            "--mode",
            args.mode,
            "--workers",
            str(args.workers),
            "--pdf-layout",
            args.pdf_layout,
        ]
        process = subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        peak = 0
        deadline = time.monotonic() + 600
        try:
            monitor = psutil.Process(process.pid)
            while process.poll() is None:
                if time.monotonic() > deadline:
                    raise TimeoutError("benchmark run exceeded 600 seconds")
                try:
                    processes = [monitor] + monitor.children(recursive=True)
                    rss = sum(p.memory_info().rss for p in processes if p.is_running())
                    peak = max(peak, rss)
                except psutil.Error:
                    pass
                time.sleep(0.01)
            stdout, stderr = process.communicate(timeout=5)
            if process.returncode:
                raise RuntimeError(stderr[-2000:])
            record = json.loads(stdout)
            record["peak_process_tree_rss_bytes"] = peak
            runs.append(record)
        finally:
            if process.poll() is None:
                for child in psutil.Process(process.pid).children(recursive=True):
                    child.kill()
                process.kill()
                process.wait()
    from brainpipe import brainpipe as native

    source_root = Path(__file__).resolve().parent.parent
    source_hash = hashlib.sha256()
    sources = sorted((source_root / "brainpipe").rglob("*.py")) + sorted(
        (source_root / "src").rglob("*.rs")
    )
    sources += [
        source_root / name for name in ("Cargo.toml", "Cargo.lock", "pyproject.toml")
    ]
    for source in sources:
        source_hash.update(str(source.relative_to(source_root)).encode())
        source_hash.update(source.read_bytes())
    from brainpipe._runtime import pdfium_distribution

    runtime = native.runtime_info()
    library = Path(runtime["pdfium_path"])
    report = {
        "schema_version": 1,
        "source_sha256": source_hash.hexdigest(),
        "engine_binary_sha256": hashlib.sha256(
            Path(native.__file__).read_bytes()
        ).hexdigest(),
        "mode": args.mode,
        "build_label": args.build_label,
        "brainpipe_version": version("brainpipe"),
        "pdfium_distribution": pdfium_distribution(),
        "pdfium_binary_sha256": hashlib.sha256(library.read_bytes()).hexdigest()
        if library.is_file()
        else None,
        "pdf_layout": args.pdf_layout if args.mode == "isolated" else None,
        "python": platform.python_version(),
        "platform": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "corpus_sha256": corpus.hexdigest(),
        "files": len(files),
        "input_bytes": sum(p.stat().st_size for p, _ in files),
        "cache": False,
        "workers": args.workers if args.mode == "isolated" else None,
        "chunking": args.mode == "isolated",
        "runs": runs,
        "median_seconds": statistics.median(r["seconds"] for r in runs),
        "median_pages_per_second": statistics.median(
            r["pages_per_second"] for r in runs
        ),
        "rss_sampling_ms": 10,
        "note": "RSS sums may double-count shared mappings. Isolated mode includes safety/chunking overhead. Text hashes must agree for equivalent-output comparisons.",
    }
    payload = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(payload + "\n")
    else:
        print(payload)


if __name__ == "__main__":
    main()
