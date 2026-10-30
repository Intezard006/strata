//! Bottom bar: filename, page, zoom, mode, pending keys, messages.

use super::{pm, Canvas, DIM, ERR, FG};
use crate::text::TextRenderer;

pub struct Status<'a> {
    pub name: &'a str,
    pub message: Option<(&'a str, bool)>,
    pub page: usize,
    /// Printed page label when the book has a page map.
    pub printed: Option<&'a str>,
    /// Active look profile when it is not `normal`.
    pub profile: Option<&'a str>,
    pub total: usize,
    pub zoom_pct: u32,
    pub mode: &'a str,
    pub pending: &'a str,
    /// Search position `(i, n)`, 1-based.
    /// Background job text, e.g. `analysing 120/552`.
    pub job: Option<&'a str>,
    pub search: Option<(usize, usize, bool)>,
}

pub fn height(t: &TextRenderer) -> i32 {
    t.line_h + 6
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, s: &Status) {
    let h = height(t);
    let y = cv.h - h;
    cv.fill_rect(0, y, cv.w, h, pm([0x11, 0x11, 0x1b], 0xcc));
    let ty = y + 3;
    let cw = t.width("M").max(1);
    let mut right = String::new();
    if !s.pending.is_empty() {
        right.push_str(&format!("{}  ", s.pending));
    }
    if let Some((i, n, done)) = s.search {
        right.push_str(&format!("[{i}/{n}{}]  ", if done { "" } else { "+" }));
    }
    if let Some(j) = s.job {
        right.push_str(&format!("{j}  "));
    }
    if let Some(p) = s.profile {
        right.push_str(&format!("[{p}]  "));
    }
    if let Some(p) = s.printed {
        right.push_str(&format!("p.{p} "));
    }
    right.push_str(&format!("[{}/{}]  {}%  {}", s.page, s.total, s.zoom_pct, s.mode));
    let rw = t.width(&right);
    t.draw(cv, cv.w - rw - cw, ty, &right, DIM, rw + cw);
    let (left, color) = match s.message {
        Some((m, err)) => (m, if err { ERR } else { FG }),
        None => (s.name, FG),
    };
    t.draw(cv, cw, ty, left, color, (cv.w - rw - 3 * cw).max(0));
}
