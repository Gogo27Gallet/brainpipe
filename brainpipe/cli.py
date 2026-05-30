import sys
import os
import json
import typer
from pathlib import Path
from typing import Optional
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn, BarColumn, TaskProgressColumn, TimeElapsedColumn, MofNCompleteColumn
from rich import print as rprint
import pypdf
import re

import brainpipe

app = typer.Typer(help="BrainPipe CLI: Fast and reliable document ingestion engine.")
console = Console()

SUPPORTED_EXTS = {
    ".pdf", ".txt", ".md", ".docx", ".xlsx", ".pptx", ".xls", ".doc", ".ppt",
    ".odt", ".ods", ".odp", ".rtf", ".epub", ".tex", ".rst",
    ".png", ".jpg", ".jpeg", ".tiff", ".tif", ".webp", ".bmp",
    ".html", ".htm", ".csv", ".json", ".xml", ".yaml", ".yml", ".ndjson", ".log",
    ".eml", ".msg",
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".zip",
}


def count_pages(directory: str, pattern_str: Optional[str]) -> tuple[int, int]:
    total_files = 0
    total_pages = 0
    pat = re.compile(pattern_str) if pattern_str else None
    
    for path in Path(directory).rglob("*"):
        if not path.is_file():
            continue
        ext = path.suffix.lower()
        if ext not in SUPPORTED_EXTS:
            continue
        if pat and not pat.search(path.name):
            continue
            
        total_files += 1
        if ext == ".pdf":
            try:
                with open(path, "rb") as f:
                    reader = pypdf.PdfReader(f)
                    total_pages += len(reader.pages)
            except Exception:
                total_pages += 1
        else:
            total_pages += 1
            
    return total_files, total_pages

@app.command()
def ingest(
    directory: Path = typer.Argument(..., help="Directory containing documents to ingest."),
    chunk_size: int = typer.Option(1000, "--chunk-size", "-c", help="Chunk size for semantic text splitting."),
    child_chunk_size: Optional[int] = typer.Option(None, "--child-chunk-size", help="Optional smaller child chunk size for hierarchical chunking."),
    use_cache: bool = typer.Option(True, "--cache/--no-cache", help="Enable or disable caching of processed pages."),
    use_ocr: bool = typer.Option(False, "--ocr/--no-ocr", help="Enable OCR for scanned/image-only PDFs."),
    pattern: Optional[str] = typer.Option(None, "--pattern", "-p", help="Regex pattern to filter filenames."),
    use_embeddings: bool = typer.Option(False, "--embeddings/--no-embeddings", help="Use local embeddings for semantic boundary detection."),
    use_vlm: bool = typer.Option(False, "--vlm/--no-vlm", help="Use table-detection model to rebuild tables in markdown."),
    max_pages: Optional[int] = typer.Option(None, "--max-pages", help="Max pages to process per document."),
    strategy: str = typer.Option("fast", "--strategy", help="fast | hi_res | ocr | auto"),
    repair_pdf: bool = typer.Option(False, "--repair-pdf/--no-repair-pdf", help="Try qpdf/gs repair when PDF open fails."),
    pii_mode: str = typer.Option("off", "--pii-mode", help="PII: off | redact | mask | tag (Rust-native, no Presidio)."),
    ocr_mode: str = typer.Option("fast", "--ocr-mode", help="OCR: fast | quality | hybrid | vision"),
    repair_level: str = typer.Option("normal", "--repair-level", help="PDF repair: normal | aggressive"),
    pdf_password: Optional[str] = typer.Option(None, "--pdf-password", help="Password for encrypted PDFs."),
    ingest_archives: bool = typer.Option(False, "--ingest-archives/--no-ingest-archives", help="Extract one-level ZIP archives."),
    output: Optional[Path] = typer.Option(None, "--output", "-o", help="File path to save the output chunks (JSON or MD format)."),
):
    """
    Ingest all documents in a directory, perform chunking (with optional OCR/embeddings/VLM tables), and output the results.
    """
    if not directory.exists() or not directory.is_dir():
        console.print(f"[red]Error:[/red] Directory '{directory}' does not exist or is not a directory.")
        raise typer.Exit(code=1)

    console.print(f"[bold green]Starting BrainPipe Ingestion[/bold green]")
    console.print(f"Directory: [cyan]{directory}[/cyan]")
    console.print(f"Parameters: chunk_size={chunk_size}, child_chunk_size={child_chunk_size}, ocr={use_ocr}, embeddings={use_embeddings}, vlm={use_vlm}\n")

    # Estimate total pages
    with console.status("[bold yellow]Scanning directory...[/bold yellow]"):
        num_files, num_pages = count_pages(str(directory), pattern)

    if num_files == 0:
        console.print("[yellow]No supported files found in the directory.[/yellow]")
        raise typer.Exit()

    console.print(f"Found [cyan]{num_files}[/cyan] files with approximately [cyan]{num_pages}[/cyan] total pages.")

    try:
        ingest_kw = dict(
            directory=str(directory),
            chunk_size=chunk_size,
            child_chunk_size=child_chunk_size,
            use_cache=use_cache,
            use_ocr=use_ocr,
            pattern=pattern,
            use_embeddings=use_embeddings,
            use_vlm=use_vlm,
            max_pages=max_pages,
        )
        if hasattr(brainpipe, "ingest"):
            import inspect
            sig = inspect.signature(brainpipe.ingest)
            if "strategy" in sig.parameters:
                ingest_kw["strategy"] = strategy
            if "repair_pdf" in sig.parameters:
                ingest_kw["repair_pdf"] = repair_pdf
            if "ocr_mode" in sig.parameters:
                ingest_kw["ocr_mode"] = ocr_mode
            if "repair_level" in sig.parameters:
                ingest_kw["repair_level"] = repair_level
            if "pdf_password" in sig.parameters:
                ingest_kw["pdf_password"] = pdf_password
            if "ingest_archives" in sig.parameters:
                ingest_kw["ingest_archives"] = ingest_archives
            if "pii_mode" in sig.parameters:
                ingest_kw["pii_mode"] = pii_mode
        stream = brainpipe.ingest(**ingest_kw)
    except Exception as e:
        console.print(f"[red]Failed to start ingestion engine:[/red] {e}")
        raise typer.Exit(code=1)

    pages = []
    if hasattr(stream, "drain_all"):
        with console.status("[bold yellow]Ingesting (turbo path)...[/bold yellow]"):
            pages = stream.drain_all()
    else:
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            BarColumn(),
            TaskProgressColumn(),
            MofNCompleteColumn(),
            TimeElapsedColumn(),
            console=console,
        ) as progress:
            task = progress.add_task("[green]Processing pages...", total=num_pages)
            for doc_page in stream:
                pages.append(doc_page)
                progress.update(
                    task,
                    advance=1,
                    description=f"[green]Processing: {os.path.basename(doc_page.path)} (Page {doc_page.page_index})",
                )
            progress.update(task, completed=num_pages, description="[green]Done!")

    total_chunks = sum(p.num_chunks for p in pages)
    console.print(f"\n[bold green]Ingestion completed successfully![/bold green]")
    console.print(f"Total Pages Processed: [cyan]{len(pages)}[/cyan]")
    console.print(f"Total Chunks Generated: [cyan]{total_chunks}[/cyan]")
    if hasattr(brainpipe, "build_ingest_report"):
        report = brainpipe.build_ingest_report(pages)
        console.print(
            f"Ingest report: ok={report.files_ok} failed={report.files_failed} "
            f"ocr_pages={report.pages_ocr} avg_confidence={report.avg_confidence:.2f} "
            f"repairs={report.repair_count}"
        )

    if output:
        ext = output.suffix.lower()
        with console.status(f"[bold yellow]Saving results to {output}...[/bold yellow]"):
            if ext == ".json":
                data = []
                for p in pages:
                    p_data = {
                        "path": p.path,
                        "page_index": p.page_index,
                        "content": p.content,
                        "chunks": p.chunks,
                        "child_chunks": [{"text": cc.text, "parent_index": cc.parent_index} for cc in p.child_chunks],
                        "metadata": p.metadata,
                        "form_fields": p.form_fields,
                        "error": getattr(p, "error", None),
                        "extraction_confidence": getattr(p, "extraction_confidence", None),
                        "ocr_used": getattr(p, "ocr_used", False),
                        "text_density": getattr(p, "text_density", None),
                        "warnings": getattr(p, "warnings", []),
                        "language": getattr(p, "language", None),
                    }
                    data.append(p_data)
                
                with open(output, "w", encoding="utf-8") as f:
                    json.dump(data, f, ensure_ascii=False, indent=2)
            else:
                with open(output, "w", encoding="utf-8") as f:
                    for p in pages:
                        f.write(f"# Document: {p.path} - Page {p.page_index}\n\n")
                        f.write(p.content)
                        f.write("\n\n---\n\n")
        console.print(f"Results saved to [cyan]{output}[/cyan]")
    else:
        if pages:
            console.print("\n[bold]Sample Chunk Preview:[/bold]")
            sample_page = pages[0]
            if sample_page.chunks:
                console.print(f"--- First chunk from [cyan]{os.path.basename(sample_page.path)}[/cyan] (Page {sample_page.page_index}) ---")
                console.print(sample_page.chunks[0][:300] + ("..." if len(sample_page.chunks[0]) > 300 else ""))

@app.command()
def doctor(
    directory: Path = typer.Argument(..., help="Directory to scan for ingest health."),
    pattern: Optional[str] = typer.Option(None, "--pattern", "-p", help="Regex filter on filenames."),
):
    """
    Scan a directory: file counts by type, broken PDFs, and likely empty pages.
    """
    if not directory.exists() or not directory.is_dir():
        console.print(f"[red]Error:[/red] '{directory}' is not a directory.")
        raise typer.Exit(code=1)

    pat = re.compile(pattern) if pattern else None
    counts: dict[str, int] = {}
    broken_pdfs: list[str] = []
    empty_estimate = 0
    total_files = 0

    for path in directory.rglob("*"):
        if not path.is_file():
            continue
        ext = path.suffix.lower()
        if ext not in SUPPORTED_EXTS:
            continue
        if pat and not pat.search(path.name):
            continue
        total_files += 1
        counts[ext] = counts.get(ext, 0) + 1
        if ext == ".pdf":
            try:
                with open(path, "rb") as f:
                    reader = pypdf.PdfReader(f)
                    if len(reader.pages) == 0:
                        empty_estimate += 1
                    for page in reader.pages:
                        text = page.extract_text() or ""
                        if len(text.strip()) < 5:
                            empty_estimate += 1
            except Exception as e:
                broken_pdfs.append(f"{path.relative_to(directory)}: {e}")

    console.print(f"[bold]BrainPipe doctor[/bold] — {directory}\n")
    console.print(f"Supported files: [cyan]{total_files}[/cyan]")
    if counts:
        console.print("\n[bold]By extension:[/bold]")
        for ext in sorted(counts.keys()):
            console.print(f"  {ext}: {counts[ext]}")
    if broken_pdfs:
        console.print(f"\n[red]Broken PDFs ({len(broken_pdfs)}):[/red]")
        for line in broken_pdfs[:20]:
            console.print(f"  • {line}")
        if len(broken_pdfs) > 20:
            console.print(f"  … and {len(broken_pdfs) - 20} more")
    else:
        console.print("\n[green]No broken PDFs detected (pypdf open).[/green]")
    console.print(f"\nLikely sparse/empty page samples (heuristic): [yellow]{empty_estimate}[/yellow]")
    console.print(
        "\nTip: use [cyan]strategy=auto[/cyan] for scanned PDFs, "
        "[cyan]repair_pdf=True[/cyan] (default) for corrupt files."
    )


@app.command()
def bench(
    quick: bool = typer.Option(False, "--quick", help="100 pages (10×10) instead of 1500."),
    skip_generate: bool = typer.Option(False, "--skip-generate", help="Reuse existing test_data_heavy."),
):
    """Benchmark BrainPipe vs PyMuPDF (throughput + RAM)."""
    import gc
    import time
    import psutil
    from reportlab.pdfgen import canvas
    from reportlab.lib.pagesizes import letter

    data_dir = Path("./test_data_heavy")
    num_pdfs = 10 if quick else 30
    pages_per_pdf = 10 if quick else 50

    def rss_mb():
        return psutil.Process(os.getpid()).memory_info().rss / (1024 * 1024)

    if not skip_generate or not data_dir.exists():
        import shutil
        if data_dir.exists():
            shutil.rmtree(data_dir)
        data_dir.mkdir()
        console.print(f"Generating {num_pdfs}×{pages_per_pdf} pages…")
        for i in range(num_pdfs):
            path = data_dir / f"doc_{i}.pdf"
            c = canvas.Canvas(str(path), pagesize=letter)
            for p in range(pages_per_pdf):
                c.drawString(50, 750, f"Doc {i} page {p}")
                c.showPage()
            c.save()

    total_pages = num_pdfs * pages_per_pdf
    gc.collect()
    rss0 = rss_mb()
    t0 = time.perf_counter()
    if hasattr(brainpipe, "ingest_text"):
        texts = brainpipe.ingest_text(str(data_dir), parallel_files=False, backend="pdfium")
    else:
        texts = [p.content for p in brainpipe.ingest(str(data_dir), use_cache=False, strategy="fast", chunk_pages=False)]
    bp_time = time.perf_counter() - t0
    bp_rss = rss_mb() - rss0

    pm_time = None
    try:
        import fitz
        gc.collect()
        t1 = time.perf_counter()
        n = 0
        for name in sorted(os.listdir(data_dir)):
            if not name.endswith(".pdf"):
                continue
            doc = fitz.open(data_dir / name)
            for page in doc:
                _ = page.get_text()
                n += 1
            doc.close()
        pm_time = time.perf_counter() - t1
    except ImportError:
        pass

    console.print(f"\n[bold]BrainPipe bench[/bold] ({total_pages} pages, quick={quick})")
    console.print(f"  ingest_text pages: {len(texts)}")
    console.print(f"  Time: {bp_time:.3f}s ({total_pages / bp_time:.0f} pages/s)")
    console.print(f"  RAM delta: {bp_rss:.1f} MB")
    if pm_time:
        console.print(f"  PyMuPDF: {pm_time:.3f}s (ratio {bp_time / pm_time:.3f}x)")
    console.print("  Tip: build release with `cargo build --release`; optional RUSTFLAGS='-C target-cpu=native'")


@app.command()
def compare(
    directory: Path = typer.Argument(..., help="Directory with PDFs to compare."),
    pattern: Optional[str] = typer.Option(None, "--pattern", "-p"),
):
    """Quick table: BrainPipe vs PyMuPDF text length and time per file."""
    import time
    if not directory.is_dir():
        console.print("[red]Not a directory.[/red]")
        raise typer.Exit(1)
    pat = re.compile(pattern) if pattern else None
    rows = []
    for path in sorted(directory.rglob("*.pdf")):
        if pat and not pat.search(path.name):
            continue
        t0 = time.perf_counter()
        bp_len = 0
        pages = list(
            brainpipe.ingest(
                str(path.parent),
                pattern=f"^{re.escape(path.name)}$",
                use_cache=False,
                strategy="fast",
                chunk_pages=False,
            )
        )
        bp_time = time.perf_counter() - t0
        bp_len = sum(len(p.content) for p in pages)
        pm_len, pm_time = 0, None
        try:
            import fitz
            t1 = time.perf_counter()
            doc = fitz.open(path)
            pm_len = sum(len(page.get_text() or "") for page in doc)
            doc.close()
            pm_time = time.perf_counter() - t1
        except ImportError:
            pm_time = None
        rows.append((path.name, bp_len, bp_time, pm_len, pm_time))

    console.print(f"[bold]Compare[/bold] — {directory}\n")
    console.print(f"{'file':<28} {'BP chars':>10} {'BP s':>8} {'PM chars':>10} {'PM s':>8}")
    for name, bl, bt, pl, pt in rows:
        pts = f"{pt:.3f}" if pt is not None else "n/a"
        console.print(f"{name:<28} {bl:>10} {bt:>8.3f} {pl:>10} {pts:>8}")


@app.command("sanitize")
def sanitize_cmd(
    path: Path = typer.Argument(..., help="File or directory to sanitize."),
    mode: str = typer.Option("redact", "--mode", "-m", help="off | redact | mask | tag"),
    output: Optional[Path] = typer.Option(None, "--output", "-o", help="Write sanitized text here."),
):
    """PII detection/redaction in Rust (no Presidio). Beats Unstructured+Presidio 2-step pipelines on latency."""
    if not hasattr(brainpipe, "sanitize_pii"):
        console.print("[red]Rebuild extension: maturin develop --release[/red]")
        raise typer.Exit(1)
    texts = []
    if path.is_file():
        raw = path.read_text(encoding="utf-8", errors="replace")
        out, n, types = brainpipe.sanitize_pii(raw, mode)
        texts.append((str(path), out, n, types))
    elif path.is_dir():
        for fp in path.rglob("*"):
            if fp.is_file() and fp.suffix.lower() in SUPPORTED_EXTS:
                raw = fp.read_text(encoding="utf-8", errors="replace")
                out, n, types = brainpipe.sanitize_pii(raw, mode)
                texts.append((str(fp), out, n, types))
    else:
        console.print(f"[red]Not found: {path}[/red]")
        raise typer.Exit(1)
    total = sum(t[2] for t in texts)
    console.print(f"[green]Sanitized {len(texts)} file(s), {total} PII span(s)[/green]")
    if output:
        output.write_text("\n\n".join(t[1] for t in texts), encoding="utf-8")
        console.print(f"Wrote {output}")
    else:
        for fp, out, n, types in texts[:5]:
            console.print(f"  {fp}: {n} entities ({', '.join(types)})")


@app.command()
def version():
    """
    Print the version of BrainPipe.
    """
    console.print("[bold]BrainPipe[/bold] CLI v0.1.0")

@app.command()
def serve(
    host: str = typer.Option("0.0.0.0", "--host", "-h", help="The interface to bind to."),
    port: int = typer.Option(8000, "--port", "-p", help="The port to bind to."),
    reload: bool = typer.Option(False, "--reload", help="Enable auto-reload for development."),
):
    """
    Start the FastAPI web server for BrainPipe.
    """
    import uvicorn
    console.print(f"[bold green]Starting BrainPipe FastAPI server[/bold green] on {host}:{port}...")
    uvicorn.run("brainpipe.api:app", host=host, port=port, reload=reload)

if __name__ == "__main__":
    app()
