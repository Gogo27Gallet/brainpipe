"""
Export compatible Unstructured — éléments typés à partir de DocumentPage BrainPipe.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, Optional
import os
import re

import brainpipe


@dataclass
class ElementMetadata:
    filename: str = ""
    page_number: int = 0
    file_directory: str = ""
    extra: Dict[str, Any] = field(default_factory=dict)


@dataclass
class Element:
    text: str
    category: str
    metadata: ElementMetadata = field(default_factory=ElementMetadata)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "type": self.category,
            "text": self.text,
            "metadata": {
                "filename": self.metadata.filename,
                "page_number": self.metadata.page_number,
                "file_directory": self.metadata.file_directory,
                **self.metadata.extra,
            },
        }


@dataclass
class Title(Element):
    category: str = "Title"


@dataclass
class NarrativeText(Element):
    category: str = "NarrativeText"


@dataclass
class Table(Element):
    category: str = "Table"


@dataclass
class ListItem(Element):
    category: str = "ListItem"


@dataclass
class Header(Element):
    category: str = "Header"


@dataclass
class Footer(Element):
    category: str = "Footer"


@dataclass
class FigureCaption(Element):
    category: str = "FigureCaption"


@dataclass
class Unchecked(Element):
    category: str = "UncategorizedText"


def _classify_paragraph(text: str) -> str:
    t = text.strip()
    if not t:
        return "NarrativeText"
    if t.startswith("|") and "|" in t[1:]:
        return "Table"
    if re.match(r"^#{1,6}\s", t) or (len(t) < 120 and t.isupper()):
        return "Title"
    if re.match(r"^[-*•]\s", t) or re.match(r"^\d+\.\s", t):
        return "ListItem"
    if len(t) < 80 and not t.endswith("."):
        return "Header"
    return "NarrativeText"


def _warnings_to_category(warnings: list) -> Optional[str]:
    w = " ".join(str(x) for x in (warnings or [])).lower()
    if "page_recovered" in w or "ocr" in w:
        return "FigureCaption"
    if "encrypted" in w or "pdf_open_failed" in w:
        return "UncategorizedText"
    if "garbled" in w:
        return "UncategorizedText"
    if "hybrid_ocr" in w:
        return "NarrativeText"
    if "legacy_office" in w or "msg_format" in w:
        return "UncategorizedText"
    return None


def _make_element(text: str, category: str, meta: ElementMetadata) -> Element:
    if category == "Title":
        return Title(text=text, metadata=meta)
    if category == "Table":
        return Table(text=text, metadata=meta)
    if category == "ListItem":
        return ListItem(text=text, metadata=meta)
    if category == "Header":
        return Header(text=text, metadata=meta)
    if category == "Footer":
        return Footer(text=text, metadata=meta)
    if category == "FigureCaption":
        return FigureCaption(text=text, metadata=meta)
    if category == "UncategorizedText":
        return Unchecked(text=text, metadata=meta)
    return NarrativeText(text=text, metadata=meta)


def document_pages_to_elements(
    pages: List[Any],
    split_paragraphs: bool = True,
) -> List[Element]:
    """Convertit des DocumentPage BrainPipe en éléments style Unstructured."""
    elements: List[Element] = []
    for doc in pages:
        extra = dict(doc.metadata)
        extra["extraction_confidence"] = getattr(doc, "extraction_confidence", None)
        extra["ocr_used"] = getattr(doc, "ocr_used", False)
        extra["text_density"] = getattr(doc, "text_density", None)
        if getattr(doc, "error", None):
            extra["page_error"] = doc.error
        base_meta = ElementMetadata(
            filename=os.path.basename(doc.path),
            page_number=doc.page_index + 1,
            file_directory=os.path.dirname(doc.path) or ".",
            extra=extra,
        )
        warn_cat = _warnings_to_category(getattr(doc, "warnings", []))
        text = doc.content or ""
        if not split_paragraphs:
            cat = warn_cat or _classify_paragraph(text)
            elements.append(_make_element(text, cat, base_meta))
            continue
        for para in re.split(r"\n\s*\n", text):
            para = para.strip()
            if not para:
                continue
            cat = warn_cat or _classify_paragraph(para)
            elements.append(_make_element(para, cat, base_meta))
    return elements


def partition(
    directory: str,
    strategy: str = "fast",
    pattern: Optional[str] = None,
    **ingest_kwargs,
) -> List[Element]:
    """Partitionne un répertoire (API proche unstructured.partition)."""
    stream = brainpipe.ingest(
        directory=directory,
        pattern=pattern,
        strategy=strategy,
        use_cache=ingest_kwargs.pop("use_cache", False),
        **ingest_kwargs,
    )
    pages = list(stream)
    return document_pages_to_elements(pages)


def partition_pdf(filename: str, **kwargs) -> List[Element]:
    directory = os.path.dirname(os.path.abspath(filename)) or "."
    base = os.path.basename(filename)
    stream = brainpipe.ingest(
        directory=directory,
        pattern=f"^{re.escape(base)}$",
        strategy=kwargs.pop("strategy", "fast"),
        use_cache=kwargs.pop("use_cache", False),
        **kwargs,
    )
    return document_pages_to_elements(list(stream))
