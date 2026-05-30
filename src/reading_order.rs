//! Recursive XY-Cut for geometric reading order (multi-column, Z-pattern).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    fn cx(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    fn cy(&self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }
}

/// Returns block indices in human reading order.
/// `y_increases_down`: false for PDF (origin bottom-left), true for image/YOLO coords.
pub fn xy_cut_reading_order(
    blocks: &[BBox],
    page_w: f32,
    page_h: f32,
    y_increases_down: bool,
) -> Vec<usize> {
    let n = blocks.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }
    let min_gap_x = (page_w * 0.02).max(8.0);
    let min_gap_y = (page_h * 0.015).max(6.0);
    let mut indices: Vec<usize> = (0..n).collect();
    sort_group(blocks, &mut indices, min_gap_x, min_gap_y, y_increases_down);
    indices
}

fn sort_group(
    blocks: &[BBox],
    indices: &mut Vec<usize>,
    min_gap_x: f32,
    min_gap_y: f32,
    y_increases_down: bool,
) {
    let n = indices.len();
    if n <= 1 {
        return;
    }

    if let Some(cut) = largest_gap_cut(blocks, indices, true, min_gap_x) {
        let (mut first, mut second) = partition_indices(blocks, indices, cut, true);
        if !first.is_empty() && !second.is_empty() {
            sort_group(blocks, &mut first, min_gap_x, min_gap_y, y_increases_down);
            sort_group(blocks, &mut second, min_gap_x, min_gap_y, y_increases_down);
            indices.clear();
            indices.extend(first);
            indices.extend(second);
            return;
        }
    }

    if let Some(cut) = largest_gap_cut(blocks, indices, false, min_gap_y) {
        let (mut first, mut second) = partition_indices(blocks, indices, cut, false);
        if !first.is_empty() && !second.is_empty() {
            sort_group(blocks, &mut first, min_gap_x, min_gap_y, y_increases_down);
            sort_group(blocks, &mut second, min_gap_x, min_gap_y, y_increases_down);
            if y_increases_down {
                indices.clear();
                indices.extend(first);
                indices.extend(second);
            } else {
                indices.clear();
                indices.extend(second);
                indices.extend(first);
            }
            return;
        }
    }

    fallback_sort(blocks, indices, y_increases_down);
}

fn fallback_sort(blocks: &[BBox], indices: &mut Vec<usize>, y_increases_down: bool) {
    indices.sort_by(|&a, &b| {
        let ba = &blocks[a];
        let bb = &blocks[b];
        let ya = ba.cy();
        let yb = bb.cy();
        let y_cmp = if y_increases_down {
            ya.partial_cmp(&yb)
        } else {
            yb.partial_cmp(&ya)
        };
        y_cmp
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ba.x0.partial_cmp(&bb.x0).unwrap_or(std::cmp::Ordering::Equal))
    });
}

fn merge_intervals(mut intervals: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    if intervals.is_empty() {
        return Vec::new();
    }
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged = vec![intervals[0]];
    for (s, e) in intervals.into_iter().skip(1) {
        let last = merged.last_mut().unwrap();
        if s <= last.1 {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    merged
}

fn largest_gap_cut(
    blocks: &[BBox],
    indices: &[usize],
    vertical: bool,
    min_gap: f32,
) -> Option<f32> {
    let intervals: Vec<(f32, f32)> = indices
        .iter()
        .map(|&i| {
            let b = &blocks[i];
            if vertical {
                (b.x0, b.x1)
            } else {
                (b.y0, b.y1)
            }
        })
        .collect();
    let merged = merge_intervals(intervals);
    if merged.len() < 2 {
        return None;
    }
    let mut best_gap = 0.0f32;
    let mut best_cut = None;
    for w in merged.windows(2) {
        let gap = w[1].0 - w[0].1;
        if gap > best_gap {
            best_gap = gap;
            best_cut = Some((w[0].1 + w[1].0) * 0.5);
        }
    }
    if best_gap >= min_gap {
        best_cut
    } else {
        None
    }
}

fn partition_indices(
    blocks: &[BBox],
    indices: &[usize],
    cut: f32,
    vertical: bool,
) -> (Vec<usize>, Vec<usize>) {
    let mut first = Vec::new();
    let mut second = Vec::new();
    for &i in indices {
        let c = if vertical {
            blocks[i].cx()
        } else {
            blocks[i].cy()
        };
        if c < cut {
            first.push(i);
        } else {
            second.push(i);
        }
    }
    (first, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(x0: f32, y0: f32, x1: f32, y1: f32) -> BBox {
        BBox::new(x0, y0, x1, y1)
    }

    #[test]
    fn two_column_pdf_coords() {
        let blocks = vec![
            block(10.0, 100.0, 90.0, 120.0),
            block(10.0, 70.0, 90.0, 90.0),
            block(210.0, 100.0, 290.0, 120.0),
            block(210.0, 70.0, 290.0, 90.0),
        ];
        let order = xy_cut_reading_order(&blocks, 300.0, 150.0, false);
        assert_eq!(order.len(), 4);
        let left: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| blocks[i].x0 < 150.0)
            .collect();
        let right: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| blocks[i].x0 >= 150.0)
            .collect();
        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        let left_pos: Vec<usize> = order
            .iter()
            .position(|&i| blocks[i].x0 < 150.0)
            .into_iter()
            .collect();
        let right_start = order.iter().position(|&i| blocks[i].x0 >= 150.0).unwrap();
        assert!(right_start >= 2);
        assert!(left_pos[0] < right_start);
    }

    #[test]
    fn full_width_title_then_columns() {
        let blocks = vec![
            block(10.0, 130.0, 290.0, 150.0),
            block(10.0, 90.0, 90.0, 110.0),
            block(210.0, 90.0, 290.0, 110.0),
            block(10.0, 60.0, 90.0, 80.0),
            block(210.0, 60.0, 290.0, 80.0),
        ];
        let order = xy_cut_reading_order(&blocks, 300.0, 160.0, false);
        assert_eq!(order[0], 0);
        let rest: Vec<usize> = order[1..].to_vec();
        let left_first = rest.iter().position(|&i| blocks[i].x0 < 150.0).unwrap();
        let right_first = rest.iter().position(|&i| blocks[i].x0 >= 150.0).unwrap();
        assert!(left_first < right_first);
    }

    #[test]
    fn sidebar_right() {
        let blocks = vec![
            block(10.0, 100.0, 200.0, 120.0),
            block(10.0, 70.0, 200.0, 90.0),
            block(230.0, 100.0, 280.0, 120.0),
            block(230.0, 70.0, 280.0, 90.0),
        ];
        let order = xy_cut_reading_order(&blocks, 300.0, 150.0, false);
        let main: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| blocks[i].x1 < 220.0)
            .collect();
        let side: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| blocks[i].x0 >= 220.0)
            .collect();
        assert_eq!(main.len(), 2);
        assert_eq!(side.len(), 2);
        let main_max = order.iter().position(|&i| blocks[i].x0 >= 220.0).unwrap();
        assert!(main_max >= 2);
    }

    #[test]
    fn single_block() {
        let blocks = vec![block(0.0, 0.0, 100.0, 20.0)];
        assert_eq!(xy_cut_reading_order(&blocks, 100.0, 20.0, false), vec![0]);
    }

    #[test]
    fn image_y_down_columns() {
        let blocks = vec![
            block(10.0, 10.0, 90.0, 30.0),
            block(10.0, 40.0, 90.0, 60.0),
            block(210.0, 10.0, 290.0, 30.0),
            block(210.0, 40.0, 290.0, 60.0),
        ];
        let order = xy_cut_reading_order(&blocks, 300.0, 80.0, true);
        let left: Vec<_> = order.iter().filter(|&&i| blocks[i].x0 < 150.0).collect();
        let right: Vec<_> = order.iter().filter(|&&i| blocks[i].x0 >= 150.0).collect();
        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        let first_right = order.iter().position(|&i| blocks[i].x0 >= 150.0).unwrap();
        assert!(first_right >= 2);
    }
}
