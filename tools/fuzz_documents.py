"""Deterministic mutation smoke fuzzing behind resource-limited process workers."""

import argparse
import hashlib
import json
from pathlib import Path
import random
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--rounds", type=int, default=40)
    parser.add_argument(
        "--pdf-layout", choices=["native", "reading_order"], default="native"
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not 1 <= args.rounds <= 10_000:
        parser.error("rounds must be 1..10000")
    from brainpipe import IngestConfig, ingest_isolated

    randomizer = random.Random(args.seed)
    seeds = [
        ("pdf", b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF"),
        ("docx", b"PK\x03\x04"),
        ("png", b"\x89PNG\r\n\x1a\n"),
        ("xml", b"<root><item>hello</item></root>"),
        ("txt", b"hello world"),
    ]
    with tempfile.TemporaryDirectory(prefix="brainpipe_fuzz_") as directory:
        for index in range(args.rounds):
            suffix, seed = seeds[index % len(seeds)]
            payload = bytearray(seed)
            for _ in range(randomizer.randrange(1, 10)):
                if randomizer.random() < 0.5 and payload:
                    payload[randomizer.randrange(len(payload))] = randomizer.randrange(
                        256
                    )
                else:
                    position = randomizer.randrange(len(payload) + 1)
                    payload[position:position] = randomizer.randbytes(
                        randomizer.randrange(1, 64)
                    )
            Path(directory, f"case-{index}.{suffix}").write_bytes(payload)
        with ingest_isolated(
            directory,
            IngestConfig(
                pdf_layout=args.pdf_layout,
                timeout_seconds=3,
                cpu_seconds=2,
                memory_mb=2048,
                max_output_bytes=1024 * 1024,
                max_file_bytes=1024 * 1024,
            ),
        ) as stream:
            pages = list(stream)
        findings = [
            p.to_dict()
            for p in pages
            if p.error and p.error.code in {"worker_crashed", "timeout"}
        ]
        completed = {p.source for p in pages}
        report = {
            "seed": args.seed,
            "pdf_layout": args.pdf_layout,
            "rounds": args.rounds,
            "files_reported": len(completed),
            "resource_limited_files": len(
                {
                    p.source
                    for p in pages
                    if p.error and p.error.code == "resource_limit"
                }
            ),
            "findings": [
                {
                    "source": p["source"],
                    "error": p["error"],
                    "input_sha256": hashlib.sha256(
                        Path(directory, p["source"]).read_bytes()
                    ).hexdigest(),
                }
                for p in findings
            ],
            "passed": not findings and len(completed) == args.rounds,
            "scope": "Small deterministic mutation corpus; not exhaustive parser fuzzing.",
        }
    payload = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(payload + "\n")
    else:
        print(payload)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
