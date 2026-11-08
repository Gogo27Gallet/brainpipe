"""Generate deterministic native-text PDFs for reproducible throughput measurements."""

import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--files", type=int, default=8)
    parser.add_argument("--pages", type=int, default=25)
    args = parser.parse_args()
    if not 1 <= args.files <= 1000 or not 1 <= args.pages <= 1000:
        parser.error("files/pages must be between 1 and 1000")
    args.directory.mkdir(parents=True, exist_ok=True)
    if any(args.directory.iterdir()):
        parser.error("use an empty output directory")
    from reportlab.pdfgen import canvas
    from reportlab.lib.pagesizes import A4

    for document in range(args.files):
        pdf = canvas.Canvas(
            str(args.directory / f"document-{document:03}.pdf"),
            pagesize=A4,
            invariant=1,
            pageCompression=1,
        )
        for page in range(args.pages):
            pdf.setFont("Helvetica", 9)
            for line in range(35):
                text = f"Document {document:03} page {page:03} line {line:02}: reproducible local ingestion with traceable sources."
                pdf.drawString(35, 800 - line * 20, text)
            pdf.showPage()
        pdf.save()
    print(
        json.dumps(
            {
                "files": args.files,
                "pages_per_file": args.pages,
                "total_pages": args.files * args.pages,
                "invariant_pdf_metadata": True,
            }
        )
    )


if __name__ == "__main__":
    main()
