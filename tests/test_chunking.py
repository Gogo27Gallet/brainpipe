from brainpipe import IngestConfig
from brainpipe.chunking import chunk_text


def test_structure_and_exact_unicode_spans():
    text = (
        "# Guide\n\n## Setup\n\nBonjour 世界 🧠 " * 15
        + "\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```python\nprint('hello')\n```\n"
    )
    chunks = chunk_text(text, "source", 0, IngestConfig(chunk_size=20, chunk_overlap=3))
    assert {c.kind for c in chunks} >= {"heading", "paragraph", "table", "code"}
    assert all(text[c.start_char : c.end_char] == c.text for c in chunks)
    assert all(c.section for c in chunks)
    assert all(len(c.text) <= 20 for c in chunks if c.kind not in {"code", "table"})
    assert any(c.oversized for c in chunks if c.kind == "table")


def test_overlap_and_stable_ids():
    text = "abcdefghijklmnop" * 20
    config = IngestConfig(chunk_size=40, chunk_overlap=8)
    chunks = chunk_text(text, "source", 3, config)
    assert chunks[1].start_char == chunks[0].end_char - 8
    assert chunks == chunk_text(text, "source", 3, config)
    assert chunks[0].id != chunk_text(text, "source", 4, config)[0].id


def test_token_budget_with_multilingual_text():
    import pytest

    pytest.importorskip("tiktoken")
    config = IngestConfig(chunk_unit="tokens", chunk_size=20, chunk_overlap=3)
    text = "Bonjour le monde. こんにちは世界。 مرحبا بالعالم. 🧠 " * 20
    chunks = chunk_text(text, "source", 0, config)
    assert len(chunks) > 1
    assert all(c.token_count <= 20 for c in chunks)
    assert all(text[c.start_char : c.end_char] == c.text for c in chunks)
    covered = set()
    for chunk in chunks:
        covered.update(range(chunk.start_char, chunk.end_char))
    assert all(i in covered for i, char in enumerate(text) if not char.isspace())


def test_hierarchical_chunks_keep_absolute_offsets_and_parent_ids():
    from brainpipe.chunking import child_chunks

    text = "Some paragraph about extraction and references. " * 10
    config = IngestConfig(chunk_size=120, chunk_overlap=10, child_chunk_size=40)
    parents = chunk_text(text, "source", 0, config)
    children = child_chunks(text, parents, config)
    assert children and all(c.parent_id in {p.id for p in parents} for c in children)
    assert all(text[c.start_char : c.end_char] == c.text for c in children)
    assert all(c.id != c.parent_id for c in children)
