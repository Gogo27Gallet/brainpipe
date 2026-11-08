//! Bounded per-page native PDF geometry, in display-oriented top-left points.
use pdfium_render::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use serde::Serialize;
use std::sync::mpsc::{sync_channel, Receiver};

#[derive(Serialize)]
struct Glyph {
    text: String,
    bbox: [f32; 4],
    layout_bbox: [f32; 4],
    font_size: f32,
}
#[derive(Serialize)]
struct PageGeometry {
    page_index: usize,
    width: f32,
    height: f32,
    rotation: String,
    glyphs: Vec<Glyph>,
}

#[pyclass(unsendable)]
pub struct PdfGeometryStream {
    receiver: Option<Receiver<Result<String, String>>>,
}

#[pymethods]
impl PdfGeometryStream {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }
    fn __next__(&self, py: Python<'_>) -> PyResult<Option<String>> {
        let Some(rx) = self.receiver.as_ref() else {
            return Ok(None);
        };
        // Receiver is not Sync. Scoped release via a mutex-free ownership move
        // is handled below by keeping the GIL during a short non-blocking poll.
        loop {
            match rx.try_recv() {
                Ok(Ok(page)) => return Ok(Some(page)),
                Ok(Err(error)) => return Err(PyRuntimeError::new_err(error)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(None),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    py.allow_threads(|| std::thread::sleep(std::time::Duration::from_millis(2)));
                    py.check_signals()?;
                }
            }
        }
    }
    fn close(&mut self) {
        self.receiver.take();
    }
}

fn read_page(page: &PdfPage<'_>, index: usize) -> Result<String, String> {
    let width = page.width().value;
    let height = page.height().value;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("Invalid PDF page dimensions".into());
    }
    // No bitmap allocation: only the same device transform used for rendering.
    let config = PdfRenderConfig::new().scale_page_by_factor(10.0);
    let text = page.text().map_err(|e| e.to_string())?;
    let mut glyphs = Vec::new();
    for character in text.chars().iter() {
        let Some(value) = character.unicode_string() else {
            continue;
        };
        if value.chars().all(char::is_control) {
            continue;
        }
        let bounds = character
            .tight_bounds()
            .or_else(|_| character.loose_bounds());
        let Ok(bounds) = bounds else { continue };
        let mut xs = Vec::with_capacity(4);
        let mut ys = Vec::with_capacity(4);
        for x in [bounds.left(), bounds.right()] {
            for y in [bounds.top(), bounds.bottom()] {
                if let Ok((x, y)) = page.points_to_pixels(x, y, &config) {
                    xs.push(x as f32 / 10.0);
                    ys.push(y as f32 / 10.0);
                }
            }
        }
        if xs.len() != 4 {
            continue;
        }
        let bbox = [
            xs.iter()
                .copied()
                .fold(f32::INFINITY, f32::min)
                .clamp(0.0, width),
            ys.iter()
                .copied()
                .fold(f32::INFINITY, f32::min)
                .clamp(0.0, height),
            xs.iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                .clamp(0.0, width),
            ys.iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                .clamp(0.0, height),
        ];
        if bbox[2] < bbox[0] || bbox[3] < bbox[1] {
            continue;
        }
        let loose = character.loose_bounds().unwrap_or(bounds);
        let layout_bbox = [
            loose.left().value,
            -loose.top().value,
            loose.right().value,
            -loose.bottom().value,
        ];
        glyphs.push(Glyph {
            text: value,
            bbox,
            layout_bbox,
            font_size: character.scaled_font_size().value,
        });
    }
    serde_json::to_string(&PageGeometry {
        page_index: index,
        width,
        height,
        rotation: format!("{:?}", page.rotation().map_err(|e| e.to_string())?),
        glyphs,
    })
    .map_err(|e| e.to_string())
}

#[pyfunction]
#[pyo3(signature = (path, max_pages=500, password=None))]
pub fn pdf_geometry(
    path: String,
    max_pages: usize,
    password: Option<String>,
) -> PyResult<PdfGeometryStream> {
    if max_pages == 0 {
        return Err(PyValueError::new_err("max_pages must be positive"));
    }
    let library = crate::resolve_pdfium_path().to_string();
    Pdfium::bind_to_library(&library)
        .or_else(|_| Pdfium::bind_to_system_library())
        .map_err(|e| PyRuntimeError::new_err(format!("PDFium unavailable: {e}")))?;
    let (tx, rx) = sync_channel(2);
    std::thread::spawn(move || {
        crate::with_thread_pdfium(&library, |pdfium| {
            let document = match pdfium.load_pdf_from_file(&path, password.as_deref()) {
                Ok(doc) => doc,
                Err(error) => {
                    let _ = tx.send(Err(format!("PDF open failed: {error}")));
                    return;
                }
            };
            for index in 0..max_pages.min(document.pages().len() as usize) {
                let result = document
                    .pages()
                    .get(index as u16)
                    .map_err(|e| e.to_string())
                    .and_then(|page| read_page(&page, index));
                let failed = result.is_err();
                if tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
    });
    Ok(PdfGeometryStream { receiver: Some(rx) })
}

#[pyfunction]
#[pyo3(signature = (path, page_index=0, width=1000, password=None))]
pub fn render_pdf_page<'py>(
    py: Python<'py>,
    path: String,
    page_index: usize,
    width: i32,
    password: Option<String>,
) -> PyResult<Bound<'py, pyo3::types::PyBytes>> {
    if !(64..=2000).contains(&width) || page_index > u16::MAX as usize {
        return Err(PyValueError::new_err(
            "Invalid preview dimensions or page index",
        ));
    }
    let library = crate::resolve_pdfium_path().to_string();
    Pdfium::bind_to_library(&library)
        .or_else(|_| Pdfium::bind_to_system_library())
        .map_err(|e| PyRuntimeError::new_err(format!("PDFium unavailable: {e}")))?;
    let result: Result<Vec<u8>, String> = py.allow_threads(move || {
        crate::with_thread_pdfium(&library, |pdfium| {
            let doc = pdfium
                .load_pdf_from_file(&path, password.as_deref())
                .map_err(|e| e.to_string())?;
            let page = doc
                .pages()
                .get(page_index as u16)
                .map_err(|e| e.to_string())?;
            let bitmap = page
                .render_with_config(
                    &PdfRenderConfig::new()
                        .set_target_width(width)
                        .set_maximum_height(2000),
                )
                .map_err(|e| e.to_string())?;
            let mut bytes = std::io::Cursor::new(Vec::new());
            bitmap
                .as_image()
                .write_to(&mut bytes, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok(bytes.into_inner())
        })
    });
    let bytes = result.map_err(PyRuntimeError::new_err)?;
    Ok(pyo3::types::PyBytes::new(py, &bytes))
}
