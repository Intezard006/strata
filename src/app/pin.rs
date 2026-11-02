//! The pin pane (methods of `App`): a page or preview target in a right-hand split of the main window.

use super::*;
use crate::ui::pin::{self, Pin};

impl App {
    /// Width of the document view inside a window `win_w` wide (the pane takes the rest).
    pub(super) fn view_w(&self, win_w: u32) -> u32 {
        if self.pin.is_some() && !self.in_float {
            pin::split(win_w, self.cfg.pin_width).0
        } else {
            win_w
        }
    }

    pub(super) fn in_pane(&self) -> bool {
        self.pin.is_some() && !self.in_float && self.cursor.0 >= self.vp.0 as f64 && self.cursor.1 < self.doc_h() as f64
    }

    /// The pane's rectangle in window pixels.
    fn pane_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let win_w = self.window.as_ref()?.inner_size().width;
        Some((self.vp.0 as i32, 0, win_w as i32 - self.vp.0 as i32, self.doc_h() as i32))
    }

    pub(super) fn toggle_pin(&mut self) {
        if self.float_solo || self.present.is_some() {
            return;
        }
        if self.pin.take().is_some() {
            self.wanted_pin.clear();
            self.cache.set_pinned(2, &[]);
            self.sync_wanted();
            self.relayout_for_pin();
            self.cache.retain_scales(&[self.scale_key()].into_iter().chain(self.float_scale_key()).collect::<Vec<_>>());
            unsafe { libc::malloc_trim(0) };
            self.msg("Unpinned".into());
            return;
        }
        let target = self.popup.as_ref().and_then(|p| self.pv.as_ref().and_then(|pv| pv.targets.get(p.target as usize))).map(|t| (t.page as usize, t.rect));
        let (page, region) = match target {
            Some((p, r)) => (p, Some(r)),
            None => (self.current_page(), None),
        };
        self.close_popup();
        self.pin = Some(Pin::new(page, region));
        self.relayout_for_pin();
        self.msg(format!("Pinned page {} (Ctrl-w switches focus, gP unpins)", page + 1));
    }

    /// The window is split (or whole again): recompute the document view's width, keeping a fit-width zoom.
    fn relayout_for_pin(&mut self) {
        let Some(win_w) = self.window.as_ref().map(|w| w.inner_size().width) else { return };
        let p = self.current_page();
        let fit = (self.zoom - self.vp.0 as f32 / self.layout.row_size(p).0 / self.dpi).abs() < 0.003;
        self.vp.0 = self.view_w(win_w);
        if fit {
            self.set_zoom(self.vp.0 as f32 / self.layout.row_size(p).0 / self.dpi);
            self.sx = self.layout.row_span(p).0;
        }
        self.clamp_scroll();
        self.invalidate(false);
    }

    pub(super) fn pin_scale_key(&self) -> Option<u32> {
        let (p, r) = (self.pin.as_ref()?, self.pane_rect()?);
        Some(pin::geo(self.layout.size(p.page), p.region, r.2 as f32, p.mul).key)
    }

    /// Clamp the pane's scroll and ask for the tiles it shows (pinned against eviction, like the main view's).
    pub(super) fn pin_prepare(&mut self) {
        let Some(r) = self.pane_rect().filter(|_| !self.in_float) else { return };
        let wanted: Vec<TileKey> = {
            let Some(p) = self.pin.as_mut() else { return };
            if p.page >= self.layout.len() {
                p.page = self.layout.len() - 1;
            }
            let size = self.layout.size(p.page);
            let g = pin::geo(size, p.region, r.2 as f32, p.mul);
            let (mx, my) = g.max_scroll(r.2 as f32, r.3 as f32);
            p.sx = p.sx.clamp(0.0, mx);
            p.sy = p.sy.clamp(0.0, my);
            pin::tiles(p.page, size, &g, p.sx, p.sy, r.2, r.3).into_iter().map(|t| t.0).collect()
        };
        self.cache.set_pinned(2, &wanted);
        self.wanted_pin = wanted.clone();
        self.sync_wanted();
        let gen = self.render.gen.load(Ordering::Relaxed);
        let recolor = self.applied.recolor.with_enabled(self.cfg.recolor.enabled);
        for k in &wanted {
            if self.cache.wants(k) && self.pin.as_mut().is_some_and(|p| p.pending.insert(*k)) {
                let _ = self.render.req_tx.send(Cmd::Tile(Request { key: *k, gen, recolor }));
            }
        }
        for k in &wanted {
            self.cache.get(k);
        }
    }

    pub(super) fn pin_scroll(&mut self, dx: f32, dy: f32) {
        if let Some(p) = self.pin.as_mut() {
            p.sx += dx;
            p.sy += dy;
        }
    }

    /// Zoom the pane by `f`, keeping its centre.
    pub(super) fn pin_zoom(&mut self, f: f32) {
        let Some(r) = self.pane_rect() else { return };
        let Some(p) = self.pin.as_mut() else { return };
        let mul = (p.mul * f).clamp(pin::MIN_MUL, pin::MAX_MUL);
        let k = mul / p.mul;
        p.sx = (p.sx + r.2 as f32 / 2.0) * k - r.2 as f32 / 2.0;
        p.sy = (p.sy + r.3 as f32 / 2.0) * k - r.3 as f32 / 2.0;
        p.mul = mul;
    }

    pub(super) fn pin_wheel(&mut self, dx: f32, dy: f32, ctrl: bool) {
        if ctrl {
            let f = self.wheel_zoom_factor().powf(dy);
            self.pin_zoom(f);
        } else {
            let k = self.cfg.wheel_step * self.dpi;
            self.pin_scroll(-dx * k, -dy * k);
        }
    }

    /// Keys while the pane has focus. Always consumes the key.
    pub(super) fn pin_key(&mut self, token: &str, el: &ActiveEventLoop) {
        if token == "<Esc>" {
            self.keys.clear();
            if let Some(p) = self.pin.as_mut() {
                p.focus = false;
            }
            return;
        }
        let Some((cmd, count)) = self.keys.feed(&self.keymap, Mode::Normal, token) else { return };
        let n = count.unwrap_or(1).max(1);
        let step = self.cfg.scroll_step * self.dpi * n as f32;
        let ph = self.pane_rect().map_or(400.0, |r| r.3 as f32);
        match cmd {
            Command::ScrollDown => self.pin_scroll(0.0, step),
            Command::ScrollUp => self.pin_scroll(0.0, -step),
            Command::ScrollLeft => self.pin_scroll(-step, 0.0),
            Command::ScrollRight => self.pin_scroll(step, 0.0),
            Command::HalfPageDown => self.pin_scroll(0.0, ph / 2.0 * n as f32),
            Command::HalfPageUp => self.pin_scroll(0.0, -ph / 2.0 * n as f32),
            Command::NextPage | Command::PrevPage => {
                let d = if cmd == Command::NextPage { n as i64 } else { -(n as i64) };
                let len = self.layout.len() as i64;
                if let Some(p) = self.pin.as_mut().filter(|p| p.region.is_none()) {
                    p.page = (p.page as i64 + d).clamp(0, len - 1) as usize;
                    p.sy = 0.0;
                }
            }
            Command::ZoomIn => self.pin_zoom(self.zoom_factor(n as i32)),
            Command::ZoomOut => self.pin_zoom(1.0 / self.zoom_factor(n as i32)),
            Command::ZoomReset | Command::FitWidth | Command::FitPage => {
                if let Some(p) = self.pin.as_mut() {
                    (p.mul, p.sx, p.sy) = (1.0, 0.0, 0.0);
                }
            }
            other => self.exec(other, count, el),
        }
    }

    pub(super) fn pin_focus_toggle(&mut self) {
        match self.pin.as_mut() {
            Some(p) => {
                p.focus = !p.focus;
                let f = p.focus;
                self.msg(if f { "Pane focus (Ctrl-w or Esc returns)".into() } else { "Document focus".into() });
            }
            None => self.err("Nothing pinned (gP pins the page)".into()),
        }
    }
}
