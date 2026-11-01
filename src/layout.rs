//! Page geometry in PDF points. Built from page bounds only; nothing here renders.

use std::ops::RangeInclusive;

#[cfg(test)]
pub const GAP: f32 = 8.0;
/// Upper bound on pixels per cached tile; taller pages are split into strips.
pub const MAX_TILE_PX: u64 = 4 * 1024 * 1024;
const MIN_STRIP: u32 = 256;

/// Horizontal space between the two pages of a spread, in points.
pub const GUTTER: f32 = 2.0;

/// Book mode parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Book {
    /// The first page is a cover shown alone; spreads are (1,2), (3,4)...
    pub first_single: bool,
    /// Right-to-left: the lower page of a spread sits on the right.
    pub rtl: bool,
}

/// One row of the layout: a single page, or a spread of two.
#[derive(Clone, Copy, Debug)]
struct Row {
    first: usize,
    /// Number of pages (1 or 2).
    n: usize,
    top: f32,
    h: f32,
    left: f32,
    w: f32,
}

pub struct Layout {
    sizes: Vec<(f32, f32)>,
    /// Per page: top and left in content coordinates, and its row.
    tops: Vec<f32>,
    lefts: Vec<f32>,
    row_of: Vec<usize>,
    rows: Vec<Row>,
    pub max_w: f32,
    pub total_h: f32,
}

impl Layout {
    /// One page per row, centered.
    pub fn new(sizes: Vec<(f32, f32)>, gap: f32) -> Self {
        Self::build(sizes, gap, None)
    }

    /// Two-page spreads (see `Book`).
    pub fn book(sizes: Vec<(f32, f32)>, gap: f32, book: Book) -> Self {
        Self::build(sizes, gap, Some(book))
    }

    fn build(sizes: Vec<(f32, f32)>, gap: f32, book: Option<Book>) -> Self {
        let n = sizes.len();
        // group the pages into rows
        let mut groups: Vec<(usize, usize)> = Vec::with_capacity(n);
        let mut p = 0;
        while p < n {
            let take = match book {
                None => 1,
                Some(b) if b.first_single && p == 0 => 1,
                Some(_) => (n - p).min(2),
            };
            groups.push((p, take));
            p += take;
        }
        let mut rows = Vec::with_capacity(groups.len());
        let (mut y, mut max_w) = (0.0f32, 0.0f32);
        for &(first, take) in &groups {
            let w: f32 = sizes[first..first + take].iter().map(|s| s.0).sum::<f32>() + GUTTER * (take as f32 - 1.0);
            let h = sizes[first..first + take].iter().map(|s| s.1).fold(0.0, f32::max);
            rows.push(Row { first, n: take, top: y, h, left: 0.0, w });
            y += h + gap;
            max_w = max_w.max(w);
        }
        let total_h = (y - gap).max(0.0);
        let mut tops = vec![0.0; n];
        let mut lefts = vec![0.0; n];
        let mut row_of = vec![0; n];
        for (ri, r) in rows.iter_mut().enumerate() {
            r.left = (max_w - r.w) / 2.0;
            // visual order of the pages in the row, left to right
            let mut order: Vec<usize> = (r.first..r.first + r.n).collect();
            if book.is_some_and(|b| b.rtl) {
                order.reverse();
            }
            let mut x = r.left;
            for &pg in &order {
                lefts[pg] = x;
                tops[pg] = r.top + (r.h - sizes[pg].1) / 2.0;
                row_of[pg] = ri;
                x += sizes[pg].0 + GUTTER;
            }
        }
        Layout { sizes, tops, lefts, row_of, rows, max_w, total_h }
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

    /// Y of the top edge of page `i` (pages of a spread are centered vertically).
    pub fn top(&self, i: usize) -> f32 {
        self.tops[i]
    }

    /// X offset of page `i` within the content column.
    pub fn left(&self, i: usize) -> f32 {
        self.lefts[i]
    }

    /// Top of the row (spread) holding page `i`; where navigation scrolls to.
    pub fn row_top(&self, i: usize) -> f32 {
        self.rows[self.row_of[i]].top
    }

    /// Left edge and width of the row holding page `i`: the whole spread, for fitting.
    pub fn row_span(&self, i: usize) -> (f32, f32) {
        let r = &self.rows[self.row_of[i]];
        (r.left, r.w)
    }

    /// Size of the row holding page `i`.
    pub fn row_size(&self, i: usize) -> (f32, f32) {
        let r = &self.rows[self.row_of[i]];
        (r.w, r.h)
    }

    /// First and last page of the row holding page `i`.
    #[cfg(test)]
    pub fn row_pages(&self, i: usize) -> (usize, usize) {
        let r = &self.rows[self.row_of[i]];
        (r.first, r.first + r.n - 1)
    }

    /// The first page of the row `n` rows after (or before, negative) the one holding `i`, clamped.
    pub fn step(&self, i: usize, n: isize) -> usize {
        let r = (self.row_of[i] as isize + n).clamp(0, self.rows.len() as isize - 1) as usize;
        self.rows[r].first
    }

    fn row_at(&self, y: f32) -> usize {
        self.rows.partition_point(|r| r.top <= y).saturating_sub(1)
    }

    /// First page of the row whose slot (row + trailing gap) contains `y`, clamped to valid pages.
    pub fn page_at(&self, y: f32) -> usize {
        self.rows[self.row_at(y)].first
    }

    /// The page under content point (`x`, `y`): in a spread, the one on that side of the gutter.
    pub fn page_at_xy(&self, x: f32, y: f32) -> usize {
        let r = &self.rows[self.row_at(y)];
        if r.n == 1 {
            return r.first;
        }
        let (a, b) = (r.first, r.first + 1);
        let (l, rt) = if self.lefts[a] <= self.lefts[b] { (a, b) } else { (b, a) };
        if x < self.lefts[l] + self.sizes[l].0 + GUTTER / 2.0 { l } else { rt }
    }

    /// Pages overlapping the vertical span `[y0, y1)`.
    pub fn visible(&self, y0: f32, y1: f32) -> RangeInclusive<usize> {
        let mut first = self.row_at(y0);
        if self.rows[first].top + self.rows[first].h <= y0 && first + 1 < self.rows.len() {
            first += 1;
        }
        let mut last = self.row_at(y1);
        if self.rows[last].top >= y1 && last > first {
            last -= 1;
        }
        let last = last.max(first);
        self.rows[first].first..=self.rows[last].first + self.rows[last].n - 1
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

    fn pages(n: usize) -> Vec<(f32, f32)> {
        vec![(100.0, 200.0); n]
    }

    #[test]
    fn book_spreads_with_a_cover() {
        let l = Layout::book(pages(6), GAP, Book { first_single: true, rtl: false });
        // rows: [0] [1 2] [3 4] [5]
        assert_eq!(l.row_pages(0), (0, 0));
        assert_eq!(l.row_pages(2), (1, 2));
        assert_eq!(l.row_pages(4), (3, 4));
        assert_eq!(l.row_pages(5), (5, 5));
        assert_eq!(l.max_w, 202.0, "two pages and the gutter");
        assert_eq!(l.left(0), 51.0, "the cover is centered");
        assert_eq!((l.left(1), l.left(2)), (0.0, 102.0));
        assert_eq!(l.top(1), l.top(2));
        assert_eq!(l.top(1), 200.0 + GAP);
        assert_eq!(l.total_h, 4.0 * 200.0 + 3.0 * GAP);
        assert_eq!(l.step(0, 1), 1);
        assert_eq!(l.step(1, 1), 3);
        assert_eq!(l.step(2, 1), 3);
        assert_eq!(l.step(4, 1), 5);
        assert_eq!(l.step(5, 1), 5, "clamped at the end");
        assert_eq!(l.step(3, -1), 1, "back to the first page of the spread");
        assert_eq!(l.step(1, -1), 0);
        assert_eq!(l.step(0, -1), 0);
    }

    #[test]
    fn book_without_cover_and_rtl() {
        let l = Layout::book(pages(5), GAP, Book { first_single: false, rtl: false });
        assert_eq!(l.row_pages(0), (0, 1));
        assert_eq!(l.row_pages(3), (2, 3));
        assert_eq!(l.row_pages(4), (4, 4));
        let r = Layout::book(pages(5), GAP, Book { first_single: false, rtl: true });
        assert_eq!((r.left(1), r.left(0)), (0.0, 102.0), "the lower page is on the right");
        assert_eq!(r.page_at(0.0), 0, "navigation still lands on the lower page");
    }

    #[test]
    fn book_hit_testing_picks_the_side() {
        let l = Layout::book(pages(5), GAP, Book { first_single: true, rtl: false });
        let y = l.top(1) + 10.0;
        assert_eq!(l.page_at_xy(10.0, y), 1);
        assert_eq!(l.page_at_xy(150.0, y), 2);
        assert_eq!(l.page_at_xy(-50.0, y), 1);
        assert_eq!(l.page_at_xy(1e6, y), 2);
        assert_eq!(l.page_at_xy(10.0, 0.0), 0, "the cover");
        let r = Layout::book(pages(5), GAP, Book { first_single: true, rtl: true });
        assert_eq!(r.page_at_xy(10.0, y), 2);
        assert_eq!(r.page_at_xy(150.0, y), 1);
        // a page-local coordinate is the content point minus the page origin, whatever the side
        let p = r.page_at_xy(150.0, y);
        assert_eq!((150.0 - r.left(p), y - r.top(p)), (150.0 - 0.0 - 102.0, 10.0));
    }

    #[test]
    fn book_visible_pages_come_in_spreads() {
        let l = Layout::book(pages(7), GAP, Book { first_single: true, rtl: false });
        assert_eq!(l.visible(0.0, 100.0), 0..=0);
        assert_eq!(l.visible(0.0, 300.0), 0..=2, "cover and the first spread");
        assert_eq!(l.visible(l.top(3) + 5.0, l.top(3) + 50.0), 3..=4);
        assert_eq!(l.visible(0.0, 1e9), 0..=6);
    }

    #[test]
    fn mixed_sizes_are_centered_in_the_row_and_fit_uses_the_spread() {
        let l = Layout::book(vec![(100.0, 100.0), (100.0, 200.0), (150.0, 100.0)], GAP, Book { first_single: true, rtl: false });
        assert_eq!(l.row_size(1), (252.0, 200.0));
        assert_eq!(l.row_span(1), (0.0, 252.0));
        assert_eq!(l.top(2), l.top(1) + 50.0, "the shorter page is centered vertically");
        assert_eq!(l.row_top(2), l.top(1));
        assert_eq!(l.row_span(0), (76.0, 100.0));
    }

    #[test]
    fn single_layout_is_unchanged_by_the_row_model() {
        let l = lay();
        assert_eq!(l.step(0, 1), 1);
        assert_eq!(l.page_at_xy(500.0, l.top(1) + 1.0), 1);
        assert_eq!(l.row_span(1), (0.0, 150.0));
        assert_eq!(l.row_pages(2), (2, 2));
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
