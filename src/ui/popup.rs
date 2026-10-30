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
    pub title: String,
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

const PAD: i32 = 8;

/// Panel size for a tile inside a window of `vw` x `vh`.
pub fn panel_size(tile: Option<&Tile>, vw: i32, vh: i32, zoom: f32, line_h: i32) -> (i32, i32) {
    let max_w = ((0.6 * vw as f32 * zoom.max(1.0)) as i32).min(vw * 95 / 100);
    let max_h = ((0.6 * vh as f32 * zoom.max(1.0)) as i32).min(vh * 90 / 100);
    let (tw, th) = tile.map_or((vw / 4, line_h * 3), |t| (t.w as i32, t.h as i32));
    ((tw + 2 * PAD).min(max_w), (th + 2 * PAD + line_h + 4).min(max_h))
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, p: &mut Popup) {
    let (w, h) = panel_size(p.tile.as_ref(), cv.w, cv.h, p.zoom, t.line_h);
    let (mut x, mut y) = if p.hover {
        (p.anchor.0 + 12, p.anchor.1 + 14)
    } else {
        ((cv.w - w) / 2, (cv.h - h) / 2)
    };
    if x + w > cv.w {
        x = (p.anchor.0 - w - 12).max(0).min(cv.w - w);
        if p.hover && x < p.anchor.0 && x + w > p.anchor.0 {
            x = (cv.w - w).max(0);
        }
    }
    if y + h > cv.h {
        y = (p.anchor.1 - h - 14).max(0);
        if y + h > cv.h {
            y = (cv.h - h).max(0);
        }
    }
    p.rect = (x, y, w, h);
    cv.fill_rect(x, y, w, h, pm([0x1e, 0x1e, 0x2e], 0xe6));
    for (bx, by, bw, bh) in [(x, y, w, 1), (x, y + h - 1, w, 1), (x, y, 1, h), (x + w - 1, y, 1, h)] {
        cv.fill_rect(bx, by, bw, bh, pm(ACCENT, 0xff));
    }
    t.draw(cv, x + PAD, y + 3, &p.title, ACCENT, w - 2 * PAD);
    let body_y = y + t.line_h + 6;
    let body_h = h - (t.line_h + 6) - 1;
    match &p.tile {
        Some(tile) if tile.w > 0 => {
            let max_scroll = (tile.h as i32 - body_h + PAD).max(0);
            p.scroll = p.scroll.min(max_scroll);
            // clip to the body area by blitting into a sub-canvas row range
            let clip_bottom = body_y + body_h;
            cv.blit(tile.w as i64, tile.h as i64, &tile.data, (x + PAD) as i64, (body_y - p.scroll + PAD / 2) as i64, clip_bottom as i64, false);
            if max_scroll > 0 {
                let frac = p.scroll as f32 / max_scroll as f32;
                let bar_h = (body_h as f32 * body_h as f32 / (tile.h as f32 + PAD as f32)).max(12.0) as i32;
                cv.fill_rect(x + w - 4, body_y + ((body_h - bar_h) as f32 * frac) as i32, 3, bar_h, pm(DIM, 0xa0));
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
        assert_eq!((w, h), (600, 480));
        let (w, _) = panel_size(Some(&tile), 1000, 800, 2.0, 16);
        assert_eq!(w, 950);
        let small = Tile { w: 100, h: 50, data: vec![] };
        assert_eq!(panel_size(Some(&small), 1000, 800, 1.0, 16).0, 116);
    }
}
