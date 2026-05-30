import os
import re
from typing import Iterator, List, Optional
import brainpipe

try:
    from langchain_core.documents import Document as LCDocument
    from langchain_core.document_loaders import BaseLoader
except ImportError:
    # Fallback if langchain is not installed
    class BaseLoader:
        pass
    class LCDocument:
        pass

class PyPDFLoader(BaseLoader):
    """
    Drop-in replacement for langchain_community.document_loaders.PyPDFLoader
    that uses BrainPipe under the hood. 100x faster, zero friction.
    """
    def __init__(
        self,
        file_path: str,
        chunk_size: Optional[int] = None,
        use_cache: bool = True,
        use_ocr: bool = False,
        use_embeddings: bool = False,
        use_vlm: bool = False,
        **kwargs
    ) -> None:
        if BaseLoader is object or not hasattr(BaseLoader, "__init__"):
            raise ImportError(
                "langchain-core or langchain-community is required to use brainpipe.integrations.langchain. "
                "Please install it via `pip install langchain`."
            )
        self.file_path = file_path
        self.chunk_size = chunk_size
        self.use_cache = use_cache
        self.use_ocr = use_ocr
        self.use_embeddings = use_embeddings
        self.use_vlm = use_vlm

    def lazy_load(self) -> Iterator[LCDocument]:
        abs_path = os.path.abspath(self.file_path)
        directory = os.path.dirname(abs_path)
        filename = os.path.basename(abs_path)
        
        # Ingest using brainpipe
        stream = brainpipe.ingest(
            directory=directory,
            chunk_size=self.chunk_size,
            use_cache=self.use_cache,
            use_ocr=self.use_ocr,
            pattern=f"^{re.escape(filename)}$",
            use_embeddings=self.use_embeddings,
            use_vlm=self.use_vlm
        )
        pages = list(stream)
        if not pages:
            return
        doc = pages[0]
        for i, chunk_text in enumerate(doc.chunks):
            metadata = doc.metadata.copy()
            metadata["source"] = self.file_path
            metadata["chunk_index"] = i
            metadata.update(doc.form_fields)
            yield LCDocument(page_content=chunk_text, metadata=metadata)

    def load(self) -> List[LCDocument]:
        return list(self.lazy_load())


class BrainPipeLoader(BaseLoader):
    """
    Highly optimized document loader that uses BrainPipe for ultra-fast multi-format ingestion.
    Supports directories, PDFs, text, Office docs, caching, and semantic chunking.
    """
    def __init__(
        self,
        path: str,
        chunk_size: Optional[int] = None,
        child_chunk_size: Optional[int] = None,
        use_cache: bool = True,
        use_ocr: bool = False,
        pattern: Optional[str] = None,
        use_embeddings: bool = False,
        use_vlm: bool = False
    ) -> None:
        if BaseLoader is object:
            raise ImportError("langchain is required to use BrainPipeLoader.")
        self.path = path
        self.chunk_size = chunk_size
        self.child_chunk_size = child_chunk_size
        self.use_cache = use_cache
        self.use_ocr = use_ocr
        self.pattern = pattern
        self.use_embeddings = use_embeddings
        self.use_vlm = use_vlm

    def lazy_load(self) -> Iterator[LCDocument]:
        abs_path = os.path.abspath(self.path)
        if os.path.isdir(abs_path):
            directory = abs_path
            pattern = self.pattern
        else:
            directory = os.path.dirname(abs_path)
            pattern = f"^{re.escape(os.path.basename(abs_path))}$"

        stream = brainpipe.ingest(
            directory=directory,
            chunk_size=self.chunk_size,
            child_chunk_size=self.child_chunk_size,
            use_cache=self.use_cache,
            use_ocr=self.use_ocr,
            pattern=pattern,
            use_embeddings=self.use_embeddings,
            use_vlm=self.use_vlm
        )

        for doc in list(stream):
            for i, chunk_text in enumerate(doc.chunks):
                metadata = doc.metadata.copy()
                metadata["source"] = doc.path
                metadata["chunk_index"] = i
                metadata.update(doc.form_fields)
                yield LCDocument(page_content=chunk_text, metadata=metadata)

    def load(self) -> List[LCDocument]:
        return list(self.lazy_load())
