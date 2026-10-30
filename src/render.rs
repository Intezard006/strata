//! Renderer thread (owns the MuPDF document) and the byte-capped tile cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use mupdf::{Colorspace, Device, Document, IRect, Matrix, Pixmap};
use winit::event_loop::EventLoopProxy;

use crate::layout::{page_px, tile_grid};
use crate::recolor::Recolor;

pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Shrink MuPDF's internal store to this fraction (percent) after each render.
const STORE_SHRINK_PERCENT: u32 = 10;

/// `scale` is px-per-point in thousandths, so keys hash exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub page: u32,
    pub scale: u32,
    pub tile: u32,
}

impl TileKey {
    pub fn scale_f(&self) -> f32 {
        self.scale as f32 / 1000.0
    }
}

pub struct Tile {
    pub w: u32,
    pub h: u32,
    /// Premultiplied 0xAARRGGBB.
    pub data: Vec<u32>,
}

impl Tile {
    fn bytes(&self) -> usize {
        self.data.len() * 4
    }
}

/// LRU cache bounded by total pixel bytes. The most recently used entry is never evicted.
pub struct TileCache {
    map: HashMap<TileKey, (Tile, u64)>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl TileCache {
    pub fn new(cap: usize) -> Self {
        TileCache { map: HashMap::new(), bytes: 0, cap, tick: 0 }
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn contains(&self, k: &TileKey) -> bool {
        self.map.contains_key(k)
    }

    /// Fetch and mark as most recently used.
    pub fn get(&mut self, k: &TileKey) -> Option<&Tile> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.1 = t;
            &e.0
        })
    }

    pub fn insert(&mut self, k: TileKey, tile: Tile) {
        self.tick += 1;
        self.bytes += tile.bytes();
        if let Some((old, _)) = self.map.insert(k, (tile, self.tick)) {
            self.bytes -= old.bytes();
        }
        while self.bytes > self.cap && self.map.len() > 1 {
            let victim = self.map.iter().filter(|(kk, _)| **kk != k).min_by_key(|(_, e)| e.1).map(|(kk, _)| *kk);
            match victim {
                Some(v) => self.bytes -= self.map.remove(&v).unwrap().0.bytes(),
                None => break,
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

pub struct Request {
    pub key: TileKey,
    pub gen: u64,
    pub recolor: Recolor,
}

pub enum Response {
    Tile { key: TileKey, gen: u64, tile: Tile },
    /// The renderer discarded the request because it was no longer wanted.
    Skipped { key: TileKey },
}

pub struct RenderHandle {
    pub req_tx: Sender<Request>,
    pub resp_rx: Receiver<Response>,
    /// Tiles the UI currently cares about, highest priority first.
    pub wanted: Arc<Mutex<Vec<TileKey>>>,
    /// Requests with an older generation are dropped.
    pub gen: Arc<AtomicU64>,
}

/// Open the document on a new thread; returns page sizes (points) once loaded.
pub fn spawn(path: PathBuf, proxy: EventLoopProxy<()>) -> Result<(RenderHandle, Vec<(f32, f32)>), String> {
    let (req_tx, req_rx) = channel::<Request>();
    let (resp_tx, resp_rx) = channel::<Response>();
    let (open_tx, open_rx) = channel::<Result<Vec<(f32, f32)>, String>>();
    let wanted = Arc::new(Mutex::new(Vec::new()));
    let gen = Arc::new(AtomicU64::new(0));
    let (w2, g2) = (wanted.clone(), gen.clone());
    thread::Builder::new()
        .name("renderer".into())
        .spawn(move || renderer(path, req_rx, resp_tx, open_tx, proxy, w2, g2))
        .map_err(|e| e.to_string())?;
    let sizes = open_rx.recv().map_err(|_| "renderer thread died".to_string())??;
    Ok((RenderHandle { req_tx, resp_rx, wanted, gen }, sizes))
}

fn open(path: &PathBuf) -> Result<(Document, Vec<(f32, f32)>), String> {
    let mut doc = Document::open(path.as_path()).map_err(|e| e.to_string())?;
    if doc.needs_password().map_err(|e| e.to_string())? {
        return Err("document is password protected".into());
    }
    if doc.is_reflowable().unwrap_or(false) {
        doc.layout(600.0, 800.0, 12.0).map_err(|e| e.to_string())?;
    }
    let n = doc.page_count().map_err(|e| e.to_string())?;
    if n <= 0 {
        return Err("document has no pages".into());
    }
    let mut sizes = Vec::with_capacity(n as usize);
    for i in 0..n {
        let b = doc.load_page(i).and_then(|p| p.bounds()).map_err(|e| e.to_string())?;
        sizes.push((b.width().max(1.0), b.height().max(1.0)));
    }
    Ok((doc, sizes))
}

/// The mupdf crate hardcodes a 256 MB store and does not expose the context, so we
/// recover the raw pointer from its Debug output once and shrink the store ourselves.
fn context_ptr() -> Option<*mut mupdf_sys::fz_context> {
    let s = format!("{:?}", mupdf::Context::get());
    let hex = s.split("0x").nth(1)?.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
    usize::from_str_radix(hex, 16).ok().map(|p| p as *mut _)
}

fn renderer(
    path: PathBuf,
    rx: Receiver<Request>,
    tx: Sender<Response>,
    open_tx: Sender<Result<Vec<(f32, f32)>, String>>,
    proxy: EventLoopProxy<()>,
    wanted: Arc<Mutex<Vec<TileKey>>>,
    gen: Arc<AtomicU64>,
) {
    let (doc, sizes) = match open(&path) {
        Ok(v) => v,
        Err(e) => {
            let _ = open_tx.send(Err(e));
            return;
        }
    };
    let ctx = context_ptr();
    let _ = open_tx.send(Ok(sizes.clone()));
    let cs = Colorspace::device_rgb();
    let mut queue: Vec<Request> = Vec::new();
    let mut page_cache: Option<(u32, mupdf::Page)> = None;

    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(r) => queue.push(r),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());
        let order = wanted.lock().unwrap().clone();
        let rank = |k: &TileKey| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
        let i = (0..queue.len()).min_by_key(|&i| rank(&queue[i].key)).unwrap();
        let req = queue.swap_remove(i);

        let cur = gen.load(Ordering::Relaxed);
        if req.gen != cur {
            continue; // UI already forgot about it
        }
        if rank(&req.key) == usize::MAX {
            let _ = tx.send(Response::Skipped { key: req.key });
            let _ = proxy.send_event(());
            continue;
        }
        let k = req.key;
        if page_cache.as_ref().map(|p| p.0) != Some(k.page) {
            page_cache = doc.load_page(k.page as i32).ok().map(|p| (k.page, p));
        }
        // A failed render becomes an empty tile so the UI does not re-request it forever.
        let tile = page_cache
            .as_ref()
            .and_then(|(_, page)| {
                let b = page.bounds().ok()?;
                render_tile(page, b.x0, b.y0, sizes[k.page as usize], k, &req.recolor, &cs)
            })
            .unwrap_or(Tile { w: 0, h: 0, data: Vec::new() });
        let _ = tx.send(Response::Tile { key: k, gen: req.gen, tile });
        if let Some(c) = ctx {
            unsafe { mupdf_sys::fz_shrink_store(c, STORE_SHRINK_PERCENT) };
        }
        let _ = proxy.send_event(());
    }
}

fn render_tile(
    page: &mupdf::Page,
    x0: f32,
    y0: f32,
    size: (f32, f32),
    k: TileKey,
    rc: &Recolor,
    cs: &Colorspace,
) -> Option<Tile> {
    let s = k.scale_f();
    let (w, h) = page_px(size, s);
    let (strip, n) = tile_grid(w, h);
    if k.tile >= n {
        return None;
    }
    let ty0 = (k.tile * strip) as i32;
    let ty1 = ((k.tile + 1) * strip).min(h) as i32;
    let mut pix = Pixmap::new_with_rect(cs, IRect::new(0, ty0, w as i32, ty1), true).ok()?;
    pix.clear().ok()?;
    let mut ctm = Matrix::new_translate(-x0, -y0);
    ctm.concat(Matrix::new_scale(s, s));
    {
        let dev = Device::from_pixmap(&pix).ok()?;
        page.run(&dev, &ctm).ok()?;
    }
    let (pw, ph, stride) = (pix.width() as usize, pix.height() as usize, pix.stride() as usize);
    let src = pix.samples();
    let mut data = Vec::with_capacity(pw * ph);
    for row in 0..ph {
        let line = &src[row * stride..row * stride + pw * 4];
        data.extend(line.chunks_exact(4).map(|p| rc.pixel(p[0], p[1], p[2], p[3])));
    }
    Some(Tile { w: pw as u32, h: ph as u32, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(bytes: usize) -> Tile {
        Tile { w: 1, h: 1, data: vec![0; bytes / 4] }
    }
    fn key(p: u32) -> TileKey {
        TileKey { page: p, scale: 1000, tile: 0 }
    }

    #[test]
    fn evicts_by_bytes_lru() {
        let mut c = TileCache::new(1000);
        c.insert(key(0), tile(400));
        c.insert(key(1), tile(400));
        assert!(c.get(&key(0)).is_some()); // 1 is now oldest
        c.insert(key(2), tile(400));
        assert!(!c.contains(&key(1)));
        assert!(c.contains(&key(0)) && c.contains(&key(2)));
        assert_eq!(c.bytes(), 800);
    }

    #[test]
    fn big_tile_evicts_several_and_survives() {
        let mut c = TileCache::new(1000);
        for i in 0..4 {
            c.insert(key(i), tile(200));
        }
        c.insert(key(9), tile(900));
        assert_eq!(c.len(), 1);
        assert!(c.contains(&key(9)));
        c.insert(key(10), tile(2000)); // oversized: kept alone
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 2000);
    }

    #[test]
    fn replace_adjusts_bytes() {
        let mut c = TileCache::new(1000);
        c.insert(key(0), tile(400));
        c.insert(key(0), tile(100));
        assert_eq!(c.bytes(), 100);
    }
}
