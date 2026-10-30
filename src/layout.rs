//! Page geometry in PDF points. Built from page bounds only; nothing here renders.

use std::ops::RangeInclusive;

#[cfg(test)]
pub const GAP: f32 = 8.0;
/// Upper bound on pixels per cached tile; taller pages are split into strips.
pub const MAX_TILE_PX: u64 = 4 * 1024 * 1024;
const MIN_STRIP: u32 = 256;

pub struct Layout {
    sizes: Vec<(f32, f32)>,
    tops: Vec<f32>,
    pub max_w: f32,
    pub total_h: f32,
}

impl Layout {
    pub fn new(sizes: Vec<(f32, f32)>, gap: f32) -> Self {
        let mut tops = Vec::with_capacity(sizes.len());
        let mut y = 0.0;
        let mut max_w: f32 = 0.0;
        for &(w, h) in &sizes {
            tops.push(y);
            y += h + gap;
            max_w = max_w.max(w);
        }
        let total_h = (y - gap).max(0.0);
        Layout { sizes, tops, max_w, total_h }
    }

    pub fn sizes(&self) -> &[(f32, f32)] {
        &self.sizes
    }

    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    pub fn size(&self, i: usize) -> (f32, f32) {
        self.sizes[i]
    }

    pub fn top(&self, i: usize) -> f32 {
        self.tops[i]
    }

    /// X offset of page `i` within the content column (pages are centered).
    pub fn left(&self, i: usize) -> f32 {
        (self.max_w - self.sizes[i].0) / 2.0
    }

    /// Page whose slot (page + trailing gap) contains `y`, clamped to valid pages.
    pub fn page_at(&self, y: f32) -> usize {
        self.tops.partition_point(|&t| t <= y).saturating_sub(1)
    }

    /// Pages overlapping the vertical span `[y0, y1)`.
    pub fn visible(&self, y0: f32, y1: f32) -> RangeInclusive<usize> {
        let mut first = self.page_at(y0);
        if self.tops[first] + self.sizes[first].1 <= y0 && first + 1 < self.len() {
            first += 1;
        }
        let mut last = self.page_at(y1);
        if self.tops[last] >= y1 && last > first {
            last -= 1;
        }
        first..=last.max(first)
    }
}

/// Pixel size of a page at `scale` px/pt.
pub fn page_px(size: (f32, f32), scale: f32) -> (u32, u32) {
    (((size.0 * scale).ceil() as u32).max(1), ((size.1 * scale).ceil() as u32).max(1))
}

/// Strip height and strip count for a page of `w`x`h` pixels.
pub fn tile_grid(w: u32, h: u32) -> (u32, u32) {
    let strip = if (w as u64) * (h as u64) <= MAX_TILE_PX {
        h
    } else {
        ((MAX_TILE_PX / w as u64) as u32).clamp(MIN_STRIP, h)
    };
    (strip, h.div_ceil(strip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay() -> Layout {
        Layout::new(vec![(100.0, 200.0), (150.0, 100.0), (100.0, 200.0)], GAP)
    }

    #[test]
    fn tops_and_total() {
        let l = lay();
        assert_eq!(l.top(0), 0.0);
        assert_eq!(l.top(1), 200.0 + GAP);
        assert_eq!(l.top(2), 200.0 + GAP + 100.0 + GAP);
        assert_eq!(l.max_w, 150.0);
        assert_eq!(l.total_h, 200.0 + 100.0 + 200.0 + 2.0 * GAP);
        assert_eq!(l.left(0), 25.0);
    }

    #[test]
    fn page_lookup() {
        let l = lay();
        assert_eq!(l.page_at(-5.0), 0);
        assert_eq!(l.page_at(100.0), 0);
        assert_eq!(l.page_at(l.top(1)), 1);
        assert_eq!(l.page_at(1e9), 2);
    }

    #[test]
    fn visible_ranges() {
        let l = lay();
        assert_eq!(l.visible(0.0, 100.0), 0..=0);
        // span inside the gap after page 0 touches only page 1
        assert_eq!(l.visible(203.0, 215.0), 1..=1);
        assert_eq!(l.visible(150.0, 250.0), 0..=1);
        assert_eq!(l.visible(0.0, 1e9), 0..=2);
    }

    #[test]
    fn tiles() {
        assert_eq!(tile_grid(1000, 1000), (1000, 1));
        let (s, n) = tile_grid(8000, 8000);
        assert!(s as u64 * 8000 <= MAX_TILE_PX && s >= MIN_STRIP);
        assert_eq!(n, 8000u32.div_ceil(s));
        assert_eq!(tile_grid(100_000, 5000).0, MIN_STRIP);
        assert_eq!(page_px((10.2, 5.0), 2.0), (21, 10));
    }
}
