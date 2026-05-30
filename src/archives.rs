//! One-level ZIP expansion for ingest (opt-in).

use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;

/// If `ingest_archives` and path is `.zip`, extract supported members to a temp dir (one level).
pub fn expand_zip_archive(path: &Path) -> Result<Vec<PathBuf>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("ZIP open: {}", e))?;
    let temp = std::env::temp_dir().join(format!(
        "brainpipe_zip_{}_{}",
        std::process::id(),
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("arc")
    ));
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if name.contains("..") {
            continue;
        }
        let inner_name = Path::new(&name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = Path::new(inner_name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !crate::formats::is_supported_extension(ext) || ext.eq_ignore_ascii_case("zip") {
            continue;
        }
        let dest = temp.join(inner_name);
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&dest, &buf).map_err(|e| e.to_string())?;
        out.push(dest);
    }
    if out.is_empty() {
        let _ = std::fs::remove_dir_all(&temp);
        return Err("ZIP contains no supported files".to_string());
    }
    Ok(out)
}

/// Collect walkdir paths; optionally expand `.zip` archives one level.
pub fn collect_ingest_paths(
    directory: &str,
    filter_name: Option<&regex::Regex>,
    ingest_archives: bool,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut paths = Vec::new();
    let mut temp_dirs = Vec::new();
    for entry in WalkDir::new(directory).into_iter().filter_map(Result::ok) {
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
                Ok(members) => {
                    if let Some(parent) = members.first().and_then(|p| p.parent()) {
                        temp_dirs.push(parent.to_path_buf());
                    }
                    paths.extend(members);
                }
                Err(_) => {}
            }
            continue;
        }
        if crate::formats::path_has_supported_extension(&path) {
            paths.push(path);
        }
    }
    (paths, temp_dirs)
}

pub fn cleanup_temp_dirs(dirs: &[PathBuf]) {
    for d in dirs {
        let _ = std::fs::remove_dir_all(d);
    }
}
