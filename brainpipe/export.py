"""Streaming JSON, JSONL and Markdown exports of schema-v1 page records."""

import json


def iter_export(pages, format="jsonl"):
    if format not in {"jsonl", "json", "markdown"}:
        raise ValueError("format must be jsonl, json or markdown")
    first = True
    if format == "json":
        yield "["
    for page in pages:
        record = page.to_dict() if hasattr(page, "to_dict") else page
        if format == "markdown":
            yield f"# {record['source']} — page {record['page_index'] + 1}\n\n"
            if record.get("error"):
                yield f"> Extraction error: {record['error']['code']}\n\n"
            else:
                yield record["content"] + "\n\n"
        else:
            if format == "json" and not first:
                yield ","
            yield json.dumps(record, ensure_ascii=False, allow_nan=False)
            if format == "jsonl":
                yield "\n"
        first = False
    if format == "json":
        yield "]\n"


def read_jsonl(path):
    with open(path, encoding="utf-8") as source:
        for line in source:
            if line.strip():
                yield json.loads(line)
