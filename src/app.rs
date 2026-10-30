//! winit handler and view state. Everything is event driven (`ControlFlow::Wait`).

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use crate::keys::{Command, KeyState, Mode};
use crate::layout::{page_px, tile_grid, Layout};
use crate::recolor::Recolor;
use crate::render::{RenderHandle, Request, Response, TileCache, TileKey, DEFAULT_CACHE_BYTES};

/// Window background #1e1e2e at alpha 0x99, premultiplied.
const BG: u32 = {
    const fn c(v: u32) -> u32 {
        (v * 0x99 + 127) / 255
    }
    (0x99 << 24) | (c(0x1e) << 16) | (c(0x1e) << 8) | c(0x2e)
};
const ZOOM_STEP: f32 = 1.1;
const LINE_PX: f32 = 40.0; // logical px per `j`
const WHEEL_PX: f32 = 50.0; // logical px per wheel notch
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;

pub struct App {
    render: RenderHandle,
    layout: Layout,
    name: String,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    cache: TileCache,
    pending: HashSet<TileKey>,
    recolor: Recolor,
    keys: KeyState,
    mods: ModifiersState,
    zoom: f32,
    dpi: f32,
    vp: (u32, u32),
    /// Scroll offset in points; `sx` only matters when the content is wider than the view.
    sx: f32,
    sy: f32,
    title_page: usize,
}

struct Placed {
    key: TileKey,
    x: i64,
    y: i64,
}

impl App {
    pub fn new(render: RenderHandle, layout: Layout, name: String) -> Self {
        App {
            render,
            layout,
            name,
            window: None,
            surface: None,
            cache: TileCache::new(DEFAULT_CACHE_BYTES),
            pending: HashSet::new(),
            recolor: Recolor::default(),
            keys: KeyState::default(),
            mods: ModifiersState::empty(),
            zoom: 1.0,
            dpi: 1.0,
            vp: (0, 0),
            sx: 0.0,
            sy: 0.0,
            title_page: usize::MAX,
        }
    }

    fn scale_key(&self) -> u32 {
        ((self.zoom * self.dpi * 1000.0).round() as u32).max(1)
    }

    fn scale(&self) -> f32 {
        self.scale_key() as f32 / 1000.0
    }

    fn vp_pt(&self) -> (f32, f32) {
        let s = self.scale();
        (self.vp.0 as f32 / s, self.vp.1 as f32 / s)
    }

    /// Left edge of the view in content coordinates (centered when content is narrower).
    fn x0(&self) -> f32 {
        let vw = self.vp_pt().0;
        let mw = self.layout.max_w;
        if mw <= vw { -(vw - mw) / 2.0 } else { self.sx.clamp(0.0, mw - vw) }
    }

    fn clamp_scroll(&mut self) {
        let (vw, vh) = self.vp_pt();
        self.sy = self.sy.clamp(0.0, (self.layout.total_h - vh).max(0.0));
        self.sx = self.sx.clamp(0.0, (self.layout.max_w - vw).max(0.0));
    }

    fn current_page(&self) -> usize {
        self.layout.page_at(self.sy + self.vp_pt().1 / 2.0)
    }

    fn invalidate(&mut self) {
        self.render.gen.fetch_add(1, Ordering::Relaxed);
        self.cache.clear();
        self.pending.clear();
    }

    fn set_zoom(&mut self, z: f32) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - self.zoom).abs() < 1e-6 {
            return;
        }
        let (vw, vh) = self.vp_pt();
        let (cx, cy) = (self.x0() + vw / 2.0, self.sy + vh / 2.0);
        self.zoom = z;
        let (vw, vh) = self.vp_pt();
        self.sx = cx - vw / 2.0;
        self.sy = cy - vh / 2.0;
        self.clamp_scroll();
        self.invalidate();
    }

    fn goto_page(&mut self, p: usize) {
        self.sy = self.layout.top(p.min(self.layout.len() - 1));
        self.clamp_scroll();
    }

    fn scroll_by(&mut self, dx_px: f32, dy_px: f32) {
        let s = self.scale();
        self.sx += dx_px / s;
        self.sy += dy_px / s;
        self.clamp_scroll();
    }

    fn exec(&mut self, cmd: Command, count: Option<u32>, el: &ActiveEventLoop) {
        let n = count.unwrap_or(1).max(1);
        let logical = self.dpi; // logical px -> physical px
        match cmd {
            Command::ScrollDown => self.scroll_by(0.0, LINE_PX * logical * n as f32),
            Command::ScrollUp => self.scroll_by(0.0, -LINE_PX * logical * n as f32),
            Command::ScrollLeft => self.scroll_by(-LINE_PX * logical * n as f32, 0.0),
            Command::ScrollRight => self.scroll_by(LINE_PX * logical * n as f32, 0.0),
            Command::HalfPageDown => self.scroll_by(0.0, self.vp.1 as f32 / 2.0 * n as f32),
            Command::HalfPageUp => self.scroll_by(0.0, -(self.vp.1 as f32) / 2.0 * n as f32),
            Command::NextPage => self.goto_page((self.current_page() + n as usize).min(self.layout.len() - 1)),
            Command::PrevPage => self.goto_page(self.current_page().saturating_sub(n as usize)),
            Command::GotoFirst => self.goto_page(count.map_or(0, |c| c.max(1) as usize - 1)),
            Command::GotoLast => self.goto_page(count.map_or(self.layout.len() - 1, |c| c.max(1) as usize - 1)),
            Command::ZoomIn => self.set_zoom(self.zoom * ZOOM_STEP.powi(n as i32)),
            Command::ZoomOut => self.set_zoom(self.zoom / ZOOM_STEP.powi(n as i32)),
            Command::ZoomReset => self.set_zoom(1.0),
            Command::FitWidth => {
                let p = self.current_page();
                self.set_zoom(self.vp.0 as f32 / self.layout.size(p).0 / self.dpi);
                self.goto_page(p);
            }
            Command::FitPage => {
                let p = self.current_page();
                let (w, h) = self.layout.size(p);
                self.set_zoom((self.vp.0 as f32 / w).min(self.vp.1 as f32 / h) / self.dpi);
                self.goto_page(p);
            }
            Command::ToggleRecolor => {
                self.recolor.enabled = !self.recolor.enabled;
                self.invalidate();
            }
            Command::Quit => el.exit(),
        }
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Tiles intersecting the view, with their screen positions.
    fn visible_tiles(&self) -> Vec<Placed> {
        let s = self.scale();
        let (vw_pt, vh_pt) = self.vp_pt();
        let _ = vw_pt;
        let (x0, vh) = (self.x0(), self.vp.1 as i64);
        let mut out = Vec::new();
        for p in self.layout.visible(self.sy, self.sy + vh_pt) {
            let (w, h) = page_px(self.layout.size(p), s);
            let (strip, n) = tile_grid(w, h);
            let px = ((self.layout.left(p) - x0) * s).round() as i64;
            let py = ((self.layout.top(p) - self.sy) * s).round() as i64;
            for t in 0..n {
                let ty = py + (t * strip) as i64;
                let th = (strip.min(h - t * strip)) as i64;
                if ty < vh && ty + th > 0 {
                    out.push(Placed { key: TileKey { page: p as u32, scale: self.scale_key(), tile: t }, x: px, y: ty });
                }
            }
        }
        out
    }

    fn tile_count(&self, page: usize) -> u32 {
        let (w, h) = page_px(self.layout.size(page), self.scale());
        tile_grid(w, h).1
    }

    /// One tile before the first and after the last visible tile.
    fn neighbours(&self, vis: &[Placed]) -> Vec<TileKey> {
        let (Some(first), Some(last)) = (vis.first(), vis.last()) else { return vec![] };
        let mut out = Vec::new();
        let (p, t) = (first.key.page, first.key.tile);
        if t > 0 {
            out.push(TileKey { tile: t - 1, ..first.key });
        } else if p > 0 {
            let pp = p - 1;
            out.push(TileKey { page: pp, tile: self.tile_count(pp as usize) - 1, ..first.key });
        }
        let (p, t) = (last.key.page, last.key.tile);
        if t + 1 < self.tile_count(p as usize) {
            out.push(TileKey { tile: t + 1, ..last.key });
        } else if (p as usize) + 1 < self.layout.len() {
            out.push(TileKey { page: p + 1, tile: 0, ..last.key });
        }
        out
    }

    fn paint(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let size = window.inner_size();
        self.vp = (size.width, size.height);
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        self.clamp_scroll();

        let vis = self.visible_tiles();
        let mut wanted: Vec<TileKey> = vis.iter().map(|p| p.key).collect();
        wanted.extend(self.neighbours(&vis));
        *self.render.wanted.lock().unwrap() = wanted.clone();
        let gen = self.render.gen.load(Ordering::Relaxed);
        for k in &wanted {
            if !self.cache.contains(k) && self.pending.insert(*k) {
                let _ = self.render.req_tx.send(Request { key: *k, gen, recolor: self.recolor });
            }
        }
        // Touch neighbours so they outlive less relevant entries.
        for k in &wanted {
            self.cache.get(k);
        }

        let Some(surface) = self.surface.as_mut() else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        buf.fill(BG);
        let (bw, bh) = (size.width as i64, size.height as i64);
        for p in &vis {
            if let Some(tile) = self.cache.get(&p.key) {
                let under = !self.recolor.enabled;
                blit(&mut buf, bw, bh, tile.w as i64, tile.h as i64, &tile.data, p.x, p.y, under);
            }
        }
        let _ = window.pre_present_notify();
        let _ = buf.present();

        let page = self.current_page();
        if page != self.title_page {
            self.title_page = page;
            window.set_title(&format!("strata — {} [{}/{}]", self.name, page + 1, self.layout.len()));
        }
    }
}

/// Source-over blit of premultiplied pixels, clipped to the buffer. `under` first lays down opaque white.
fn blit(buf: &mut [u32], bw: i64, bh: i64, tw: i64, th: i64, src: &[u32], dx: i64, dy: i64, under: bool) {
    let (cx0, cx1) = (dx.max(0), (dx + tw).min(bw));
    let (cy0, cy1) = (dy.max(0), (dy + th).min(bh));
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }
    for y in cy0..cy1 {
        let s_row = &src[((y - dy) * tw + (cx0 - dx)) as usize..][..(cx1 - cx0) as usize];
        let d_row = &mut buf[(y * bw + cx0) as usize..][..(cx1 - cx0) as usize];
        for (d, &s) in d_row.iter_mut().zip(s_row) {
            *d = over(s, if under { 0xffff_ffff } else { *d });
        }
    }
}

#[inline]
fn over(s: u32, d: u32) -> u32 {
    let sa = s >> 24;
    match sa {
        255 => s,
        0 => d,
        _ => {
            let inv = 255 - sa;
            let ch = |sh: u32| {
                let dc = (d >> sh) & 0xff;
                let v = ((s >> sh) & 0xff) + (dc * inv + 127) / 255;
                v.min(255) << sh
            };
            ch(24) | ch(16) | ch(8) | ch(0)
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("strata — {}", self.name))
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(900.0, 1000.0))
            .with_name("strata", "strata");
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let ctx = Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(Surface::new(&ctx, window.clone()).expect("softbuffer surface"));
        self.dpi = window.scale_factor() as f32;
        let sz = window.inner_size();
        self.vp = (sz.width, sz.height);
        self.window = Some(window);
        self.exec(Command::FitWidth, None, el);
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _: ()) {
        let cur = self.render.gen.load(Ordering::Relaxed);
        let mut changed = false;
        while let Ok(r) = self.render.resp_rx.try_recv() {
            match r {
                Response::Tile { key, gen, tile } if gen == cur => {
                    self.pending.remove(&key);
                    self.cache.insert(key, tile);
                    changed = true;
                }
                Response::Skipped { key } => {
                    self.pending.remove(&key);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(sz) => {
                self.vp = (sz.width, sz.height);
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.dpi = scale_factor as f32;
                self.invalidate();
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.mods.control_key();
                let token = match &event.logical_key {
                    Key::Named(NamedKey::Space) if self.mods.shift_key() => "<S-Space>".to_string(),
                    Key::Named(NamedKey::Space) => "<Space>".to_string(),
                    Key::Named(NamedKey::Escape) => {
                        self.keys.clear();
                        return;
                    }
                    Key::Character(c) if ctrl => format!("<C-{}>", c.to_lowercase()),
                    Key::Character(c) => c.to_string(),
                    _ => return,
                };
                if let Some((cmd, count)) = self.keys.feed(Mode::Normal, &token) {
                    self.exec(cmd, count, el);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / (WHEEL_PX * self.dpi), p.y as f32 / (WHEEL_PX * self.dpi)),
                };
                if self.mods.control_key() {
                    self.set_zoom(self.zoom * ZOOM_STEP.powf(dy));
                } else {
                    self.scroll_by(-dx * WHEEL_PX * self.dpi, -dy * WHEEL_PX * self.dpi);
                }
                self.redraw();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_blends_premultiplied() {
        assert_eq!(over(0, 0x1234_5678), 0x1234_5678);
        assert_eq!(over(0xff01_0203, 0x1234_5678), 0xff01_0203);
        // half-alpha white over opaque black
        assert_eq!(over(0x8080_8080, 0xff00_0000) >> 24, 0xff);
        assert_eq!((over(0x8080_8080, 0xff00_0000) & 0xff), 0x80);
    }

    #[test]
    fn blit_clips() {
        let mut buf = vec![0u32; 4 * 4];
        let src = vec![0xffff_ffffu32; 3 * 3];
        blit(&mut buf, 4, 4, 3, 3, &src, -1, 2, false);
        assert_eq!(buf.iter().filter(|&&p| p != 0).count(), 2 * 2);
        blit(&mut buf, 4, 4, 3, 3, &src, 10, 10, false);
    }
}
