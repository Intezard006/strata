//! The pin pane: one page (or a region of it) in a right-hand split of the main window. Pure layout
//! maths and the clipped drawing; the app owns the state transitions.

use std::collections::HashSet;

use super::Canvas;
use crate::layout::{page_px, tile_grid, Layout};
use crate::render::{TileCache, TileKey};
use crate::search::Rect4;

/// Margin around a pinned region, in page points.
const MARGIN: f32 = 8.0;
pub const MIN_MUL: f32 = 0.3;
pub const MAX_MUL: f32 = 8.0;

pub struct Pin {
    pub page: usize,
    /// The preview target's rectangle; `None` pins the whole page.
    pub region: Option<Rect4>,
    /// Zoom relative to fit-width of the pane.
    pub mul: f32,
    /// Scroll in pixels inside the pinned content.
    pub sx: f32,
    pub sy: f32,
    /// Keys and wheel act on the pane.
    pub focus: bool,
    pub pending: HashSet<TileKey>,
}

impl Pin {
    pub fn new(page: usize, region: Option<Rect4>) -> Self {
        Pin { page, region, mul: 1.0, sx: 0.0, sy: 0.0, focus: false, pending: HashSet::new() }
    }
}

/// (main width, pane width) of a window `win_w` wide with the pane taking `pct` percent.
pub fn split(win_w: u32, pct: f32) -> (u32, u32) {
    if win_w < 240 {
        return (win_w - win_w / 2, win_w / 2);
    }
    let pane = ((win_w as f32 * pct / 100.0).round() as u32).clamp(120, win_w - 120);
    (win_w - pane, pane)
}

/// Where the pinned content sits: scale (px per point, a whole number of thousandths so the tile key is
/// exact), the content origin in page pixels and its size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub key: u32,
    pub x0: f32,
    pub y0: f32,
    pub w: f32,
    pub h: f32,
}

impl Geo {
    pub fn scale(&self) -> f32 {
        self.key as f32 / 1000.0
    }

    /// Largest scroll (x, y) for a pane of `pw` x `ph` pixels.
    pub fn max_scroll(&self, pw: f32, ph: f32) -> (f32, f32) {
        ((self.w - pw).max(0.0), (self.h - ph).max(0.0))
    }
}

pub fn geo(size: (f32, f32), region: Option<Rect4>, pane_w: f32, mul: f32) -> Geo {
    let (cw, ch, ox, oy) = match region {
        None => (size.0, size.1, 0.0, 0.0),
        Some(r) => ((r[2] - r[0]).max(40.0) + 2.0 * MARGIN, (r[3] - r[1]).max(8.0) + 2.0 * MARGIN, r[0] - MARGIN, r[1] - MARGIN),
    };
    let key = ((pane_w / cw.max(1.0) * mul.clamp(MIN_MUL, MAX_MUL) * 1000.0).round() as u32).clamp(50, 16000);
    let s = key as f32 / 1000.0;
    Geo { key, x0: ox * s, y0: oy * s, w: cw * s, h: ch * s }
}

/// Tiles of the page that intersect a pane of `pw` x `ph` pixels scrolled to (`sx`, `sy`), with their
/// position relative to the pane's top-left corner.
pub fn tiles(page: usize, size: (f32, f32), g: &Geo, sx: f32, sy: f32, pw: i32, ph: i32) -> Vec<(TileKey, i64, i64)> {
    let (w, h) = page_px(size, g.scale());
    let (strip, n) = tile_grid(w, h);
    let mut out = Vec::new();
    for t in 0..n {
        let th = strip.min(h - t * strip) as i64;
        let y = (t * strip) as i64 - (g.y0 + sy).round() as i64;
        if y < ph as i64 && y + th > 0 {
            out.push((TileKey { page: page as u32, scale: g.key, tile: t }, -(g.x0 + sx).round() as i64, y));
        }
    }
    let _ = pw;
    out
}

/// Draw the pane into `rect` (x, y, w, h) of the canvas: its background, then its tiles clipped to it.
pub fn draw(cv: &mut Canvas, cache: &mut TileCache, p: &Pin, layout: &Layout, rect: (i32, i32, i32, i32), bg: u32, focus_color: u32) {
    let (rx, ry, rw, rh) = rect;
    let (x1, y1) = ((rx + rw).min(cv.w), (ry + rh).min(cv.h));
    for y in ry.max(0)..y1 {
        cv.buf[(y * cv.w + rx.max(0)) as usize..(y * cv.w + x1) as usize].fill(bg);
    }
    let g = geo(layout.size(p.page), p.region, rw as f32, p.mul);
    for (k, x, y) in tiles(p.page, layout.size(p.page), &g, p.sx, p.sy, rw, rh) {
        if let Some(t) = cache.get(&k) {
            blit_clip(cv, t.w as i64, t.h as i64, &t.data, rx as i64 + x, ry as i64 + y, (rx as i64, ry as i64, x1 as i64, y1 as i64));
        }
    }
    cv.fill_rect(rx, ry, 2, rh, focus_color);
}

/// Source-over blit of premultiplied pixels clipped to `clip` (x0, y0, x1, y1).
fn blit_clip(cv: &mut Canvas, tw: i64, th: i64, src: &[u32], dx: i64, dy: i64, clip: (i64, i64, i64, i64)) {
    let (cx0, cx1) = (dx.max(clip.0), (dx + tw).min(clip.2));
    let (cy0, cy1) = (dy.max(clip.1), (dy + th).min(clip.3));
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }
    for y in cy0..cy1 {
        let s_row = &src[((y - dy) * tw + (cx0 - dx)) as usize..][..(cx1 - cx0) as usize];
        let d_row = &mut cv.buf[(y * cv.w as i64 + cx0) as usize..][..(cx1 - cx0) as usize];
        for (d, &s) in d_row.iter_mut().zip(s_row) {
            *d = super::over(s, *d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_gives_the_pane_its_share() {
        assert_eq!(split(1920, 35.0), (1248, 672));
        assert_eq!(split(1000, 70.0), (300, 700));
        assert_eq!(split(1000, 10.0), (880, 120), "the pane keeps a usable minimum");
        assert_eq!(split(100, 35.0), (50, 50));
        let (m, p) = split(1366, 35.0);
        assert_eq!(m + p, 1366);
    }

    #[test]
    fn whole_page_fits_the_pane_width() {
        let g = geo((600.0, 800.0), None, 672.0, 1.0);
        assert_eq!(g.key, 1120);
        assert!((g.w - 672.0).abs() < 0.5);
        assert_eq!((g.x0, g.y0), (0.0, 0.0));
        let z = geo((600.0, 800.0), None, 672.0, 2.0);
        assert_eq!(z.key, 2240);
        assert_eq!(z.max_scroll(672.0, 500.0).0.round(), 672.0);
        assert!(geo((600.0, 800.0), None, 672.0, 1000.0).key <= 16000, "zoom is bounded");
    }

    #[test]
    fn region_starts_at_its_corner() {
        let r = [100.0, 200.0, 300.0, 260.0];
        let g = geo((600.0, 800.0), Some(r), 432.0, 1.0);
        assert_eq!(g.key, 2000, "216 pt wide with margins into 432 px");
        assert_eq!((g.x0, g.y0), (184.0, 384.0));
        assert_eq!((g.w, g.h), (432.0, 152.0));
        // the first tile row of the page that holds y=192*2
        let t = tiles(3, (600.0, 800.0), &g, 0.0, 0.0, 432, 200);
        assert!(!t.is_empty() && t.iter().all(|(k, _, _)| k.page == 3 && k.scale == 2000));
        assert!(t[0].1 == -184 && t.iter().all(|&(_, _, y)| y < 200));
    }

    #[test]
    fn only_intersecting_tiles_are_listed() {
        let g = geo((600.0, 20000.0), None, 600.0, 1.0);
        let all = tiles(0, (600.0, 20000.0), &g, 0.0, 0.0, 600, 100000).len();
        let top = tiles(0, (600.0, 20000.0), &g, 0.0, 0.0, 600, 300);
        assert!(top.len() < all && top.len() >= 1);
        assert_eq!(top[0].2, 0);
        let scrolled = tiles(0, (600.0, 20000.0), &g, 0.0, 10000.0, 600, 300);
        assert!(scrolled.iter().all(|&(_, _, y)| y < 300 && y > -7000));
        assert!(scrolled[0].0.tile > 0);
    }

    #[test]
    fn blit_is_clipped_to_the_pane() {
        let mut buf = vec![0u32; 100];
        let mut cv = Canvas { buf: &mut buf, w: 10, h: 10 };
        let src = vec![0xffff_ffffu32; 36];
        // a 6x6 tile starting left of the pane (x 4..10) and above it
        blit_clip(&mut cv, 6, 6, &src, 1, -2, (4, 0, 10, 10));
        let lit: Vec<usize> = (0..100).filter(|&i| buf[i] != 0).collect();
        assert!(lit.iter().all(|i| i % 10 >= 4 && i % 10 < 7 && i / 10 < 4), "{lit:?}");
        assert_eq!(lit.len(), 3 * 4);
    }
}
