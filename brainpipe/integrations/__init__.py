# BrainPipe Integrations for LangChain, LlamaIndex, and Unstructured-style export
from .langchain import PyPDFLoader, BrainPipeLoader
from .llamaindex import SimpleDirectoryReader

try:
    from .unstructured import partition, partition_pdf, Element, NarrativeText, Title, Table
except ImportError:
    partition = None  # type: ignore

__all__ = [
    "PyPDFLoader",
    "BrainPipeLoader",
    "SimpleDirectoryReader",
    "partition",
    "partition_pdf",
    "Element",
    "NarrativeText",
    "Title",
    "Table",
]
