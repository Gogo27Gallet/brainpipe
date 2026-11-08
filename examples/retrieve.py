"""Local retrieval example with citations; no API key or model provider required."""
import argparse
import re

from brainpipe import IngestConfig, ingest_isolated


def retrieve(directory, question):
    terms = set(re.findall(r"\w+", question.lower()))
    matches = []
    with ingest_isolated(directory, IngestConfig(chunk_size=600, chunk_overlap=60)) as pages:
        for page in pages:
            if page.error:
                print(f"Skipped {page.source}: {page.error.code}")
                continue
            for chunk in page.chunks:
                score = len(terms & set(re.findall(r"\w+", chunk.text.lower())))
                if score:
                    matches.append((score, page.source, chunk))
    for score, source, chunk in sorted(matches, key=lambda item: item[0], reverse=True)[:5]:
        print(f"[{source}, page {chunk.page_index + 1}, chars {chunk.start_char}:{chunk.end_char}, id {chunk.id[:12]}]")
        print(chunk.text + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory")
    parser.add_argument("question")
    args = parser.parse_args()
    retrieve(args.directory, args.question)
