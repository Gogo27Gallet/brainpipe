"""
BrainPipe Benchmark — compare BrainPipe, PyMuPDF, LangChain.
Modes: text-only (chunk_pages=False) vs chunked RAG path.
Targets: beat PyMuPDF on ingest_text; 10x faster than LangChain; minimal RAM.
"""
import argparse
import gc
import os
import shutil
import sys
import time

import psutil
from reportlab.pdfgen import canvas
from reportlab.lib.pagesizes import letter

DATA_DIR = "./test_data_heavy"
NUM_PDFS = 30
PAGES_PER_PDF = 50


def _page_counts(quick: bool):
    if quick:
        return 10, 10
    return NUM_PDFS, PAGES_PER_PDF


def get_rss_mb():
    return psutil.Process(os.getpid()).memory_info().rss / (1024 * 1024)


def generate_heavy_pdfs(num_pdfs: int, pages_per_pdf: int):
    if os.path.exists(DATA_DIR):
        shutil.rmtree(DATA_DIR)
    os.makedirs(DATA_DIR)
    total = num_pdfs * pages_per_pdf
    print(f"Generating {num_pdfs} PDFs ({pages_per_pdf} pages each, {total} total)...")
    for i in range(num_pdfs):
        path = os.path.join(DATA_DIR, f"doc_{i}.pdf")
        c = canvas.Canvas(path, pagesize=letter)
        for p in range(pages_per_pdf):
            c.drawString(50, 750, f"Document {i} - Page {p}")
            text = (
                "BrainPipe benchmark validation. Complex text simulation for realistic workload. "
                * 3
            )
            for line in range(40):
                c.drawString(50, 700 - (line * 15), text[:100])
            c.showPage()
        c.save()


def _ingest_kwargs():
    import brainpipe

    return dict(
        directory=DATA_DIR,
        use_cache=False,
        chunk_pages=False,
        parallel_files=sys.platform != "win32",
        layout_mode="bulk",
        fast_chunk=True,
        strategy="fast",
        repair_pdf=False,
        backend="pdfium",
    )


def _drain(stream):
    if hasattr(stream, "drain_all"):
        return stream.drain_all()
    return list(stream)


def bench_brainpipe_turbo():
    import brainpipe

    gc.collect()
    rss_before = get_rss_mb()
    start = time.perf_counter()
    stream = brainpipe.ingest(**_ingest_kwargs())
    docs = _drain(stream)
    elapsed = time.perf_counter() - start
    rss_after = get_rss_mb()
    return len(docs), elapsed, rss_after - rss_before


def bench_brainpipe_ingest_text():
    import brainpipe

    gc.collect()
    rss_before = get_rss_mb()
    start = time.perf_counter()
    texts = brainpipe.ingest_text(
        DATA_DIR,
        parallel_files=sys.platform != "win32",
        repair_pdf=False,
        backend="pdfium",
    )
    elapsed = time.perf_counter() - start
    rss_after = get_rss_mb()
    return len(texts), elapsed, rss_after - rss_before


def bench_pymupdf():
    try:
        import fitz
    except ImportError:
        print("  PyMuPDF (fitz) not installed — pip install pymupdf")
        return 0, 0.0, 0.0

    gc.collect()
    rss_before = get_rss_mb()
    start = time.perf_counter()
    count = 0
    for name in sorted(os.listdir(DATA_DIR)):
        if not name.endswith(".pdf"):
            continue
        doc = fitz.open(os.path.join(DATA_DIR, name))
        for page in doc:
            _ = page.get_text()
            count += 1
        doc.close()
    elapsed = time.perf_counter() - start
    rss_after = get_rss_mb()
    return count, elapsed, rss_after - rss_before


def bench_langchain():
    try:
        from langchain_community.document_loaders import PyPDFDirectoryLoader
    except ImportError:
        print("  LangChain not installed — skipping.")
        return 0, 0.0, 0.0

    gc.collect()
    rss_before = get_rss_mb()
    start = time.perf_counter()
    loader = PyPDFDirectoryLoader(DATA_DIR)
    docs = loader.load()
    elapsed = time.perf_counter() - start
    rss_after = get_rss_mb()
    return len(docs), elapsed, rss_after - rss_before


def _pps(pages: int, seconds: float) -> float:
    return pages / seconds if seconds > 0 else 0.0


def main():
    parser = argparse.ArgumentParser(description="BrainPipe vs PyMuPDF vs LangChain")
    parser.add_argument("--skip-generate", action="store_true")
    parser.add_argument("--quick", action="store_true", help="100 pages (10 PDFs x 10)")
    parser.add_argument("--modes", nargs="+", default=["turbo", "text", "pymupdf", "langchain"])
    args = parser.parse_args()

    num_pdfs, pages_per_pdf = _page_counts(args.quick)
    if not args.skip_generate:
        generate_heavy_pdfs(num_pdfs, pages_per_pdf)

    total_pages = num_pdfs * pages_per_pdf
    print("-" * 60)
    print(f"BENCHMARK: {total_pages} pages ({num_pdfs} PDFs × {pages_per_pdf})")
    print(f"CPU: {psutil.cpu_count()} cores")
    print("Settings: strategy=fast, repair_pdf=False, chunk_pages=False, parallel_files=True")
    print("-" * 60)

    results = {}

    if "turbo" in args.modes:
        n, t, ram = bench_brainpipe_turbo()
        results["brainpipe_turbo"] = (t, ram)
        print(f"\nBrainPipe TURBO (ingest + drain_all, DocumentPage):")
        print(f"  Pages   : {n}")
        print(f"  Time    : {t:.3f}s  ({_pps(n, t):.0f} pages/s)")
        print(f"  RAM delta : {ram:.2f} MB")

    if "text" in args.modes:
        n, t, ram = bench_brainpipe_ingest_text()
        results["brainpipe_text"] = (t, ram)
        print(f"\nBrainPipe ingest_text() [primary speed path]:")
        print(f"  Pages   : {n}")
        print(f"  Time    : {t:.3f}s  ({_pps(n, t):.0f} pages/s)")
        print(f"  RAM delta : {ram:.2f} MB")

    if "pymupdf" in args.modes:
        n, t, ram = bench_pymupdf()
        results["pymupdf"] = (t, ram)
        print(f"\nPyMuPDF get_text():")
        print(f"  Pages   : {n}")
        print(f"  Time    : {t:.3f}s  ({_pps(n, t):.0f} pages/s)")
        print(f"  RAM delta : {ram:.2f} MB")

    if "langchain" in args.modes:
        n, t, ram = bench_langchain()
        if n:
            results["langchain"] = (t, ram)
            print(f"\nLangChain PyPDFDirectoryLoader:")
            print(f"  Docs    : {n}")
            print(f"  Time    : {t:.3f}s  ({_pps(n, t):.0f} pages/s)")
            print(f"  RAM delta : {ram:.2f} MB")

    print("-" * 60)
    bp_t = results.get("brainpipe_text", (0, 0))[0]
    pm_t = results.get("pymupdf", (0, 0))[0]
    lc_t = results.get("langchain", (0, 0))[0]

    if bp_t and pm_t:
        ratio = bp_t / pm_t
        winner = "BrainPipe FASTER" if ratio < 1.0 else "PyMuPDF faster"
        print(f"ingest_text / PyMuPDF: {ratio:.3f}x — {winner}")
    if bp_t and lc_t:
        ratio = lc_t / bp_t
        print(f"LangChain / BrainPipe: {ratio:.1f}x slower")
        if ratio >= 10:
            print("  OK: 10x faster than LangChain target MET")
        else:
            print(f"  NOTE: 10x LangChain target not met ({ratio:.1f}x; need ~{10/ratio:.1f}x more)")
    if bp_t and pm_t:
        bp_ram = results.get("brainpipe_text", (0, 0))[1]
        pm_ram = results.get("pymupdf", (0, 0))[1]
        if pm_ram > 0:
            print(f"RAM BrainPipe / PyMuPDF: {bp_ram / pm_ram:.2f}x")
    print("Tip: broken PDFs -> repair_pdf=True, strategy=auto, ocr_mode=vision")
    return 0


if __name__ == "__main__":
    sys.exit(main())
