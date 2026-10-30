//! Renderer thread (owns the MuPDF document) and the byte-capped tile cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use mupdf::{Colorspace, Device, Document, IRect, Matrix, Pixmap};
use crate::links::{LinkInfo, Target};
use crate::search::{find_matches, Rect4};
use crate::ui::index::Item;
use winit::event_loop::EventLoopProxy;

use crate::layout::{page_px, tile_grid};
use crate::recolor::Recolor;

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

    pub fn peek(&self, k: &TileKey) -> Option<&Tile> {
        self.map.get(k).map(|e| &e.0)
    }

    /// Scale (other than `not`) at which `page` is cached, closest to `want`.
    pub fn nearest_scale(&self, page: u32, want: u32) -> Option<u32> {
        self.map
            .keys()
            .filter(|k| k.page == page && k.scale != want)
            .map(|k| k.scale)
            .min_by_key(|&s| (s as i64 - want as i64).abs())
    }

    /// Drop tiles rendered at other scales.
    pub fn retain_scale(&mut self, scale: u32) {
        let dead: Vec<TileKey> = self.map.keys().filter(|k| k.scale != scale).copied().collect();
        for k in dead {
            self.bytes -= self.map.remove(&k).unwrap().0.bytes();
        }
    }

    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
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

pub struct SearchReq {
    pub id: u64,
    pub query: String,
    pub forward: bool,
    pub start_page: u32,
    pub case_sensitive: bool,
}

/// A clipped render of part of a page (preview popup).
pub struct PreviewReq {
    pub id: u64,
    pub page: u32,
    /// Page points from the page origin.
    pub rect: [f32; 4],
    pub scale: f32,
    pub recolor: Recolor,
}

pub enum Cmd {
    Preview(PreviewReq),
    /// Plain text inside a rectangle (for the clipboard).
    Text { page: u32, rect: [f32; 4] },
    Tile(Request),
    Search(SearchReq),
    CancelSearch,
    Links(u32),
    Open(PathBuf),
    /// Document to use for text search (the OCR copy), or `None` for the original.
    TextSource(Option<PathBuf>),
}

pub enum Response {
    Preview { id: u64, tile: Tile },
    Text(String),
    Tile { key: TileKey, gen: u64, tile: Tile },
    /// The renderer discarded the request because it was no longer wanted.
    Skipped { key: TileKey },
    Hits { id: u64, page: u32, rects: Vec<Rect4> },
    SearchDone { id: u64 },
    Links { page: u32, links: Vec<LinkInfo> },
    Outline(Vec<Item>),
    Opened(Result<Vec<(f32, f32)>, String>),
}

pub struct RenderHandle {
    pub req_tx: Sender<Cmd>,
    pub resp_rx: Receiver<Response>,
    /// Tiles the UI currently cares about, highest priority first.
    pub wanted: Arc<Mutex<Vec<TileKey>>>,
    /// Requests with an older generation are dropped.
    pub gen: Arc<AtomicU64>,
}

/// Open the document on a new thread; returns page sizes (points) once loaded.
pub fn spawn(path: PathBuf, proxy: EventLoopProxy<()>) -> Result<(RenderHandle, Vec<(f32, f32)>), String> {
    let (req_tx, req_rx) = channel::<Cmd>();
    let (resp_tx, resp_rx) = channel::<Response>();
    let wanted = Arc::new(Mutex::new(Vec::new()));
    let gen = Arc::new(AtomicU64::new(0));
    let (w2, g2) = (wanted.clone(), gen.clone());
    thread::Builder::new()
        .name("renderer".into())
        .spawn(move || {
            let (doc, sizes) = match open(&path) {
                Ok(v) => v,
                Err(e) => {
                    let _ = resp_tx.send(Response::Opened(Err(e)));
                    return;
                }
            };
            let mut r = Renderer {
                ctx: context_ptr(),
                cs: Colorspace::device_rgb(),
                doc,
                path: path.clone(),
                text_path: path.clone(),
                loads: 0,
                sizes: sizes.clone(),
                queue: Vec::new(),
                search: None,
                page_cache: None,
                tx: resp_tx,
                proxy,
                wanted: w2,
                gen: g2,
            };
            let _ = r.tx.send(Response::Opened(Ok(sizes)));
            r.send_outline();
            r.run(req_rx);
        })
        .map_err(|e| e.to_string())?;
    match resp_rx.recv() {
        Ok(Response::Opened(Ok(sizes))) => Ok((RenderHandle { req_tx, resp_rx, wanted, gen }, sizes)),
        Ok(Response::Opened(Err(e))) => Err(e),
        _ => Err("renderer thread died".into()),
    }
}

pub fn open_doc(path: &PathBuf) -> Result<Document, String> {
    let mut doc = Document::open(path.as_path()).map_err(|e| e.to_string())?;
    if doc.needs_password().map_err(|e| e.to_string())? {
        return Err("document is password protected".into());
    }
    if doc.is_reflowable().unwrap_or(false) {
        doc.layout(600.0, 800.0, 12.0).map_err(|e| e.to_string())?;
    }
    Ok(doc)
}

fn open(path: &PathBuf) -> Result<(Document, Vec<(f32, f32)>), String> {
    let doc = open_doc(path)?;
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

/// Vertical target of a destination, in page space.
fn dest_top(k: &mupdf::DestinationKind) -> Option<f32> {
    use mupdf::DestinationKind::*;
    match *k {
        XYZ { top, .. } | FitH { top } | FitBH { top } => top,
        FitR { top, .. } => Some(top),
        _ => None,
    }
}

struct SearchState {
    req: SearchReq,
    /// Private document handle: MuPDF keeps every touched page's objects alive, so scanning the
    /// whole book would bloat the viewing document. It is reopened periodically and dropped at the end.
    doc: Option<Document>,
    since_open: usize,
    /// Pages still to scan, in order.
    next: usize,
    remaining: usize,
    hits: usize,
}

const MAX_HITS: usize = 5000;

extern "C" {
    /// Return freed heap pages to the OS (glibc).
    fn malloc_trim(pad: usize) -> i32;
}

struct Renderer {
    ctx: Option<*mut mupdf_sys::fz_context>,
    cs: Colorspace,
    doc: Document,
    path: PathBuf,
    text_path: PathBuf,
    loads: usize,
    sizes: Vec<(f32, f32)>,
    queue: Vec<Request>,
    search: Option<SearchState>,
    page_cache: Option<(u32, mupdf::Page)>,
    tx: Sender<Response>,
    proxy: EventLoopProxy<()>,
    wanted: Arc<Mutex<Vec<TileKey>>>,
    gen: Arc<AtomicU64>,
}

impl Renderer {
    fn shrink_store(&self) {
        if let Some(c) = self.ctx {
            unsafe { mupdf_sys::fz_shrink_store(c, STORE_SHRINK_PERCENT) };
        }
    }

    fn send(&self, r: Response) {
        let _ = self.tx.send(r);
        let _ = self.proxy.send_event(());
    }

    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            if self.queue.is_empty() && self.search.is_none() {
                match rx.recv() {
                    Ok(c) => self.handle(c),
                    Err(_) => return,
                }
            }
            for c in rx.try_iter().collect::<Vec<_>>() {
                self.handle(c);
            }
            if !self.queue.is_empty() {
                self.render_next();
            } else if self.search.is_some() {
                self.search_step();
            }
        }
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Tile(r) => self.queue.push(r),
            Cmd::Preview(req) => {
                let tile = self.render_preview(&req).unwrap_or(Tile { w: 0, h: 0, data: Vec::new() });
                self.shrink_store();
                self.send(Response::Preview { id: req.id, tile });
            }
            Cmd::Text { page, rect } => {
                let t = text_in_rect(&self.doc, page, rect);
                self.send(Response::Text(t));
            }
            Cmd::Search(req) => {
                let n = self.sizes.len();
                let start = (req.start_page as usize).min(n - 1);
                self.search = Some(SearchState { next: start, remaining: n, hits: 0, req, doc: None, since_open: 0 });
            }
            Cmd::CancelSearch => self.search = None,
            Cmd::TextSource(p) => {
                let usable = p
                    .and_then(|p| open_doc(&p).ok().filter(|d| d.page_count().map(|c| c as usize == self.sizes.len()).unwrap_or(false)).map(|_| p));
                self.text_path = usable.unwrap_or_else(|| self.path.clone());
            }
            Cmd::Links(p) => {
                let links = self.load_links(p);
                self.send(Response::Links { page: p, links });
            }
            Cmd::Open(path) => {
                self.search = None;
                self.queue.clear();
                self.page_cache = None;
                match open(&path) {
                    Ok((doc, sizes)) => {
                        self.doc = doc;
                        self.text_path = path.clone();
                        self.path = path;
                        self.loads = 0;
                        self.sizes = sizes.clone();
                        self.send(Response::Opened(Ok(sizes)));
                        self.send_outline();
                    }
                    Err(e) => self.send(Response::Opened(Err(e))),
                }
            }
        }
    }

    fn load_links(&mut self, p: u32) -> Vec<LinkInfo> {
        let Ok(page) = self.doc.load_page(p as i32) else { return vec![] };
        let Ok(b) = page.bounds() else { return vec![] };
        let Ok(iter) = page.links() else { return vec![] };
        iter.filter_map(|l| {
            let target = match l.dest {
                Some(d) => Target::Page(d.loc.page_number, dest_top(&d.kind)),
                None if !l.uri.is_empty() => Target::Uri(l.uri),
                None => return None,
            };
            Some(LinkInfo { rect: [l.bounds.x0 - b.x0, l.bounds.y0 - b.y0, l.bounds.x1 - b.x0, l.bounds.y1 - b.y0], target })
        })
        .collect()
    }

    fn render_preview(&mut self, req: &PreviewReq) -> Option<Tile> {
        let page = self.doc.load_page(req.page as i32).ok()?;
        let b = page.bounds().ok()?;
        let s = req.scale;
        let (w, h) = (((req.rect[2] - req.rect[0]) * s).ceil().max(1.0) as i32, ((req.rect[3] - req.rect[1]) * s).ceil().max(1.0) as i32);
        let mut pix = Pixmap::new_with_rect(&self.cs, IRect::new(0, 0, w, h), true).ok()?;
        pix.clear().ok()?;
        let mut ctm = Matrix::new_translate(-(b.x0 + req.rect[0]), -(b.y0 + req.rect[1]));
        ctm.concat(Matrix::new_scale(s, s));
        {
            let dev = Device::from_pixmap(&pix).ok()?;
            page.run(&dev, &ctm).ok()?;
        }
        let (pw, ph, stride) = (pix.width() as usize, pix.height() as usize, pix.stride() as usize);
        let src = pix.samples();
        let mut data = Vec::with_capacity(pw * ph);
        for row in 0..ph {
            data.extend(src[row * stride..row * stride + pw * 4].chunks_exact(4).map(|p| req.recolor.pixel(p[0], p[1], p[2], p[3])));
        }
        Some(Tile { w: pw as u32, h: ph as u32, data })
    }

    fn send_outline(&mut self) {
        fn walk(list: &[mupdf::Outline], depth: u32, out: &mut Vec<Item>) {
            for o in list {
                out.push(Item {
                    title: o.title.clone(),
                    depth,
                    page: o.dest.map(|d| d.loc.page_number),
                    y: o.dest.and_then(|d| dest_top(&d.kind)),
                    uri: o.uri.clone(),
                    has_children: !o.down.is_empty(),
                });
                walk(&o.down, depth + 1, out);
            }
        }
        let mut items = Vec::new();
        if let Ok(o) = self.doc.outlines() {
            walk(&o, 0, &mut items);
        }
        self.send(Response::Outline(items));
    }

    fn render_next(&mut self) {
        let order = self.wanted.lock().unwrap().clone();
        let rank = |k: &TileKey| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
        let i = (0..self.queue.len()).min_by_key(|&i| rank(&self.queue[i].key)).unwrap();
        let req = self.queue.swap_remove(i);
        if req.gen != self.gen.load(Ordering::Relaxed) {
            return; // UI already forgot about it
        }
        let k = req.key;
        if rank(&k) == usize::MAX {
            self.send(Response::Skipped { key: k });
            return;
        }
        if self.page_cache.as_ref().map(|p| p.0) != Some(k.page) {
            self.loads += 1;
            if self.loads > 150 {
                // same reason as for search: drop the object cache MuPDF accumulates per document
                self.page_cache = None;
                if let Ok(d) = open_doc(&self.path) {
                    self.doc = d;
                    unsafe { malloc_trim(0) };
                }
                self.loads = 0;
            }
            self.page_cache = self.doc.load_page(k.page as i32).ok().map(|p| (k.page, p));
        }
        // A failed render becomes an empty tile so the UI does not re-request it forever.
        let tile = self
            .page_cache
            .as_ref()
            .and_then(|(_, page)| {
                let b = page.bounds().ok()?;
                render_tile(page, b.x0, b.y0, self.sizes[k.page as usize], k, &req.recolor, &self.cs)
            })
            .unwrap_or(Tile { w: 0, h: 0, data: Vec::new() });
        self.shrink_store();
        self.send(Response::Tile { key: k, gen: req.gen, tile });
    }

    /// Scan one page. The stext page is dropped immediately; only hit rectangles are kept.
    fn search_step(&mut self) {
        let Some(st) = self.search.as_mut() else { return };
        let n = self.sizes.len();
        let page_no = st.next;
        st.remaining -= 1;
        st.next = if st.req.forward { (st.next + 1) % n } else { (st.next + n - 1) % n };
        if st.doc.is_none() || st.since_open >= 64 {
            st.doc = None; // free the old handle before opening the next
            unsafe { malloc_trim(0) };
            st.doc = open_doc(&self.text_path).ok();
            st.since_open = 0;
        }
        st.since_open += 1;
        let (id, last) = (st.req.id, st.remaining == 0);
        let rects = match &st.doc {
            Some(d) => page_hits(d, page_no, &st.req.query, st.req.case_sensitive),
            None => vec![],
        };
        st.hits += rects.len();
        let full = st.hits >= MAX_HITS;
        if last || full {
            self.search = None; // drops the private document
            unsafe { malloc_trim(0) };
        }
        self.shrink_store();
        if !rects.is_empty() {
            self.send(Response::Hits { id, page: page_no as u32, rects });
        }
        if last || full {
            self.send(Response::SearchDone { id });
        }
    }
}

/// Text of the characters whose centres lie inside `r` (page points from the page origin).
pub fn text_in_rect(doc: &Document, p: u32, r: [f32; 4]) -> String {
    let Ok(page) = doc.load_page(p as i32) else { return String::new() };
    let Ok(b) = page.bounds() else { return String::new() };
    let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) else { return String::new() };
    let mut s = String::new();
    for block in &tp.structured().blocks {
        let mupdf::TextBlockContent::Text { lines } = &block.content else { continue };
        for l in lines {
            let mut any = false;
            for c in &l.chars {
                let (cx, cy) = ((c.quad.ul.x + c.quad.lr.x) / 2.0 - b.x0, (c.quad.ul.y + c.quad.lr.y) / 2.0 - b.y0);
                if cx >= r[0] - 1.0 && cx <= r[2] + 1.0 && cy >= r[1] - 1.0 && cy <= r[3] + 1.0 {
                    s.push(c.ch);
                    any = true;
                }
            }
            if any {
                s.push('\n');
            }
        }
    }
    s
}

fn page_hits(doc: &Document, p: usize, query: &str, cs: bool) -> Vec<Rect4> {
    let Ok(page) = doc.load_page(p as i32) else { return vec![] };
    let Ok(b) = page.bounds() else { return vec![] };
    let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) else { return vec![] };
    let st = tp.structured();
    let mut out = Vec::new();
    for block in &st.blocks {
        let mupdf::TextBlockContent::Text { lines } = &block.content else { continue };
        for line in lines {
            let text: Vec<char> = line.chars.iter().map(|c| c.ch).collect();
            for (start, len) in find_matches(&text, query, cs) {
                let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                for c in &line.chars[start..start + len] {
                    for pt in [c.quad.ul, c.quad.ur, c.quad.ll, c.quad.lr] {
                        r = [r[0].min(pt.x), r[1].min(pt.y), r[2].max(pt.x), r[3].max(pt.y)];
                    }
                }
                out.push([r[0] - b.x0, r[1] - b.y0, r[2] - b.x0, r[3] - b.y0]);
            }
        }
    }
    out
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
