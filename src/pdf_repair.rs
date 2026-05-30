//! PDF open/repair chain: single pdfium open on success path; repair only after failure.

use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// Outcome of opening (and optionally repairing) a PDF.
pub struct PdfOpenPlan {
    /// Path passed to pdfium (may be a temp repaired copy).
    pub open_path: PathBuf,
    /// Temp file to delete after processing, if any.
    pub temp_path: Option<PathBuf>,
    pub metadata: HashMap<String, String>,
    pub open_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepairLevel {
    Normal,
    Aggressive,
}

impl RepairLevel {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "normal" | "" => Ok(RepairLevel::Normal),
            "aggressive" => Ok(RepairLevel::Aggressive),
            _ => Err(format!("Unknown repair_level '{}'. Use normal or aggressive.", s)),
        }
    }
}

impl PdfOpenPlan {
    pub fn failed(path: &Path, err: String) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("pdf_open_failed".to_string(), "true".to_string());
        metadata.insert("pdf_error".to_string(), err.clone());
        if err.to_lowercase().contains("password") || err.to_lowercase().contains("encrypt") {
            metadata.insert("pdf_encrypted".to_string(), "true".to_string());
        }
        Self {
            open_path: path.to_path_buf(),
            temp_path: None,
            metadata,
            open_error: Some(err),
        }
    }
}

fn load_password(password: Option<&str>) -> Option<&str> {
    password.filter(|p| !p.is_empty())
}

/// Zero-allocation metadata path for hot text extraction (no repair attempt).
#[inline]
pub fn try_open_pdf<'a>(
    pdfium: &'a Pdfium,
    path: &Path,
    password: Option<&'a str>,
) -> Option<PdfDocument<'a>> {
    pdfium.load_pdf_from_file(path, load_password(password)).ok()
}

/// Open PDF with at most one pdfium load on the happy path; repair + second load only when needed.
pub fn load_pdf<'a>(
    pdfium: &'a Pdfium,
    path: &Path,
    repair_pdf: bool,
    repair_level: RepairLevel,
    pdf_password: Option<&'a str>,
) -> Result<(PdfDocument<'a>, PdfOpenPlan), PdfOpenPlan> {
    let pwd = load_password(pdf_password);
    if let Ok(doc) = pdfium.load_pdf_from_file(path, pwd) {
        let mut metadata = HashMap::new();
        metadata.insert("pdf_open".to_string(), "ok".to_string());
        metadata.insert("repair_level".to_string(), format!("{:?}", repair_level).to_lowercase());
        return Ok((
            doc,
            PdfOpenPlan {
                open_path: path.to_path_buf(),
                temp_path: None,
                metadata,
                open_error: None,
            },
        ));
    }

    let first_err = "pdfium failed to open PDF".to_string();
    if !repair_pdf {
        return Err(PdfOpenPlan::failed(path, first_err));
    }

    let plan = plan_repair_after_failure(pdfium, path, repair_level, pwd);
    if plan.open_error.is_some() {
        return Err(plan);
    }
    match pdfium.load_pdf_from_file(&plan.open_path, pwd) {
        Ok(doc) => Ok((doc, plan)),
        Err(e) => {
            if let Some(temp) = plan.temp_path {
                let _ = std::fs::remove_file(temp);
            }
            let msg = format!("pdfium failed after repair: {:?}", e);
            Err(PdfOpenPlan::failed(path, msg))
        }
    }
}

fn plan_repair_after_failure(
    pdfium: &Pdfium,
    path: &Path,
    repair_level: RepairLevel,
    pdf_password: Option<&str>,
) -> PdfOpenPlan {
    let mut metadata = HashMap::new();
    metadata.insert("pdf_open".to_string(), "failed".to_string());
    metadata.insert(
        "repair_level".to_string(),
        format!("{:?}", repair_level).to_lowercase(),
    );

    if mupdf_can_open(path) {
        metadata.insert("mupdf_readable".to_string(), "true".to_string());
        metadata.insert(
            "repair_hint".to_string(),
            "mupdf can read; pdfium cannot — try external repair".to_string(),
        );
    }

    if let Some((repaired, method)) = external_repair(path, repair_level) {
        metadata.insert("repair_method".to_string(), method.to_string());
        metadata.insert("xref_repaired".to_string(), "true".to_string());
        if pdfium_can_open(pdfium, &repaired, pdf_password) {
            metadata.insert("pdf_open".to_string(), "ok_after_repair".to_string());
            return PdfOpenPlan {
                open_path: repaired.clone(),
                temp_path: Some(repaired),
                metadata,
                open_error: None,
            };
        }
        let _ = std::fs::remove_file(&repaired);
        metadata.insert("repair_retry_failed".to_string(), "true".to_string());
    } else {
        metadata.insert("external_repair_skipped".to_string(), "no_tool".to_string());
    }

    PdfOpenPlan::failed(path, "pdfium failed to open PDF".to_string())
}

pub fn pdfium_can_open(pdfium: &Pdfium, path: &Path, pdf_password: Option<&str>) -> bool {
    pdfium
        .load_pdf_from_file(path, load_password(pdf_password))
        .is_ok()
}

#[cfg(feature = "mupdf")]
fn mupdf_can_open(path: &Path) -> bool {
    use mupdf::Document;
    Document::open(path).is_ok()
}

#[cfg(not(feature = "mupdf"))]
fn mupdf_can_open(_path: &Path) -> bool {
    false
}

static QPDF_AVAILABLE: LazyLock<bool> = LazyLock::new(|| tool_available("qpdf"));
static GS_WIN64_AVAILABLE: LazyLock<bool> = LazyLock::new(|| tool_available("gswin64c"));
static GS_WIN32_AVAILABLE: LazyLock<bool> = LazyLock::new(|| tool_available("gswin32c"));
static GS_UNIX_AVAILABLE: LazyLock<bool> = LazyLock::new(|| tool_available("gs"));

fn tool_available(name: &str) -> bool {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    Command::new(cmd)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_qpdf_repair(input: &Path, output: &Path, aggressive: bool) -> bool {
    if !*QPDF_AVAILABLE {
        return false;
    }
    let mut cmd = Command::new("qpdf");
    if aggressive {
        cmd.args([
            "--object-streams=disable",
            "--stream-data=uncompress",
            input.to_string_lossy().as_ref(),
            output.to_string_lossy().as_ref(),
        ]);
    } else {
        cmd.args([
            "--linearize",
            input.to_string_lossy().as_ref(),
            output.to_string_lossy().as_ref(),
        ]);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn run_qpdf_check_fix(input: &Path, output: &Path) -> bool {
    if !*QPDF_AVAILABLE {
        return false;
    }
    Command::new("qpdf")
        .args([
            "--check",
            "--replace-input",
            input.to_string_lossy().as_ref(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && std::fs::copy(input, output).is_ok()
}

fn run_gs_repair(input: &Path, output: &Path) -> bool {
    let gs = if *GS_WIN64_AVAILABLE {
        "gswin64c"
    } else if *GS_WIN32_AVAILABLE {
        "gswin32c"
    } else if *GS_UNIX_AVAILABLE {
        "gs"
    } else {
        return false;
    };
    Command::new(gs)
        .args([
            "-q",
            "-dNOPAUSE",
            "-dBATCH",
            "-sDEVICE=pdfwrite",
            &format!("-sOutputFile={}", output.display()),
            &input.to_string_lossy(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn external_repair(input: &Path, level: RepairLevel) -> Option<(PathBuf, &'static str)> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("doc");
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let out = parent.join(format!(".brainpipe_repair_{stem}.pdf"));

    if level == RepairLevel::Aggressive {
        if run_qpdf_check_fix(input, &out) {
            return Some((out, "qpdf_check"));
        }
        if run_qpdf_repair(input, &out, true) {
            return Some((out, "qpdf_aggressive"));
        }
    }
    if run_qpdf_repair(input, &out, false) {
        return Some((out, "qpdf"));
    }
    if run_gs_repair(input, &out) {
        return Some((out, "ghostscript"));
    }
    if level == RepairLevel::Aggressive {
        let out2 = parent.join(format!(".brainpipe_repair2_{stem}.pdf"));
        if run_gs_repair(input, &out2) {
            return Some((out2, "ghostscript_retry"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_detection_does_not_panic() {
        let _ = tool_available("qpdf");
        let _ = tool_available("nonexistent_tool_xyz");
    }

    #[test]
    fn repair_level_parse() {
        assert_eq!(
            RepairLevel::from_str("aggressive").unwrap(),
            RepairLevel::Aggressive
        );
    }
}
