"""Structure-aware chunks with exact source spans and optional token budgets."""

from functools import lru_cache
import hashlib
import re

from .records import Chunk


@lru_cache(maxsize=8)
def token_counter(encoding: str):
    try:
        import tiktoken
    except ImportError as exc:
        raise ImportError(
            "Token chunking requires pip install 'brainpipe[tokens]'"
        ) from exc
    if encoding != "cl100k_base":
        raise ValueError("Only the bundled cl100k_base tokenizer is supported")
    from pathlib import Path
    from tiktoken.load import load_tiktoken_bpe

    ranks = load_tiktoken_bpe(
        str(Path(__file__).parent / "assets" / "cl100k_base.tiktoken"),
        expected_hash="223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7",
    )
    encoder = tiktoken.Encoding(
        name="cl100k_base",
        pat_str=r"'(?i:[sdmt]|ll|ve|re)|[^\r\n\p{L}\p{N}]?+\p{L}++|\p{N}{1,3}+| ?[^\s\p{L}\p{N}]++[\r\n]*+|\s++$|\s*[\r\n]|\s+(?!\S)|\s",
        mergeable_ranks=ranks,
        special_tokens={
            "<|endoftext|>": 100257,
            "<|fim_prefix|>": 100258,
            "<|fim_middle|>": 100259,
            "<|fim_suffix|>": 100260,
            "<|endofprompt|>": 100276,
        },
    )
    return lambda text: len(encoder.encode(text, disallowed_special=()))


def _blocks(text):
    lines = list(re.finditer(r".*(?:\n|$)", text))
    section = []
    i = 0
    while i < len(lines):
        line = lines[i].group()
        if not line.strip():
            i += 1
            continue
        heading = re.match(r"^(#{1,6})\s+(.+?)\s*#*\s*$", line)
        if heading:
            level = len(heading[1])
            section = section[: level - 1] + [heading[2]]
            yield lines[i].start(), lines[i].end(), "heading", tuple(section)
            i += 1
            continue
        start = i
        fence = re.match(r"^\s*(`{3,}|~{3,})", line)
        if fence:
            marker = fence[1]
            i += 1
            while i < len(lines):
                current = lines[i].group()
                i += 1
                if re.match(
                    r"^\s*" + re.escape(marker[0]) + "{" + str(len(marker)) + r",}\s*$",
                    current,
                ):
                    break
            kind = "code"
        else:
            kind = (
                "table"
                if "|" in line
                else "list"
                if re.match(r"^\s*(?:[-*+] |\d+[.)] )", line)
                else "paragraph"
            )
            i += 1
            while i < len(lines) and lines[i].group().strip():
                following = lines[i].group()
                if re.match(r"^#{1,6}\s|^\s*(?:`{3,}|~{3,})", following):
                    break
                if ("|" in following) != (kind == "table"):
                    break
                i += 1
        yield lines[start].start(), lines[i - 1].end(), kind, tuple(section)


def chunk_text(text, source_id, page_index, config):
    measure = (
        len
        if config.chunk_unit == "characters"
        else token_counter(config.token_encoding)
    )
    chunks = []

    def append(start, end, kind, section):
        while start < end and text[start].isspace():
            start += 1
        while end > start and text[end - 1].isspace():
            end -= 1
        if start == end:
            return
        part = text[start:end]
        digest = hashlib.sha256(
            f"{source_id}:{page_index}:{start}:{end}:{part}".encode()
        ).hexdigest()
        chunks.append(
            Chunk(
                digest,
                source_id,
                page_index,
                part,
                start,
                end,
                kind,
                section,
                measure(part) if config.chunk_unit == "tokens" else None,
                measure(part) > config.chunk_size,
            )
        )

    for start, end, kind, section in _blocks(text):
        if kind in {"table", "code"} or measure(text[start:end]) <= config.chunk_size:
            append(start, end, kind, section)
            continue
        cursor = start
        while cursor < end:
            # Search on Unicode character boundaries; never decode partial tokens.
            low, high = cursor + 1, end
            best = cursor
            while low <= high:
                middle = (low + high) // 2
                if measure(text[cursor:middle]) <= config.chunk_size:
                    best, low = middle, middle + 1
                else:
                    high = middle - 1
            if best == cursor:
                # A single Unicode scalar may require multiple tokenizer tokens.
                append(cursor, cursor + 1, kind, section)
                cursor += 1
                continue
            if best < end:
                boundary = max(
                    text.rfind(" ", cursor, best), text.rfind("\n", cursor, best)
                )
                if boundary > cursor + (best - cursor) // 2:
                    best = boundary
            append(cursor, best, kind, section)
            if best == end:
                break
            overlap_start = best
            while (
                overlap_start > cursor
                and measure(text[overlap_start - 1 : best]) <= config.chunk_overlap
            ):
                overlap_start -= 1
            cursor = max(cursor + 1, overlap_start)
    return chunks


def child_chunks(text, parents, config):
    from dataclasses import replace

    if config.child_chunk_size is None:
        return []
    child_config = replace(
        config,
        chunk_size=config.child_chunk_size,
        chunk_overlap=min(config.chunk_overlap, config.child_chunk_size - 1),
        child_chunk_size=None,
    )
    children = []
    for parent in parents:
        for chunk in chunk_text(
            parent.text, parent.source_id, parent.page_index, child_config
        ):
            start, end = (
                parent.start_char + chunk.start_char,
                parent.start_char + chunk.end_char,
            )
            identity = hashlib.sha256(
                f"child:{parent.id}:{start}:{end}:{chunk.text}".encode()
            ).hexdigest()
            children.append(
                replace(
                    chunk,
                    id=identity,
                    parent_id=parent.id,
                    section=parent.section,
                    start_char=start,
                    end_char=end,
                )
            )
    return children
