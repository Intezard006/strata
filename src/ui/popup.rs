//! The preview popup: a clipped render of a target inside a bordered panel.

use super::{pm, Canvas, ACCENT, DIM, FG};
use crate::render::Tile;
use crate::text::TextRenderer;

pub struct Popup {
    pub target: u32,
    /// Render request this popup is waiting for.
    pub id: u64,
    pub tile: Option<Tile>,
    /// Multiplier on the fitted scale (`+`/`-`).
    pub zoom: f32,
    /// Vertical scroll in pixels for tall targets.
    pub scroll: i32,
    /// Opened by hovering (closes when the mouse leaves); otherwise by keyboard.
    pub hover: bool,
    pub anchor: (i32, i32),
    /// The reference this popup explains, in window pixels (x0, y0, x1, y1); the panel avoids covering it.
    pub ref_rect: Option<(i32, i32, i32, i32)>,
    pub title: String,
    /// A note's text (highlight notes); such a popup has no tile.
    pub note: Option<String>,
    /// Last drawn panel rectangle (x, y, w, h), for hit testing.
    pub rect: (i32, i32, i32, i32),
}

impl Popup {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (px, py, w, h) = self.rect;
        x >= px as f64 && x < (px + w) as f64 && y >= py as f64 && y < (py + h) as f64
    }

    pub fn scroll_by(&mut self, d: i32) {
        self.scroll = (self.scroll + d).max(0);
    }
}

/// Hover state machine: the only timer in the program. `poll` says what the event loop should do.
#[derive(Default)]
pub struct HoverTimer {
    pub key: Option<(u32, usize)>,
    deadline: Option<std::time::Instant>,
}

#[derive(Debug, PartialEq)]
pub enum Poll {
    /// Nothing pending: `ControlFlow::Wait`.
    Idle,
    /// Sleep until this instant (`WaitUntil`).
    Wait(std::time::Instant),
    Fire,
}

impl HoverTimer {
    /// The pointer now rests on `key` (or on nothing). Returns true when that changed.
    pub fn moved(&mut self, key: Option<(u32, usize)>, now: std::time::Instant, delay: std::time::Duration) -> bool {
        if key == self.key {
            return false;
        }
        self.key = key;
        self.deadline = key.map(|_| now + delay);
        true
    }

    pub fn cancel(&mut self) {
        self.key = None;
        self.deadline = None;
    }

    pub fn disarm(&mut self) {
        self.deadline = None;
    }

    pub fn poll(&mut self, now: std::time::Instant) -> Poll {
        match self.deadline {
            None => Poll::Idle,
            Some(d) if now >= d => {
                self.deadline = None;
                Poll::Fire
            }
            Some(d) => Poll::Wait(d),
        }
    }
}

const PAD: i32 = 10;
const GAP: i32 = 8;

/// Panel size for a tile inside a window of `vw` x `vh`.
pub fn panel_size(tile: Option<&Tile>, vw: i32, vh: i32, zoom: f32, line_h: i32) -> (i32, i32) {
    let max_w = ((0.6 * vw as f32 * zoom.max(1.0)) as i32).min(vw * 95 / 100);
    let max_h = vh * 90 / 100;
    let (tw, th) = tile.map_or((vw / 4, line_h * 3), |t| (t.w as i32, t.h as i32));
    ((tw + 2 * PAD).min(max_w), (th + 2 * PAD + line_h + 4).min(max_h))
}

/// Panel position: below the reference when it fits, else above, else wherever it fits best; never
/// over the reference line. Without a reference (list popups) it is centred.
pub fn place(w: i32, h: i32, vw: i32, vh: i32, rr: Option<(i32, i32, i32, i32)>, anchor: (i32, i32)) -> (i32, i32) {
    let clampx = |x: i32| x.min(vw - w).max(0);
    match rr {
        Some((x0, y0, _x1, y1)) => {
            let x = clampx(x0 - PAD);
            let below = y1 + GAP;
            let above = y0 - GAP - h;
            if below + h <= vh {
                (x, below)
            } else if above >= 0 {
                (x, above)
            } else if vh - y1 >= y0 {
                (x, (vh - h).max(y1 + 1).min(vh - h))
            } else {
                (x, 0)
            }
        }
        None if anchor != (0, 0) => (clampx(anchor.0 + 12), (anchor.1 + 14).min(vh - h).max(0)),
        None => ((vw - w) / 2, ((vh - h) / 2).max(0)),
    }
}

/// Greedy word wrap to `max_w` pixels; explicit newlines are kept.
pub fn wrap(t: &mut TextRenderer, s: &str, max_w: i32) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && t.width(&trial) > max_w {
                out.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = trial;
            }
        }
        out.push(cur);
    }
    out
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, p: &mut Popup, bottom_reserved: i32, bg: [u8; 4]) {
    let vh = cv.h - bottom_reserved;
    let note_lines = p.note.as_deref().map(|n| wrap(t, n, (cv.w * 45 / 100).max(220) - 2 * PAD));
    let (w, h) = match &note_lines {
        Some(ls) => {
            let widest = ls.iter().map(|l| t.width(l)).max().unwrap_or(0).max(t.width(&p.title));
            ((widest + 2 * PAD + 8).min(cv.w * 95 / 100), (ls.len() as i32 * t.line_h + t.line_h + 6 + PAD + 4).min(vh * 80 / 100))
        }
        None => panel_size(p.tile.as_ref(), cv.w, vh, p.zoom, t.line_h),
    };
    let (x, y) = place(w, h, cv.w, vh, p.ref_rect, p.anchor);
    p.rect = (x, y, w, h);
    // opaque base first, so nothing of the page shows through, then the (nearly opaque) panel colour
    cv.fill_rect(x, y, w, h, pm([bg[0] / 2, bg[1] / 2, bg[2] / 2], 0xff));
    cv.fill_rect(x, y, w, h, pm([bg[0], bg[1], bg[2]], bg[3]));
    for (bx, by, bw, bh) in [(x, y, w, 1), (x, y + h - 1, w, 1), (x, y, 1, h), (x + w - 1, y, 1, h)] {
        cv.fill_rect(bx, by, bw, bh, pm(ACCENT, 0xff));
    }
    t.draw(cv, x + PAD, y + 3, &p.title, ACCENT, w - 2 * PAD);
    let body_y = y + t.line_h + 6;
    let body_h = h - (t.line_h + 6) - 1;
    if let Some(ls) = note_lines {
        let max_scroll = (ls.len() as i32 * t.line_h + PAD - body_h).max(0);
        p.scroll = p.scroll.min(max_scroll);
        for (i, l) in ls.iter().enumerate() {
            let ly = body_y + PAD / 2 + i as i32 * t.line_h - p.scroll;
            if ly >= body_y && ly + t.line_h <= body_y + body_h {
                t.draw(cv, x + PAD, ly, l, FG, w - 2 * PAD);
            }
        }
        return;
    }
    match &p.tile {
        Some(tile) if tile.w > 0 => {
            let max_scroll = (tile.h as i32 + PAD - body_h).max(0);
            p.scroll = p.scroll.min(max_scroll);
            // clip to the body area by blitting into a sub-canvas row range
            let clip_bottom = body_y + body_h;
            cv.blit(tile.w as i64, tile.h as i64, &tile.data, (x + PAD) as i64, (body_y - p.scroll + PAD / 2) as i64, clip_bottom as i64, false);
            if max_scroll > 0 {
                let frac = p.scroll as f32 / max_scroll as f32;
                let bar_h = (body_h as f32 * body_h as f32 / (tile.h as f32 + PAD as f32)).max(14.0) as i32;
                cv.fill_rect(x + w - 6, body_y, 3, body_h, pm(DIM, 0x40));
                cv.fill_rect(x + w - 6, body_y + ((body_h - bar_h) as f32 * frac) as i32, 3, bar_h, pm(ACCENT, 0xe0));
                if p.scroll < max_scroll {
                    let sh = t.line_h + 2;
                    cv.fill_rect(x + 1, y + h - sh - 1, w - 2, sh, pm([bg[0] / 2, bg[1] / 2, bg[2] / 2], 0xff));
                    t.draw(cv, x + PAD, y + h - sh, "v more  (j/k or wheel to scroll)", ACCENT, w - 2 * PAD);
                }
            }
        }
        Some(_) => {
            t.draw(cv, x + PAD, body_y + 2, "(nothing to show)", DIM, w - 2 * PAD);
        }
        None => {
            t.draw(cv, x + PAD, body_y + 2, "rendering...", FG, w - 2 * PAD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_timer_arms_only_while_pending() {
        use std::time::{Duration, Instant};
        let (t0, d) = (Instant::now(), Duration::from_millis(300));
        let mut h = HoverTimer::default();
        assert_eq!(h.poll(t0), Poll::Idle, "no timer without a hover");
        assert!(h.moved(Some((3, 1)), t0, d));
        assert_eq!(h.poll(t0 + Duration::from_millis(100)), Poll::Wait(t0 + d));
        assert!(!h.moved(Some((3, 1)), t0 + Duration::from_millis(150), d), "same ref keeps its deadline");
        assert_eq!(h.poll(t0 + d), Poll::Fire);
        assert_eq!(h.poll(t0 + d), Poll::Idle, "fires once, then back to Wait");
        assert!(h.moved(Some((3, 2)), t0 + d, d));
        assert!(h.moved(None, t0 + d, d), "moving away cancels");
        assert_eq!(h.poll(t0 + d * 5), Poll::Idle);
    }

    #[test]
    fn panel_is_capped_to_the_window() {
        let tile = Tile { w: 2000, h: 3000, data: vec![] };
        let (w, h) = panel_size(Some(&tile), 1000, 800, 1.0, 16);
        assert_eq!((w, h), (600, 720));
        let (w, _) = panel_size(Some(&tile), 1000, 800, 2.0, 16);
        assert_eq!(w, 950);
        let small = Tile { w: 100, h: 50, data: vec![] };
        assert_eq!(panel_size(Some(&small), 1000, 800, 1.0, 16).0, 120);
    }
}
