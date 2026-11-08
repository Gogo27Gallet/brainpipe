//! Shared Office/EPUB ZIP validation before any XML or spreadsheet parser runs.
use std::io::{Read, Seek};
use zip::ZipArchive;

pub const MAX_ENTRIES: usize = 1000;
pub const MAX_MEMBER_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

pub fn validate<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<(), String> {
    if archive.len() > MAX_ENTRIES { return Err("ZIP entry count exceeds limit".into()); }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir() { continue; }
        let allowed = MAX_MEMBER_BYTES.min(MAX_TOTAL_BYTES - total);
        if entry.size() > allowed { return Err("ZIP uncompressed size exceeds limit".into()); }
        // Read through a bound as well as checking headers: never trust claimed sizes.
        let size = std::io::copy(&mut entry.by_ref().take(allowed + 1), &mut std::io::sink())
            .map_err(|e| format!("ZIP member validation failed: {e}"))?;
        if size > allowed { return Err("ZIP uncompressed size exceeds limit".into()); }
        total += size;
    }
    Ok(())
}
