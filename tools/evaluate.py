"""Synthetic extraction/traceability evaluation; not an OCR quality benchmark."""

import argparse
import json
from pathlib import Path
import tempfile


def generate_corpus(root):
    from reportlab.pdfgen import canvas

    root.mkdir(parents=True, exist_ok=True)
    fixtures = {
        "guide.md": "# Installation\n\nInstall BrainPipe locally.\n\n## Limits\n\nDocument workers enforce resource limits.\n",
        "table.md": "# Revenue\n\n| Quarter | Revenue |\n|---|---|\n| Q1 | 120 |\n| Q2 | 180 |\n",
        "code.md": "# Example\n\n```python\nprint('traceable chunks')\n```\n\nBonjour 世界 🧠\n",
    }
    expected = {}
    for name, text in fixtures.items():
        (root / name).write_text(text, encoding="utf-8")
        expected[name] = {"pages": 1, "phrases": [text.splitlines()[0][2:]]}
    pdf = canvas.Canvas(str(root / "multipage.pdf"))
    for i in range(3):
        pdf.drawString(40, 750, f"BrainPipe evaluation page {i}.")
        pdf.drawString(40, 720, "This is a reproducible synthetic document.")
        pdf.showPage()
    pdf.save()
    expected["multipage.pdf"] = {
        "pages": 3,
        "phrases": [f"evaluation page {i}" for i in range(3)],
    }
    pdf = canvas.Canvas(str(root / "columns.pdf"))
    pdf.drawString(40, 750, "LEFT COLUMN")
    pdf.drawString(320, 750, "RIGHT COLUMN")
    pdf.showPage()
    pdf.save()
    expected["columns.pdf"] = {"pages": 1, "phrases": ["LEFT COLUMN", "RIGHT COLUMN"]}
    return expected


def evaluate(root, expected):
    from brainpipe import IngestConfig, ingest_isolated

    with ingest_isolated(root, IngestConfig(chunk_size=80, chunk_overlap=8)) as stream:
        pages = list(stream)
    results = []
    for source, specification in expected.items():
        extracted = [p for p in pages if p.source == source]
        text = "\n".join(
            p.content for p in sorted(extracted, key=lambda p: p.page_index)
        )
        phrase_hits = sum(phrase in text for phrase in specification["phrases"])
        traceable = all(
            p.content[c.start_char : c.end_char] == c.text
            for p in extracted
            for c in p.chunks
        )
        covered = True
        for page in extracted:
            offsets = set()
            for chunk in page.chunks:
                offsets.update(range(chunk.start_char, chunk.end_char))
            covered &= all(
                i in offsets
                for i, char in enumerate(page.content)
                if not char.isspace()
            )
        results.append(
            {
                "source": source,
                "expected_pages": specification["pages"],
                "actual_pages": len(extracted),
                "required_phrase_recall": phrase_hits / len(specification["phrases"]),
                "exact_chunk_spans": traceable,
                "non_whitespace_coverage": covered,
                "errors": [p.error.code for p in extracted if p.error],
            }
        )
    passed = all(
        r["actual_pages"] == r["expected_pages"]
        and r["required_phrase_recall"] == 1
        and r["exact_chunk_spans"]
        and r["non_whitespace_coverage"]
        and not r["errors"]
        for r in results
    )
    return {
        "schema_version": 1,
        "passed": passed,
        "documents": results,
        "scope": "Synthetic native-text PDF/Markdown smoke evaluation. Does not establish OCR accuracy, table geometry or reading order quality.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--keep-corpus", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="brainpipe_eval_") as directory:
        root = args.keep_corpus or Path(directory)
        expected = generate_corpus(root)
        report = evaluate(root, expected)
    payload = json.dumps(report, indent=2, ensure_ascii=False)
    if args.output:
        args.output.write_text(payload + "\n")
    else:
        print(payload)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
