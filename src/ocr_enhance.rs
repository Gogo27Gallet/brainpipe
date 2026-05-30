//! OCR preprocessing and hybrid (partial-page) triggers — opt-in only.

use image::{imageops, RgbImage};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OcrMode {
    Fast,
    Quality,
    Hybrid,
    /// Multi-scale render + contrast; problem pages only.
    Vision,
}

impl OcrMode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "fast" | "" => Ok(OcrMode::Fast),
            "quality" | "high" => Ok(OcrMode::Quality),
            "hybrid" => Ok(OcrMode::Hybrid),
            "vision" => Ok(OcrMode::Vision),
            _ => Err(format!(
                "Unknown ocr_mode '{}'. Use fast, quality, hybrid, or vision.",
                s
            )),
        }
    }

    pub fn uses_multiscale(self) -> bool {
        matches!(self, OcrMode::Vision | OcrMode::Quality)
    }
}

/// Grayscale + contrast stretch for quality/vision modes (in-place on RGB buffer).
pub fn preprocess_rgb_for_ocr(rgb: &mut [u8], mode: OcrMode) {
    if mode == OcrMode::Fast {
        return;
    }
    let n = rgb.len() / 3;
    let mut min_v = 255u8;
    let mut max_v = 0u8;
    for i in 0..n {
        let r = rgb[i * 3];
        let g = rgb[i * 3 + 1];
        let b = rgb[i * 3 + 2];
        let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
        min_v = min_v.min(gray);
        max_v = max_v.max(gray);
    }
    let span = (max_v.saturating_sub(min_v)).max(1) as f32;
    for i in 0..n {
        let r = rgb[i * 3];
        let g = rgb[i * 3 + 1];
        let b = rgb[i * 3 + 2];
        let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
        let stretched = (((gray.saturating_sub(min_v)) as f32 / span) * 255.0) as u8;
        rgb[i * 3] = stretched;
        rgb[i * 3 + 1] = stretched;
        rgb[i * 3 + 2] = stretched;
    }
}

pub fn render_scale_to_width(base_width: i32, scale: f32) -> i32 {
    ((base_width as f32) * scale.clamp(0.5, 3.0)) as i32
}

/// Hybrid OCR: partial native text but layout complex or very low density.
pub fn needs_hybrid_ocr(
    native_text_len: usize,
    text_density: f32,
    layout_complex: bool,
    mode: OcrMode,
    strategy_ocr: bool,
) -> bool {
    if mode != OcrMode::Hybrid && !strategy_ocr {
        return false;
    }
    if mode == OcrMode::Hybrid || strategy_ocr {
        let sparse = native_text_len < 80 || text_density < 0.08;
        return sparse && (layout_complex || text_density < 0.15);
    }
    false
}

/// Broken/garbled page: open OK but unusable native text — trigger vision OCR path.
pub fn needs_broken_page_recovery(
    native_text_len: usize,
    text_density: f32,
    garbled: bool,
    mode: OcrMode,
) -> bool {
    if mode == OcrMode::Fast {
        return false;
    }
    let empty = native_text_len < 5;
    let sparse = text_density < 0.06 && native_text_len < 120;
    empty || sparse || (garbled && native_text_len < 500)
}

/// Scales to try for vision OCR (base + 1.5×).
pub fn vision_ocr_scales(base_scale: f32) -> Vec<f32> {
    let b = base_scale.clamp(0.75, 2.0);
    if (b - 1.5).abs() < 0.01 {
        vec![1.0, 1.5]
    } else {
        vec![b, (b * 1.5).min(3.0)]
    }
}

/// Pick best OCR candidate by printable character count (proxy for confidence).
pub fn pick_best_ocr_text(candidates: &[(f32, String)]) -> (String, Vec<String>) {
    let mut scales: Vec<String> = Vec::new();
    let best = candidates
        .iter()
        .max_by_key(|(scale, t)| {
            scales.push(format!("{scale:.2}"));
            (score_ocr_text(t), (t.len() as i64))
        })
        .map(|(_, t)| t.clone())
        .unwrap_or_default();
    (best, scales)
}

fn score_ocr_text(t: &str) -> i64 {
    t.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .count() as i64
}

#[allow(dead_code)]
pub fn resize_rgb_image(rgb: RgbImage, target_width: u32) -> RgbImage {
    let (w, h) = rgb.dimensions();
    if w <= target_width {
        return rgb;
    }
    let new_h = ((h as f32) * (target_width as f32 / w as f32)) as u32;
    imageops::resize(&rgb, target_width, new_h.max(1), imageops::FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_triggers_on_sparse_complex() {
        assert!(needs_hybrid_ocr(10, 0.02, true, OcrMode::Hybrid, false));
        assert!(!needs_hybrid_ocr(5000, 2.0, false, OcrMode::Fast, false));
    }

    #[test]
    fn vision_scales() {
        assert_eq!(vision_ocr_scales(1.0).len(), 2);
    }
}
