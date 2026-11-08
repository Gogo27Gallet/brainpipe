//! One-level ZIP expansion for ingest (opt-in).

const MAX_ARCHIVE_ENTRIES: usize = 1000;
const MAX_MEMBER_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;

/// If `ingest_archives` and path is `.zip`, extract supported members to a temp dir (one level).
pub fn expand_zip_archive(path: &Path) -> Result<(Vec<PathBuf>, tempfile::TempDir), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("ZIP open: {}", e))?;
    // Unique ownership avoids collisions between archives and concurrent runs;
    // TempDir removes partial output on every early return.
    let temp = tempfile::Builder::new().prefix("brainpipe_zip_")
        .tempdir().map_err(|e| e.to_string())?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("ZIP entry count exceeds limit".into());
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let enclosed = entry.enclosed_name()
            .ok_or_else(|| "Unsafe ZIP entry path".to_string())?;
        let inner_name = enclosed.file_name()
            .ok_or_else(|| "Invalid ZIP entry name".to_string())?;
        let ext = enclosed.extension().and_then(|s| s.to_str()).unwrap_or("");
        if !crate::formats::is_supported_extension(ext) || ext.eq_ignore_ascii_case("zip") {
            continue;
        }
        if entry.size() > MAX_MEMBER_BYTES || entry.size() > MAX_ARCHIVE_BYTES - total {
            return Err("ZIP uncompressed size exceeds limit".into());
        }
        // Preserve distinct members with identical basenames, without recreating
        // untrusted directory trees or symlinks.
        let member_dir = temp.path().join(i.to_string());
        std::fs::create_dir(&member_dir).map_err(|e| e.to_string())?;
        let dest = member_dir.join(inner_name);
        let mut output = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        let allowed = MAX_MEMBER_BYTES.min(MAX_ARCHIVE_BYTES - total);
        let copied = std::io::copy(&mut entry.by_ref().take(allowed + 1), &mut output)
            .map_err(|e| e.to_string())?;
        if copied > allowed {
            return Err("ZIP uncompressed size exceeds limit".into());
        }
        total += copied;
        out.push(dest);
    }
    if out.is_empty() {
        return Err("ZIP contains no supported files".to_string());
    }
    Ok((out, temp))
}

/// Collect walkdir paths; optionally expand `.zip` archives one level.
pub fn collect_ingest_paths(
    directory: &str,
    filter_name: Option<&regex::Regex>,
    ingest_archives: bool,
) -> Result<(Vec<PathBuf>, Vec<tempfile::TempDir>), String> {
    let mut paths = Vec::new();
    let mut temp_dirs = Vec::new();
    for entry in WalkDir::new(directory) {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if let Some(re) = filter_name {
            if !re.is_match(fname) {
                continue;
            }
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ingest_archives && ext.eq_ignore_ascii_case("zip") {
            match expand_zip_archive(&path) {
                Ok((members, temp)) => {
                    temp_dirs.push(temp);
                    paths.extend(members);
                }
                Err(e) => {
                    cleanup_temp_dirs(&temp_dirs);
                    return Err(format!("ZIP {}: {}", path.display(), e));
                }
            }
            continue;
        }
        if crate::formats::path_has_supported_extension(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok((paths, temp_dirs))
}

pub fn cleanup_temp_dirs(dirs: &[tempfile::TempDir]) {
    for d in dirs {
        let _ = std::fs::remove_dir_all(d.path());
    }
}
