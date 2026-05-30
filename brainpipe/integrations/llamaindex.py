import os
import re
from typing import List, Optional
import brainpipe

try:
    from llama_index.core.readers.base import BaseReader
    from llama_index.core.schema import Document as LIDocument
except ImportError:
    # Fallback if llama-index is not installed
    class BaseReader:
        pass
    class LIDocument:
        pass

class SimpleDirectoryReader(BaseReader):
    """
    Drop-in replacement for llama_index.core.SimpleDirectoryReader
    that leverages BrainPipe for lightning-fast multi-threaded document ingestion.
    """
    def __init__(
        self,
        input_dir: Optional[str] = None,
        input_files: Optional[List[str]] = None,
        chunk_size: Optional[int] = None,
        child_chunk_size: Optional[int] = None,
        use_cache: bool = True,
        use_ocr: bool = False,
        required_exts: Optional[List[str]] = None,
        use_embeddings: bool = False,
        use_vlm: bool = False,
        **kwargs
    ) -> None:
        if BaseReader is object or not hasattr(BaseReader, "__init__"):
            raise ImportError(
                "llama-index-core is required to use brainpipe.integrations.llamaindex. "
                "Please install it via `pip install llama-index`."
            )
        self.input_dir = input_dir
        self.input_files = input_files
        self.chunk_size = chunk_size
        self.child_chunk_size = child_chunk_size
        self.use_cache = use_cache
        self.use_ocr = use_ocr
        self.required_exts = required_exts
        self.use_embeddings = use_embeddings
        self.use_vlm = use_vlm

    def load_data(self) -> List[LIDocument]:
        results = []
        
        # If specific files are loaded
        if self.input_files:
            for filepath in self.input_files:
                abs_path = os.path.abspath(filepath)
                directory = os.path.dirname(abs_path)
                filename = os.path.basename(abs_path)
                
                stream = brainpipe.ingest(
                    directory=directory,
                    chunk_size=self.chunk_size,
                    child_chunk_size=self.child_chunk_size,
                    use_cache=self.use_cache,
                    use_ocr=self.use_ocr,
                    pattern=f"^{re.escape(filename)}$",
                    use_embeddings=self.use_embeddings,
                    use_vlm=self.use_vlm
                )
                results.extend(self._map_docs(list(stream)))
                
        # If directory is loaded
        elif self.input_dir:
            pattern = None
            if self.required_exts:
                exts_pattern = "|".join([ext.lstrip(".") for ext in self.required_exts])
                pattern = f"\\.({exts_pattern})$"
                
            stream = brainpipe.ingest(
                directory=self.input_dir,
                chunk_size=self.chunk_size,
                child_chunk_size=self.child_chunk_size,
                use_cache=self.use_cache,
                use_ocr=self.use_ocr,
                pattern=pattern,
                use_embeddings=self.use_embeddings,
                use_vlm=self.use_vlm
            )
            results.extend(self._map_docs(list(stream)))
            
        return results

    def _map_docs(self, brainpipe_docs) -> List[LIDocument]:
        li_docs = []
        for doc in brainpipe_docs:
            metadata = doc.metadata.copy()
            metadata["file_path"] = doc.path
            metadata["file_name"] = os.path.basename(doc.path)
            metadata.update(doc.form_fields)

            for i, chunk_text in enumerate(doc.chunks):
                chunk_metadata = metadata.copy()
                chunk_metadata["chunk_index"] = i
                
                li_doc = LIDocument(
                    text=chunk_text,
                    metadata=chunk_metadata,
                    id_=f"{doc.path}_chunk_{i}"
                )
                li_docs.append(li_doc)
        return li_docs
