"""Deterministic native-PDF layout evaluation with explicit reading-order truth."""

import argparse
import itertools
import json
from pathlib import Path
import random
import tempfile


def generate(root):
    from reportlab.pdfgen import canvas
    from pypdf import PdfReader, PdfWriter

    expected = {}
    for columns, rotation in itertools.product((1, 2, 3), (0, 90, 180, 270)):
        name = f"columns-{columns}-rotation-{rotation}.pdf"
        base = root / ("base-" + name)
        pdf = canvas.Canvas(str(base), pagesize=(600, 800), invariant=1)
        header = f"REFERENCE REPORT {columns}"
        footer = "REFERENCE FOOTER"
        operations = [
            ("Helvetica-Bold", 18, 40, 750, header),
            ("Helvetica", 9, 40, 35, footer),
        ]
        order = [header]
        for col in range(columns):
            for line in range(5):
                phrase = f"COLUMN {col + 1} SENTENCE {line + 1}"
                order.append(phrase)
                operations.append(
                    (
                        "Helvetica",
                        10,
                        40 + col * (520 / columns),
                        680 - line * 16,
                        phrase,
                    )
                )
        order.append(footer)
        random.Random(columns * 100 + rotation).shuffle(operations)
        for font, size, x, y, text in operations:
            pdf.setFont(font, size)
            pdf.drawString(x, y, text)
        pdf.save()
        reader = PdfReader(base)
        reader.pages[0].rotate(rotation)
        writer = PdfWriter()
        writer.add_page(reader.pages[0])
        with (root / name).open("wb") as output:
            writer.write(output)
        base.unlink()
        expected[name] = order
    return expected


def score(content, expected):
    positions = [content.find(phrase) for phrase in expected]
    hits = sum(p >= 0 for p in positions)
    pairs = list(itertools.combinations(range(len(expected)), 2))
    ordered = sum(positions[a] >= 0 and positions[b] > positions[a] for a, b in pairs)
    return {
        "phrase_recall": hits / len(expected),
        "pairwise_reading_order": ordered / len(pairs),
    }


def evaluate(root, expected):
    from brainpipe import IngestConfig, ingest_isolated

    results = {}
    for mode in ("native", "reading_order"):
        with ingest_isolated(root, IngestConfig(pdf_layout=mode)) as stream:
            pages = list(stream)
        results[mode] = []
        for page in pages:
            metrics = score(page.content, expected[page.source])
            covered = set()
            for span in page.source_spans:
                covered.update(range(span.start_char, span.end_char))
            letters = [i for i, c in enumerate(page.content) if not c.isspace()]
            metrics.update(
                source=page.source,
                error=page.error.code if page.error else None,
                geometric_text_coverage=sum(i in covered for i in letters)
                / max(len(letters), 1),
                exact_chunk_spans=all(
                    page.content[c.start_char : c.end_char] == c.text
                    for c in page.chunks
                ),
            )
            results[mode].append(metrics)
        results[mode].sort(key=lambda r: r["source"])
    passed = all(
        r["phrase_recall"]
        == r["pairwise_reading_order"]
        == r["geometric_text_coverage"]
        == 1
        and r["exact_chunk_spans"]
        and r["error"] is None
        for r in results["reading_order"]
    )
    return {
        "schema_version": 1,
        "passed": passed,
        "documents": len(expected),
        "results": results,
        "scope": "Twelve synthetic PDFs with shuffled content streams, one/two/three columns, headers, footers and four page rotations. Not a real-world OCR, RTL or table-quality study.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="brainpipe_layout_eval_") as folder:
        root = Path(folder)
        report = evaluate(root, generate(root))
    payload = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(payload + "\n")
    else:
        print(payload)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
