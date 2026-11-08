import zipfile

import pytest
import brainpipe


@pytest.mark.parametrize("suffix", ["docx", "xlsx", "pptx", "odt", "epub"])
def test_office_and_epub_entry_limits(tmp_path, suffix):
    with zipfile.ZipFile(tmp_path / f"malformed.{suffix}", "w") as archive:
        for i in range(1001):
            archive.writestr(f"{i}.xml", "<root/>")
    pages = list(brainpipe.ingest(str(tmp_path), use_cache=False))
    assert pages and pages[0].error and "entry count" in pages[0].error


def test_bad_zip_crc_is_reported(tmp_path):
    path = tmp_path / "a.docx"
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.writestr("word/document.xml", "<w:t>safe</w:t>")
    data = path.read_bytes().replace(b"safe", b"oops")
    path.write_bytes(data)
    pages = list(brainpipe.ingest(str(tmp_path), use_cache=False))
    assert pages[0].error is not None
