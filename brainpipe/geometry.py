"""Geometric native-PDF reading order and exact text-to-page provenance."""

from dataclasses import replace
import hashlib
import json
import statistics
import unicodedata

from .records import TextBlock, TextSpan


def envelope(boxes):
    boxes = list(boxes)
    return (
        (
            min(b[0] for b in boxes),
            min(b[1] for b in boxes),
            max(b[2] for b in boxes),
            max(b[3] for b in boxes),
        )
        if boxes
        else None
    )


def _split_segments(glyphs, page_width):
    """Group with loose font boxes; cite actual tight character boxes."""
    rows = []
    ordered = sorted(
        glyphs,
        key=lambda g: (
            (g["layout_bbox"][1] + g["layout_bbox"][3]) / 2,
            g["layout_bbox"][0],
        ),
    )
    for glyph in ordered:
        box = glyph["layout_bbox"]
        cy = (box[1] + box[3]) / 2
        height = max(box[3] - box[1], glyph["font_size"], 1)
        row = None
        # Only nearby rows can overlap; no all-pairs character comparisons.
        for candidate in reversed(rows[-4:]):
            if abs(cy - candidate["cy"]) <= max(
                1.5, min(height, candidate["height"]) * 0.35
            ):
                row = candidate
                break
        if row is None:
            row = {"cy": cy, "height": height, "glyphs": []}
            rows.append(row)
        row["glyphs"].append(glyph)
    segments = []
    for row in rows:
        glyphs = sorted(row["glyphs"], key=lambda g: g["layout_bbox"][0])
        font = statistics.median(g["font_size"] for g in glyphs)
        threshold = max(font * 1.8, page_width * 0.015, 8)
        current, right = [], None
        for glyph in glyphs:
            if right is not None and glyph["layout_bbox"][0] - right > threshold:
                segments.append(current)
                current = []
            current.append(glyph)
            right = glyph["layout_bbox"][2]
        if current:
            segments.append(current)
    return [s for s in segments if any(g["text"].strip() for g in s)]


def _gap(boxes, axis, minimum):
    intervals = sorted((b[axis], b[axis + 2]) for b in boxes)
    merged = []
    for left, right in intervals:
        if merged and left <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(right, merged[-1][1]))
        else:
            merged.append((left, right))
    gaps = [(b[0] - a[1], (a[1] + b[0]) / 2) for a, b in zip(merged, merged[1:])]
    if not gaps:
        return None
    size, cut = max(gaps)
    return cut if size >= minimum else None


def reading_order(segments, page_width):
    """Recursive XY-cut; spanning headers/footers naturally form horizontal bands."""
    if len(segments) <= 1:
        return segments
    boxes = [envelope(g["layout_bbox"] for g in s) for s in segments]
    height = max(b[3] for b in boxes) - min(b[1] for b in boxes)
    # Large horizontal whitespace separates spanning titles/footers before
    # ordinary column cuts. Small inter-line gaps must not interleave columns.
    for axis, minimum in (
        (1, max(30, height * 0.12)),
        (0, max(8, page_width * 0.02)),
        (1, 4),
    ):
        cut = _gap(boxes, axis, minimum)
        if cut is not None:
            first, second = [], []
            for segment, box in zip(segments, boxes):
                (first if (box[axis] + box[axis + 2]) / 2 < cut else second).append(
                    segment
                )
            if first and second:
                return reading_order(first, page_width) + reading_order(
                    second, page_width
                )
    return [
        s
        for _, s in sorted(
            zip(boxes, segments), key=lambda pair: (pair[0][1], pair[0][0])
        )
    ]


def build_page(raw, source_id):
    glyphs = raw["glyphs"]
    segments = reading_order(
        _split_segments(glyphs, max(raw["width"], raw["height"])),
        max(raw["width"], raw["height"]),
    )
    content, blocks, spans = "", [], []
    font_sizes = [g["font_size"] for g in glyphs if g["text"].strip()]
    body_size = statistics.median(font_sizes) if font_sizes else 0
    warnings = []
    previous_layout = None
    previous_kind = None
    for segment in segments:
        layout_box = envelope(g["layout_bbox"] for g in segment)
        font = statistics.median(g["font_size"] for g in segment)
        kind = "heading" if body_size and font >= body_size * 1.3 else "paragraph"
        continued = (
            previous_layout is not None
            and kind == previous_kind == "paragraph"
            and 0 < layout_box[1] - previous_layout[1] <= max(font * 1.8, 4)
            and abs(layout_box[0] - previous_layout[0]) < max(font * 1.2, 4)
        )
        if content:
            content += "\n" if continued else "\n\n"
        block_start = len(content)
        text, word_start, word_boxes = "", None, []
        local_spans = []
        previous_right = None
        for glyph in segment:
            value = glyph["text"]
            left = glyph["layout_bbox"][0]
            if (
                previous_right is not None
                and left - previous_right > max(1, glyph["font_size"] * 0.18)
                and text
                and not text[-1].isspace()
            ):
                value = " " + value
            for character in value:
                if character.isspace():
                    if word_start is not None:
                        local_spans.append(
                            (word_start, len(text), envelope(word_boxes))
                        )
                        word_start, word_boxes = None, []
                    if text and not text[-1].isspace():
                        text += " "
                else:
                    if word_start is None:
                        word_start = len(text)
                    text += character
                    word_boxes.append(glyph["bbox"])
            previous_right = glyph["layout_bbox"][2]
        if word_start is not None:
            local_spans.append((word_start, len(text), envelope(word_boxes)))
        text = text.rstrip()
        if not text:
            continue
        content += text
        identity = hashlib.sha256(
            f"{source_id}:{raw['page_index']}:{block_start}:{text}".encode()
        ).hexdigest()
        box = envelope(g["bbox"] for g in segment)
        if continued and blocks:
            blocks[-1] = replace(
                blocks[-1], end_char=len(content), bbox=envelope((blocks[-1].bbox, box))
            )
        else:
            blocks.append(TextBlock(identity, kind, block_start, len(content), box))
        previous_layout, previous_kind = layout_box, kind
        for start, end, bbox in local_spans:
            spans.append(
                TextSpan(block_start + start, block_start + min(end, len(text)), bbox)
            )
    if any(unicodedata.bidirectional(c) in {"R", "AL"} for c in content):
        warnings.append("rtl_layout_unvalidated")
    return {
        "page_index": raw["page_index"],
        "content": content,
        "blocks": [b.to_dict() for b in blocks],
        "source_spans": [s.to_dict() for s in spans],
        "page_width": raw["width"],
        "page_height": raw["height"],
        "metadata": {
            "source_type": "pdf",
            "geometry_units": "pt",
            "geometry_origin": "top_left",
            "reading_order": "xy_cut",
            "rotation": raw["rotation"],
        },
        "warnings": warnings,
        "extraction_confidence": 0.75 if content else 0,
        "ocr_used": False,
    }


def pdf_pages(path, source_id, config):
    from . import brainpipe as native

    stream = native.pdf_geometry(str(path), config.max_pages, config.pdf_password)
    try:
        for encoded in stream:
            page = build_page(json.loads(encoded), source_id)
            if config.pii_mode != "off":
                clean, _, _ = native.sanitize_pii(page["content"], config.pii_mode)
                if clean != page["content"]:
                    # Avoid publishing original PII in geometry or stale character offsets.
                    page["content"], page["blocks"], page["source_spans"] = (
                        clean,
                        [],
                        [],
                    )
                    page["warnings"].append("geometry_removed_after_pii_redaction")
            yield page
    finally:
        stream.close()


def locate_chunks(chunks, spans, blocks=(), content=""):
    located = []
    headings = [b for b in blocks if b.kind == "heading"]
    for chunk in chunks:
        regions = tuple(
            s.bbox
            for s in spans
            if s.start_char < chunk.end_char and s.end_char > chunk.start_char
        )
        active = [b for b in headings if b.start_char <= chunk.start_char]
        section = (
            (content[active[-1].start_char : active[-1].end_char],)
            if active
            else chunk.section
        )
        containing = [
            b
            for b in blocks
            if b.start_char <= chunk.start_char and b.end_char >= chunk.end_char
        ]
        kind = (
            containing[0].kind
            if containing and containing[0].kind == "heading"
            else chunk.kind
        )
        located.append(
            replace(
                chunk,
                bbox=envelope(regions),
                regions=regions,
                section=section,
                kind=kind,
            )
        )
    return located
