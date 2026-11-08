"""Opt-in, content-addressed extraction cache. Contains document text."""

import hashlib
import json
import os
from pathlib import Path
import tempfile

CACHE_VERSION = 2


def cache_key(content_hash: str, suffix: str, config) -> str:
    # Limits participate as well: a permissive prior run cannot bypass stricter settings.
    options = config.to_dict()
    for name in ("workers", "queue_size", "cache_dir"):
        options.pop(name)
    from importlib.metadata import version

    payload = {
        "cache_version": CACHE_VERSION,
        "engine": version("brainpipe"),
        "content": content_hash,
        "suffix": suffix.lower(),
        "options": options,
    }
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def load(directory, key, limit):
    if directory is None:
        return None
    path = Path(directory) / (key + ".json")
    try:
        if path.stat().st_size > limit:
            return None
        data = json.loads(path.read_bytes())
        if data["version"] != CACHE_VERSION or not isinstance(data["pages"], list):
            return None
        for page in data["pages"]:
            if (
                not isinstance(page, dict)
                or not isinstance(page.get("content"), str)
                or type(page.get("page_index")) is not int
                or not isinstance(page.get("metadata", {}), dict)
                or not isinstance(page.get("warnings", []), list)
            ):
                return None
        return data["pages"]
    except (OSError, ValueError, KeyError, TypeError):
        return None


def save(directory, key, pages, limit):
    if directory is None or any(p.get("error") for p in pages):
        return
    payload = json.dumps(
        {"version": CACHE_VERSION, "pages": pages}, ensure_ascii=False
    ).encode()
    if len(payload) > limit:
        return
    folder = Path(directory)
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    handle, filename = tempfile.mkstemp(prefix=".writing-", dir=folder)
    try:
        with os.fdopen(handle, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
        os.replace(filename, folder / (key + ".json"))
    finally:
        Path(filename).unlink(missing_ok=True)
