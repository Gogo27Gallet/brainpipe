"""Framework adapters, loaded only when requested."""

from importlib import import_module

_EXPORTS = {
    "PyPDFLoader": "langchain",
    "BrainPipeLoader": "langchain",
    "SimpleDirectoryReader": "llamaindex",
    **{
        name: "unstructured"
        for name in (
            "partition",
            "partition_pdf",
            "Element",
            "NarrativeText",
            "Title",
            "Table",
        )
    },
}
__all__ = list(_EXPORTS)


def __getattr__(name):
    module = _EXPORTS.get(name)
    if module is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    value = getattr(import_module(f".{module}", __name__), name)
    globals()[name] = value
    return value
