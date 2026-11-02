//! Persisted image classification: a stable id per image, a per-document map id -> decision
//! (`<cache dir>/images.bin`), and the rules for merging maps written by the viewer (lazily) and by
//! the analysis child (in the background).
//!
//! Identity: MuPDF does not expose a PDF object number for a decoded image, so the id is a hash of the
//! image's compressed stream (plus its pixel size). That is what the object number would give anyway:
//! a logo repeated on every slide is one stream and so one entry, and the hash also merges identical
//! copies stored under different object numbers. Images without a compressed stream (rare: inline
//! pixmaps) fall back to page + placement rectangle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mupdf::{Colorspace, ColorParams, Device, IRect, Image, Matrix, Pixmap};

/// Bump to reclassify every non-overridden image once.
pub const CLASSIFIER_VERSION: u8 = 6;

const MAGIC: &[u8; 4] = b"SIMG";
/// Format 2 entries are 10 bytes (id, flags, version) plus 3 when the flags say the placement has a cell grid;
/// format 1 (fixed 10 bytes, ids of whole streams) is still read, for its overrides.
const FORMAT: u8 = 2;
const HEADER: usize = 10;
const HAS_GRID: u8 = 16;
/// Per page, as the renderer's own cap.
pub const MAX_PER_PAGE: usize = 64;

/// Times an image was actually classified in this process (a debugging and test aid).
pub static CLASSIFY_CALLS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Unknown = 0,
    /// Diagram / logo on a white background: recolored with its hues kept.
    Line = 1,
    /// Keeps its colours.
    Photo = 2,
    /// Page-sized image: always recolored (never decoded to decide).
    Scan = 3,
}

impl Class {
    fn from_bits(b: u8) -> Class {
        match b & 3 {
            1 => Class::Line,
            2 => Class::Photo,
            3 => Class::Scan,
            _ => Class::Unknown,
        }
    }
}

/// User decision for one image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Ovr {
    #[default]
    Auto = 0,
    Recolor = 1,
    Keep = 2,
}

impl Ovr {
    fn from_bits(b: u8) -> Ovr {
        match b & 3 {
            1 => Ovr::Recolor,
            2 => Ovr::Keep,
            _ => Ovr::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Ovr::Auto => "auto",
            Ovr::Recolor => "recolor",
            Ovr::Keep => "keep",
        }
    }
}

/// Per-cell classes of a mixed crop: `nx` x `ny` cells (each 1..=4; `nx == 0`: no grid), row-major,
/// bit set = line art. Three bytes in memory and on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Grid {
    pub nx: u8,
    pub ny: u8,
    pub bits: u16,
}

impl Grid {
    pub const NONE: Grid = Grid { nx: 0, ny: 0, bits: 0 };

    pub fn is_some(self) -> bool {
        self.nx > 0 && self.ny > 0
    }

    pub fn line(self, cx: u8, cy: u8) -> bool {
        self.bits >> (cy * self.nx + cx) & 1 != 0
    }

    /// `LLP/PPL`: rows top to bottom, `L` line art, `P` keeps its colours.
    pub fn show(self) -> String {
        (0..self.ny).map(|y| (0..self.nx).map(|x| if self.line(x, y) { 'L' } else { 'P' }).collect::<String>()).collect::<Vec<_>>().join("/")
    }
}

/// What the classifier decided for a placement: a class, plus per-cell classes when the crop is mixed
/// (then `class` is `Photo`, the conservative whole-placement answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub class: Class,
    pub grid: Grid,
}

impl Verdict {
    pub fn plain(class: Class) -> Verdict {
        Verdict { class, grid: Grid::NONE }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub class: Class,
    pub grid: Grid,
    /// Classifier version that produced `class`.
    pub ver: u8,
    pub ovr: Ovr,
}

const CLASS_DIRTY: u8 = 1;
const OVR_DIRTY: u8 = 2;

/// The decisions of one document. Entries touched since the last write are tracked so that a write
/// merges only those into whatever is on disk (the other process may have added entries meanwhile).
#[derive(Default)]
pub struct ImageMap {
    map: HashMap<u64, Entry>,
    pending: HashMap<u64, u8>,
    /// Classifier version for which a whole-document pass has finished (0: none).
    complete: u8,
    complete_dirty: bool,
    path: Option<PathBuf>,
}

pub fn map_path(dir: &Path) -> PathBuf {
    dir.join("images.bin")
}

/// Short exclusive lock around a read-merge-write of the map; released when the file is dropped.
fn brief_lock(dir: &Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let f = std::fs::File::options().create(true).append(true).open(dir.join("images.lock"))?;
    while unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(f)
}

fn encode(complete: u8, map: &HashMap<u64, Entry>) -> Vec<u8> {
    let mut ids: Vec<&u64> = map.keys().collect();
    ids.sort_unstable();
    let mut out = Vec::with_capacity(HEADER + ids.len() * 10);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.push(complete);
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in ids {
        let e = map[id];
        out.extend_from_slice(&id.to_le_bytes());
        out.push(e.class as u8 | (e.ovr as u8) << 2 | if e.grid.is_some() { HAS_GRID } else { 0 });
        out.push(e.ver);
        if e.grid.is_some() {
            out.push(e.grid.nx << 4 | e.grid.ny);
            out.extend_from_slice(&e.grid.bits.to_le_bytes());
        }
    }
    out
}

fn decode(b: &[u8]) -> Option<(u8, HashMap<u64, Entry>)> {
    if b.len() < HEADER || &b[..4] != MAGIC || !(1..=FORMAT).contains(&b[4]) {
        return None;
    }
    let v1 = b[4] == 1;
    let n = u32::from_le_bytes(b[6..10].try_into().ok()?) as usize;
    let mut map = HashMap::with_capacity(n.min(1 << 20));
    let mut at = HEADER;
    for _ in 0..n {
        let c = b.get(at..at + 10)?;
        at += 10;
        let id = u64::from_le_bytes(c[..8].try_into().unwrap());
        let mut e = Entry { class: Class::from_bits(c[8]), grid: Grid::NONE, ovr: Ovr::from_bits(c[8] >> 2), ver: c[9] };
        if !v1 && c[8] & HAS_GRID != 0 {
            let g = b.get(at..at + 3)?;
            at += 3;
            e.grid = Grid { nx: g[0] >> 4, ny: g[0] & 15, bits: u16::from_le_bytes([g[1], g[2]]) };
            if !e.grid.is_some() || e.grid.nx > 4 || e.grid.ny > 4 {
                return None;
            }
        }
        if v1 {
            // whole-stream ids of the old scheme no longer match placements; only overrides are worth keeping
            if e.ovr == Ovr::Auto {
                continue;
            }
            e.class = Class::Unknown;
            e.ver = 0;
        }
        map.insert(id, e);
    }
    (at == b.len()).then_some((b[5], map))
}

/// Merge the entries `mine` marked in `pending` into `base`.
///  - an override always wins over what is on disk, and is never touched by a classification;
///  - a classification replaces a missing, older-version or `Scan` placeholder entry, but never an entry
///    of the current version (the first writer wins, so a lazy result is not clobbered by the child);
///  - entries without information (unknown class, no override) are dropped.
fn merge(base: &mut HashMap<u64, Entry>, mine: &HashMap<u64, Entry>, pending: &HashMap<u64, u8>) {
    for (id, &flags) in pending {
        let Some(m) = mine.get(id) else { continue };
        let mut b = base.get(id).copied().unwrap_or(Entry { class: Class::Unknown, grid: Grid::NONE, ver: 0, ovr: Ovr::Auto });
        if flags & CLASS_DIRTY != 0 && m.class != Class::Unknown {
            let replace = b.class == Class::Unknown || b.ver < m.ver || (b.ver == m.ver && b.class == Class::Scan && m.class != Class::Scan);
            if replace {
                b.class = m.class;
                b.grid = m.grid;
                b.ver = m.ver;
            }
        }
        if flags & OVR_DIRTY != 0 {
            b.ovr = m.ovr;
        }
        if b.class == Class::Unknown && b.ovr == Ovr::Auto {
            base.remove(id);
        } else {
            base.insert(*id, b);
        }
    }
}

impl ImageMap {
    /// An in-memory map that is never written.
    pub fn detached() -> Self {
        ImageMap::default()
    }

    /// Load `<dir>/images.bin` (missing or corrupt: empty).
    pub fn load(dir: &Path) -> Self {
        let (complete, map) = std::fs::read(map_path(dir)).ok().and_then(|b| decode(&b)).unwrap_or_default();
        ImageMap { map, pending: HashMap::new(), complete, complete_dirty: false, path: Some(dir.to_path_buf()) }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The decision for `id` if it is classified by the current classifier.
    pub fn class(&self, id: u64) -> Option<Class> {
        self.map.get(&id).filter(|e| e.ver == CLASSIFIER_VERSION && e.class != Class::Unknown).map(|e| e.class)
    }

    /// The cell grid of a mixed crop classified by the current classifier.
    pub fn grid(&self, id: u64) -> Grid {
        self.map.get(&id).filter(|e| e.ver == CLASSIFIER_VERSION).map_or(Grid::NONE, |e| e.grid)
    }

    pub fn ovr(&self, id: u64) -> Ovr {
        self.map.get(&id).map_or(Ovr::Auto, |e| e.ovr)
    }

    #[cfg(test)]
    pub fn set_class(&mut self, id: u64, class: Class) {
        self.set_verdict(id, Verdict::plain(class));
    }

    pub fn set_verdict(&mut self, id: u64, v: Verdict) {
        let e = self.map.entry(id).or_insert(Entry { class: v.class, grid: v.grid, ver: CLASSIFIER_VERSION, ovr: Ovr::Auto });
        e.class = v.class;
        e.grid = v.grid;
        e.ver = CLASSIFIER_VERSION;
        *self.pending.entry(id).or_default() |= CLASS_DIRTY;
    }

    pub fn set_ovr(&mut self, id: u64, ovr: Ovr) {
        let e = self.map.entry(id).or_insert(Entry { class: Class::Unknown, grid: Grid::NONE, ver: 0, ovr });
        e.ovr = ovr;
        *self.pending.entry(id).or_default() |= OVR_DIRTY;
    }

    /// Whether every image of the document was classified by a finished background pass.
    pub fn is_complete(&self) -> bool {
        self.complete == CLASSIFIER_VERSION
    }

    pub fn mark_complete(&mut self) {
        self.complete = CLASSIFIER_VERSION;
        self.complete_dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty() || self.complete_dirty
    }

    /// Merge with the file on disk and (when this process has news) write it back atomically.
    /// A short separate lock serializes the viewer and the analysis child; the long per-document
    /// analysis lock is not needed (and the viewer must never wait for it).
    pub fn sync(&mut self) -> std::io::Result<()> {
        let Some(dir) = self.path.clone() else { return Ok(()) };
        let write = self.is_dirty();
        if !write && !dir.exists() {
            return Ok(());
        }
        let _guard = if write {
            std::fs::create_dir_all(&dir)?;
            Some(brief_lock(&dir)?)
        } else {
            None
        };
        let (dcomplete, mut base) = std::fs::read(map_path(&dir)).ok().and_then(|b| decode(&b)).unwrap_or_default();
        merge(&mut base, &self.map, &self.pending);
        let complete = if self.complete_dirty { self.complete.max(dcomplete) } else { dcomplete };
        if write {
            let tmp = dir.join("images.bin.tmp");
            std::fs::write(&tmp, encode(complete, &base))?;
            std::fs::rename(&tmp, map_path(&dir))?;
        }
        self.map = base;
        self.complete = complete.max(self.complete);
        self.pending.clear();
        self.complete_dirty = false;
        Ok(())
    }
}

// ---- identity --------------------------------------------------------------------------------

const K: u64 = 0x9E37_79B9_7F4A_7C15;

/// Fast 64-bit hash over bytes, eight at a time.
pub fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed ^ (bytes.len() as u64).wrapping_mul(K);
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(K);
        h ^= h >> 29;
    }
    for &b in chunks.remainder() {
        h = (h ^ b as u64).wrapping_mul(K);
    }
    h ^ (h >> 32)
}

/// Id of an image with a compressed stream (top bit clear).
pub fn stream_id(w: u32, h: u32, stream: &[u8]) -> u64 {
    hash_bytes(((w as u64) << 32) | h as u64, stream) & !(1 << 63)
}

/// Id of an image without a stream: page and placement (top bit set).
pub fn placement_id(page: u32, rect: [f32; 4], w: u32, h: u32) -> u64 {
    let mut b = Vec::with_capacity(32);
    b.extend_from_slice(&page.to_le_bytes());
    for v in rect {
        b.extend_from_slice(&(v.round() as i32).to_le_bytes());
    }
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&h.to_le_bytes());
    hash_bytes(K, &b) | (1 << 63)
}

thread_local! {
    static CTX: std::cell::Cell<*mut mupdf_sys::fz_context> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The calling thread's MuPDF context (recovered from the crate's Debug output, as the renderer does).
fn ctx() -> *mut mupdf_sys::fz_context {
    CTX.with(|c| {
        if c.get().is_null() {
            let s = format!("{:?}", mupdf::Context::get());
            let p = s.split("0x").nth(1).map(|h| h.trim_end_matches(|c: char| !c.is_ascii_hexdigit())).and_then(|h| usize::from_str_radix(h, 16).ok());
            c.set(p.unwrap_or(0) as *mut _);
        }
        c.get()
    })
}

/// Drop MuPDF's cache of decoded images and glyphs (the child calls this after each page).
pub fn empty_store() {
    let c = ctx();
    if !c.is_null() {
        unsafe { mupdf_sys::fz_empty_store(c) };
    }
}

/// The raw `fz_image` behind the wrapper (whose pointer field is crate-private).
fn raw(img: &Image) -> *mut mupdf_sys::fz_image {
    const _: () = assert!(std::mem::size_of::<Image>() == std::mem::size_of::<usize>());
    unsafe { std::mem::transmute_copy::<Image, *mut mupdf_sys::fz_image>(img) }
}

/// Id of `img` drawn at `full`, of which `vis` is visible. A placement shown whole keeps the plain stream id
/// (shared by every use of the image); a crop mixes in the visible rectangle, so two crops of one image
/// get separate classes and overrides.
pub fn image_id(img: &Image, page: u32, vis: [f32; 4], full: [f32; 4]) -> u64 {
    let c = ctx();
    if !c.is_null() {
        unsafe {
            let cb = mupdf_sys::fz_compressed_image_buffer(c, raw(img));
            if !cb.is_null() && !(*cb).buffer.is_null() {
                let mut data: *mut u8 = std::ptr::null_mut();
                let n = mupdf_sys::fz_buffer_storage(c, (*cb).buffer, &mut data);
                if !data.is_null() && n > 0 {
                    let id = stream_id(img.width(), img.height(), std::slice::from_raw_parts(data, n));
                    return if vis.iter().zip(full).all(|(a, b)| (a - b).abs() < 2.0) { id } else { crop_id(id, vis) };
                }
            }
        }
    }
    placement_id(page, vis, img.width(), img.height())
}

/// Id of the crop `vis` (points, page origin) of the image with stream id `id`.
pub fn crop_id(id: u64, vis: [f32; 4]) -> u64 {
    let mut b = [0u8; 16];
    for (i, v) in vis.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&(v.round() as i32).to_le_bytes());
    }
    hash_bytes(id, &b) & !(1 << 63)
}

// ---- placements and classification -------------------------------------------------------------

/// An image placement on a page (points from the page origin).
pub struct Placement {
    /// The visible part: image rect, clipped by the clip in effect and the page.
    pub rect: [f32; 4],
    /// The image's whole rectangle (may extend past the page).
    pub full: [f32; 4],
    /// Image space (unit square) to page points.
    pub ctm: [f32; 6],
    pub id: u64,
    pub image: Image,
}

impl Placement {
    /// Visible area.
    pub fn area(&self) -> f32 {
        (self.rect[2] - self.rect[0]) * (self.rect[3] - self.rect[1])
    }
}

type Rect = [f32; 4];

fn isect(a: Rect, b: Rect) -> Rect {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn from_fz(r: mupdf_sys::fz_rect) -> Rect {
    [r.x0, r.y0, r.x1, r.y1]
}

/// Bounding box of the unit square under `m`.
fn unit_bbox(m: &mupdf_sys::fz_matrix) -> Rect {
    let xs = [m.e, m.a + m.e, m.c + m.e, m.a + m.c + m.e];
    let ys = [m.f, m.b + m.f, m.d + m.f, m.b + m.d + m.f];
    [xs.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max), ys.iter().cloned().fold(f32::MIN, f32::max)]
}

/// A MuPDF device that does nothing but track the clip stack and note each drawn image with the clip in
/// effect. Pages are run through it without decoding anything.
#[repr(C)]
struct Rec {
    dev: mupdf_sys::fz_device,
    stack: Vec<Rect>,
    mask_area: Rect,
    found: Vec<(*mut mupdf_sys::fz_image, [f32; 6], Rect)>,
}

type Ctx = *mut mupdf_sys::fz_context;
type Dev = *mut mupdf_sys::fz_device;
type Fz = mupdf_sys::fz_matrix;

unsafe fn rec<'a>(d: Dev) -> &'a mut Rec {
    unsafe { &mut *(d as *mut Rec) }
}

fn top(r: &Rec) -> Rect {
    r.stack.last().copied().unwrap_or([f32::MIN, f32::MIN, f32::MAX, f32::MAX])
}

fn push(r: &mut Rec, b: Rect) {
    let t = isect(top(r), b);
    r.stack.push(t);
}

unsafe extern "C" fn cb_drop(c: Ctx, d: Dev) {
    unsafe {
        let r = rec(d);
        for &(img, _, _) in &r.found {
            mupdf_sys::fz_drop_image(c, img);
        }
        std::ptr::drop_in_place(&mut r.found);
        std::ptr::drop_in_place(&mut r.stack);
    }
}

unsafe extern "C" fn cb_fill_image(c: Ctx, d: Dev, img: *mut mupdf_sys::fz_image, m: Fz, _a: f32, _p: mupdf_sys::fz_color_params) {
    unsafe {
        let r = rec(d);
        if r.found.len() >= MAX_PER_PAGE * 4 {
            return;
        }
        let vis = isect(top(r), unit_bbox(&m));
        if vis[2] - vis[0] < 4.0 || vis[3] - vis[1] < 4.0 {
            return; // rules and bullets drawn as images, or nothing visible
        }
        r.found.push((mupdf_sys::fz_keep_image(c, img), [m.a, m.b, m.c, m.d, m.e, m.f], top(r)));
    }
}

unsafe extern "C" fn cb_clip_path(c: Ctx, d: Dev, p: *const mupdf_sys::fz_path, _eo: i32, m: Fz, _s: mupdf_sys::fz_rect) {
    unsafe { push(rec(d), from_fz(mupdf_sys::fz_bound_path(c, p, std::ptr::null(), m))) }
}

unsafe extern "C" fn cb_clip_stroke_path(c: Ctx, d: Dev, p: *const mupdf_sys::fz_path, st: *const mupdf_sys::fz_stroke_state, m: Fz, _s: mupdf_sys::fz_rect) {
    unsafe { push(rec(d), from_fz(mupdf_sys::fz_bound_path(c, p, st, m))) }
}

unsafe extern "C" fn cb_clip_text(c: Ctx, d: Dev, t: *const mupdf_sys::fz_text, m: Fz, _s: mupdf_sys::fz_rect) {
    unsafe { push(rec(d), from_fz(mupdf_sys::fz_bound_text(c, t, std::ptr::null(), m))) }
}

unsafe extern "C" fn cb_clip_stroke_text(c: Ctx, d: Dev, t: *const mupdf_sys::fz_text, st: *const mupdf_sys::fz_stroke_state, m: Fz, _s: mupdf_sys::fz_rect) {
    unsafe { push(rec(d), from_fz(mupdf_sys::fz_bound_text(c, t, st, m))) }
}

unsafe extern "C" fn cb_clip_image_mask(_c: Ctx, d: Dev, _i: *mut mupdf_sys::fz_image, m: Fz, _s: mupdf_sys::fz_rect) {
    unsafe { push(rec(d), unit_bbox(&m)) }
}

unsafe extern "C" fn cb_pop_clip(_c: Ctx, d: Dev) {
    unsafe {
        rec(d).stack.pop();
    }
}

unsafe extern "C" fn cb_begin_mask(_c: Ctx, d: Dev, area: mupdf_sys::fz_rect, _l: i32, _cs: *mut mupdf_sys::fz_colorspace, _bc: *const f32, _p: mupdf_sys::fz_color_params) {
    unsafe { rec(d).mask_area = from_fz(area) }
}

unsafe extern "C" fn cb_end_mask(_c: Ctx, d: Dev, _f: *mut mupdf_sys::fz_function) {
    unsafe {
        let r = rec(d);
        let a = r.mask_area;
        push(r, a); // the mask is a clip until the matching pop_clip
    }
}

/// Image draws of a page with the clip in effect: (image, matrix, clip), in page points from the origin.
fn run_recorder(page: &mupdf::Page, b: &mupdf::Rect) -> Vec<(Image, [f32; 6], Rect)> {
    const _: () = assert!(std::mem::size_of::<mupdf::Page>() == std::mem::size_of::<usize>());
    let c = ctx();
    if c.is_null() {
        return Vec::new();
    }
    let page_rect = [0.0, 0.0, b.x1 - b.x0, b.y1 - b.y0];
    unsafe {
        let d = mupdf_sys::fz_new_device_of_size(c, std::mem::size_of::<Rec>() as i32);
        if d.is_null() {
            return Vec::new();
        }
        let r = rec(d);
        std::ptr::write(&mut r.stack, vec![page_rect]);
        std::ptr::write(&mut r.mask_area, page_rect);
        std::ptr::write(&mut r.found, Vec::new());
        (*d).drop_device = Some(cb_drop);
        (*d).fill_image = Some(cb_fill_image);
        (*d).clip_path = Some(cb_clip_path);
        (*d).clip_stroke_path = Some(cb_clip_stroke_path);
        (*d).clip_text = Some(cb_clip_text);
        (*d).clip_stroke_text = Some(cb_clip_stroke_text);
        (*d).clip_image_mask = Some(cb_clip_image_mask);
        (*d).pop_clip = Some(cb_pop_clip);
        (*d).begin_mask = Some(cb_begin_mask);
        (*d).end_mask = Some(cb_end_mask);
        let pg = std::mem::transmute_copy::<mupdf::Page, *mut mupdf_sys::fz_page>(page);
        let m = Fz { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: -b.x0, f: -b.y0 };
        let mut err: *mut mupdf_sys::mupdf_error_t = std::ptr::null_mut();
        mupdf_sys::mupdf_run_page(c, pg, d, m, std::ptr::null_mut(), &mut err);
        if !err.is_null() {
            mupdf_sys::mupdf_drop_error(err);
        }
        mupdf_sys::fz_close_device(c, d);
        let found = std::mem::take(&mut rec(d).found); // ownership of the image references moves to the Images
        mupdf_sys::fz_drop_device(c, d);
        found
            .into_iter()
            .map(|(p, m, clip)| (std::mem::transmute_copy::<*mut mupdf_sys::fz_image, Image>(&p), m, clip))
            .collect()
    }
}

/// Image placements of a page, each with its visible rect (image rect, clip in effect and page bounds
/// intersected). Images are referenced, not decoded.
pub fn placements(page: &mupdf::Page, page_no: u32) -> Vec<Placement> {
    let Ok(b) = page.bounds() else { return Vec::new() };
    let page_rect = [0.0, 0.0, b.x1 - b.x0, b.y1 - b.y0];
    let mut out = Vec::new();
    for (image, m, clip) in run_recorder(page, &b) {
        let full = unit_bbox(&Fz { a: m[0], b: m[1], c: m[2], d: m[3], e: m[4], f: m[5] });
        let rect = isect(isect(full, clip), page_rect);
        if rect[2] - rect[0] < 4.0 || rect[3] - rect[1] < 4.0 {
            continue;
        }
        out.push(Placement { rect, full, ctm: m, id: image_id(&image, page_no, rect, full), image });
        if out.len() >= MAX_PER_PAGE {
            break;
        }
    }
    out
}

/// Whether the placements make this a scanned page: one covers most of the page.
pub fn is_scan_page(ps: &[Placement], page_area: f32) -> bool {
    ps.iter().any(|p| p.area() > crate::recolor::SCAN_SHARE * page_area)
}

/// Longest side of the classification thumbnail, pixels.
const THUMB: f32 = 192.0;
/// A grid cell is at least this many points on a side, and a grid has at most `MAX_CELLS` per side.
pub const MIN_CELL: f32 = 48.0;
const MAX_CELLS: usize = 4;

/// Grid size for a visible rect of `w` x `h` points.
pub fn grid_dims(w: f32, h: f32) -> (usize, usize) {
    let n = |v: f32| ((v / MIN_CELL) as usize).clamp(1, MAX_CELLS);
    (n(w), n(h))
}

/// Combine per-cell line flags (row-major, `nx` x `ny`): uniform cells give a plain class, a mix a grid.
pub fn verdict_from_cells(cells: &[bool], nx: usize, ny: usize) -> Verdict {
    // a clear majority decides the whole crop; only a genuine split keeps a grid
    let lines = cells.iter().filter(|&&c| c).count();
    if lines * 4 >= cells.len() * 3 {
        return Verdict::plain(Class::Line);
    }
    if (cells.len() - lines) * 4 >= cells.len() * 3 {
        return Verdict::plain(Class::Photo);
    }
    let bits = cells.iter().enumerate().fold(0u16, |a, (i, &c)| a | (c as u16) << i);
    Verdict { class: Class::Photo, grid: Grid { nx: nx as u8, ny: ny as u8, bits } }
}

/// Whether a split into line cells (`line`) and the rest is real: the other cells together must be photographic
/// (many colours or dark), and both kinds must each form one 4-connected block.
pub fn genuine_split(line: &[bool], rest_photographic: bool, nx: usize, ny: usize) -> bool {
    if !rest_photographic {
        return false;
    }
    let blocks = |want: bool| {
        let Some(start) = line.iter().position(|&l| l == want) else { return true };
        let mut seen = vec![false; line.len()];
        let mut todo = vec![start];
        seen[start] = true;
        while let Some(i) = todo.pop() {
            let (x, y) = (i % nx, i / nx);
            let nb = [(x > 0).then(|| i - 1), (x + 1 < nx).then(|| i + 1), (y > 0).then(|| i - nx), (y + 1 < ny).then(|| i + nx)];
            for j in nb.into_iter().flatten() {
                if line[j] == want && !seen[j] {
                    seen[j] = true;
                    todo.push(j);
                }
            }
        }
        line.iter().zip(&seen).all(|(&l, &s)| l != want || s)
    };
    blocks(true) && blocks(false)
}

/// Classify the visible crop of a placement (not the whole stream, and not per tile, so the result does not
/// depend on zoom): the image drawn as placed onto a white thumbnail of the visible rect, which makes MuPDF
/// decode it subsampled. A crop of mixed content is judged per cell of a coarse grid.
pub fn classify(p: &Placement, cs: &Colorspace) -> Verdict {
    CLASSIFY_CALLS.fetch_add(1, Ordering::Relaxed);
    let img = &p.image;
    if std::env::var_os("STRATA_DEBUG").is_some() {
        eprintln!("imagemap: classifying {}x{} image crop {:?}", img.width(), img.height(), p.rect);
    }
    if img.color_space().is_none() {
        return Verdict::plain(Class::Line); // a 1-bit stencil: ink by definition
    }
    let photo = Verdict::plain(Class::Photo);
    let v = p.rect;
    let (w, h) = (v[2] - v[0], v[3] - v[1]);
    let s = (THUMB / w.max(h)).min(2.0);
    let (pw, ph) = ((w * s).ceil().max(1.0) as i32, (h * s).ceil().max(1.0) as i32);
    let Ok(mut pix) = Pixmap::new_with_rect(cs, IRect::new(0, 0, pw, ph), false) else { return photo };
    if pix.clear_with(255).is_err() {
        return photo;
    }
    {
        let Ok(dev) = Device::from_pixmap(&pix) else { return photo };
        let c = p.ctm;
        let m = Matrix::new(s * c[0], s * c[1], s * c[2], s * c[3], s * (c[4] - v[0]), s * (c[5] - v[1]));
        // a soft mask (logos with a transparent surround) must apply, or the hidden pixels behind it are judged
        let mask = img.mask();
        if mask.as_ref().is_some_and(|k| dev.clip_image_mask(k, &m).is_err()) {
            return photo;
        }
        let ok = dev.fill_image(img, &m, 1.0, ColorParams::default()).is_ok();
        if mask.is_some() {
            let _ = dev.pop_clip();
        }
        if !ok {
            return photo;
        }
    }
    let n = pix.n() as usize;
    let (stride, sm) = (pix.stride() as usize, pix.samples());
    let px = |x: usize, y: usize| {
        let q = &sm[y * stride + x * n..];
        [q[0], q[1], q[2]]
    };
    let whole = crate::recolor::line_stats((0..ph as usize).flat_map(|y| (0..pw as usize).map(move |x| (x, y))).map(|(x, y)| px(x, y)));
    if whole.is_some_and(|s| s.is_flat() && s.mean >= crate::recolor::DARK_MEAN) {
        return Verdict::plain(Class::Line); // a palette-limited graphic, however much white it has
    }
    if std::env::var_os("STRATA_DEBUG").is_some() {
        eprintln!("imagemap: whole {whole:?}");
    }
    let whole_line = whole.is_some_and(|s| s.is_line());
    let (nx, ny) = grid_dims(w, h);
    if nx * ny == 1 {
        return Verdict::plain(if whole_line { Class::Line } else { Class::Photo });
    }
    let mut cells = Vec::with_capacity(nx * ny);
    for cy in 0..ny {
        for cx in 0..nx {
            let (x0, x1) = (cx * pw as usize / nx, (cx + 1) * pw as usize / nx);
            let (y0, y1) = (cy * ph as usize / ny, (cy + 1) * ph as usize / ny);
            let st = crate::recolor::line_stats((y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| px(x, y)));
            if std::env::var_os("STRATA_DEBUG").is_some() {
                eprintln!("imagemap: cell {cx},{cy} stats {st:?}");
            }
            cells.push(st.is_some_and(|s| s.is_line()));
        }
    }
    let v = verdict_from_cells(&cells, nx, ny);
    if v.grid.is_some() {
        let rest = crate::recolor::line_stats((0..ph as usize).flat_map(|y| (0..pw as usize).map(move |x| (x, y))).filter(|&(x, y)| !cells[(y * ny / ph as usize) * nx + x * nx / pw as usize]).map(|(x, y)| px(x, y)));
        if rest.is_some_and(|r| r.is_photographic()) && genuine_split(&cells, true, nx, ny) {
            return v;
        }
        return Verdict::plain(if whole_line { Class::Line } else { Class::Photo });
    }
    v
}

/// Verdict for a placement: from the map if known, else classified (and recorded). `Scan` is by geometry.
pub fn verdict_of(map: &std::sync::Mutex<ImageMap>, p: &Placement, cs: &Colorspace) -> Verdict {
    if let Some(v) = map.lock().ok().and_then(|m| m.class(p.id).map(|class| Verdict { class, grid: m.grid(p.id) })) {
        if v.class != Class::Scan {
            return v;
        }
    }
    let v = classify(p, cs);
    if let Ok(mut m) = map.lock() {
        m.set_verdict(p.id, v);
    }
    v
}

/// Next override in the cycle for an image whose automatic look is `auto_recolored`:
/// auto -> the opposite of that look -> the same look forced -> auto.
pub fn next_override(cur: Ovr, auto_recolored: bool) -> Ovr {
    let first = if auto_recolored { Ovr::Keep } else { Ovr::Recolor };
    let second = if auto_recolored { Ovr::Recolor } else { Ovr::Keep };
    match cur {
        Ovr::Auto => first,
        o if o == first => second,
        _ => Ovr::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-imap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ids_are_stable_and_distinguish_streams() {
        let a = stream_id(100, 50, b"some jpeg bytes here, long enough to chunk");
        assert_eq!(a, stream_id(100, 50, b"some jpeg bytes here, long enough to chunk"));
        assert_ne!(a, stream_id(100, 50, b"some jpeg bytes here, long enough to chunkX"));
        assert_ne!(a, stream_id(101, 50, b"some jpeg bytes here, long enough to chunk"));
        assert_eq!(a >> 63, 0);
        // one bit flipped in a late byte
        let mut v = vec![7u8; 1000];
        let x = stream_id(1, 1, &v);
        v[993] ^= 1;
        assert_ne!(x, stream_id(1, 1, &v));
        // placement ids: same placement same id, tagged, differ by page and place
        let p = placement_id(3, [10.0, 20.0, 110.0, 80.0], 64, 32);
        assert_eq!(p, placement_id(3, [10.2, 19.9, 110.1, 80.0], 64, 32));
        assert_ne!(p, placement_id(4, [10.0, 20.0, 110.0, 80.0], 64, 32));
        assert_eq!(p >> 63, 1);
    }

    #[test]
    fn round_trip_through_the_file() {
        let d = tmpdir("rt");
        let mut m = ImageMap::load(&d);
        assert_eq!(m.len(), 0);
        m.set_class(1, Class::Line);
        m.set_class(2, Class::Photo);
        m.set_class(3, Class::Scan);
        m.set_ovr(2, Ovr::Recolor);
        m.set_ovr(9, Ovr::Keep); // never classified
        m.mark_complete();
        m.sync().unwrap();
        assert!(!m.is_dirty());
        assert_eq!(std::fs::metadata(map_path(&d)).unwrap().len() as usize, HEADER + 4 * 10);
        let l = ImageMap::load(&d);
        assert_eq!(l.class(1), Some(Class::Line));
        assert_eq!(l.class(2), Some(Class::Photo));
        assert_eq!(l.class(3), Some(Class::Scan));
        assert_eq!(l.ovr(2), Ovr::Recolor);
        assert_eq!(l.class(9), None);
        assert_eq!(l.ovr(9), Ovr::Keep);
        assert_eq!(l.class(77), None);
        assert!(l.is_complete());
        assert!(decode(b"junk").is_none());
        assert!(decode(&encode(1, &l.map)[..HEADER + 3]).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn override_wins_and_survives_reclassification() {
        let d = tmpdir("ovr");
        let mut viewer = ImageMap::load(&d);
        viewer.set_class(1, Class::Line);
        viewer.set_ovr(1, Ovr::Keep);
        viewer.sync().unwrap();
        // the child classified the same image differently and has not seen the override
        let mut child = ImageMap::detached();
        child.path = Some(d.clone());
        child.set_class(1, Class::Photo);
        child.set_class(2, Class::Photo);
        child.sync().unwrap();
        let l = ImageMap::load(&d);
        assert_eq!(l.ovr(1), Ovr::Keep);
        assert_eq!(l.class(1), Some(Class::Line), "current-version lazy entry is not clobbered");
        assert_eq!(l.class(2), Some(Class::Photo));
        // clearing the override goes through too
        let mut v2 = ImageMap::load(&d);
        v2.set_ovr(1, Ovr::Auto);
        v2.sync().unwrap();
        assert_eq!(ImageMap::load(&d).ovr(1), Ovr::Auto);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn version_bump_reclassifies_but_keeps_overrides() {
        let mut base: HashMap<u64, Entry> = HashMap::new();
        base.insert(1, Entry { class: Class::Line, grid: Grid::NONE, ver: CLASSIFIER_VERSION - 1, ovr: Ovr::Auto });
        base.insert(2, Entry { class: Class::Line, grid: Grid::NONE, ver: CLASSIFIER_VERSION - 1, ovr: Ovr::Keep });
        // loaded from an older version: not a valid classification any more
        let mut m = ImageMap { map: base.clone(), ..Default::default() };
        assert_eq!(m.class(1), None);
        assert_eq!(m.class(2), None);
        assert_eq!(m.ovr(2), Ovr::Keep);
        m.set_class(1, Class::Photo);
        m.set_class(2, Class::Photo);
        merge(&mut base, &m.map, &m.pending);
        assert_eq!(base[&1].class, Class::Photo);
        assert_eq!(base[&1].ver, CLASSIFIER_VERSION);
        assert_eq!(base[&2].class, Class::Photo);
        assert_eq!(base[&2].ovr, Ovr::Keep, "override untouched by the reclassification");
        // a Scan placeholder is replaced by a real class, a real class is not replaced by Scan
        let mut b: HashMap<u64, Entry> = HashMap::new();
        let mut c = ImageMap::default();
        c.set_class(5, Class::Scan);
        merge(&mut b, &c.map, &c.pending);
        let mut c2 = ImageMap::default();
        c2.set_class(5, Class::Line);
        merge(&mut b, &c2.map, &c2.pending);
        assert_eq!(b[&5].class, Class::Line);
        merge(&mut b, &c.map, &c.pending);
        assert_eq!(b[&5].class, Class::Line);
        // an incomplete pass from an older classifier is not complete
        let m = ImageMap { complete: CLASSIFIER_VERSION - 1, ..Default::default() };
        assert!(!m.is_complete());
    }

    #[test]
    fn complete_map_means_no_classifier_calls() {
        let d = tmpdir("zero");
        let mut m = ImageMap::load(&d);
        for id in 0..50u64 {
            m.set_class(id, if id % 2 == 0 { Class::Line } else { Class::Photo });
        }
        m.mark_complete();
        m.sync().unwrap();
        // second open: every id is known, so the lookup path never needs the classifier
        let l = ImageMap::load(&d);
        assert!(l.is_complete());
        let before = CLASSIFY_CALLS.load(Ordering::Relaxed);
        let unknown = (0..50u64).filter(|&id| l.class(id).is_none()).count();
        assert_eq!(unknown, 0);
        assert_eq!(CLASSIFY_CALLS.load(Ordering::Relaxed), before);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn override_cycle_starts_by_changing_the_look() {
        // line art (recolored): keep, then forced recolor, then auto
        assert_eq!(next_override(Ovr::Auto, true), Ovr::Keep);
        assert_eq!(next_override(Ovr::Keep, true), Ovr::Recolor);
        assert_eq!(next_override(Ovr::Recolor, true), Ovr::Auto);
        // photo (kept): recolor, keep, auto
        assert_eq!(next_override(Ovr::Auto, false), Ovr::Recolor);
        assert_eq!(next_override(Ovr::Recolor, false), Ovr::Keep);
        assert_eq!(next_override(Ovr::Keep, false), Ovr::Auto);
    }

    /// A 300x300 pt page: a 4x4 image (top half white, bottom half dark grey) at pdf (0,50)-(200,250) drawn
    /// three times: clipped to its top half, clipped to its bottom half, and whole.
    fn deck() -> Vec<u8> {
        // 8x8 RGB: white above, a spread of colours below
        let mut img = "FFFFFF".repeat(32);
        for i in 0..32u32 {
            img += &format!("{:02X}{:02X}{:02X}", i * 37 % 256, i * 91 % 256, i * 53 % 256);
        }
        img.push('>');
        let content = "q 0 150 200 100 re W n 200 0 0 200 0 50 cm /Im Do Q\n\
                       q 0 50 200 100 re W n 200 0 0 200 0 50 cm /Im Do Q\n\
                       q 200 0 0 200 0 50 cm /Im Do Q\n";
        format!(
            "%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
             3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 300]/Resources<</XObject<</Im 4 0 R>>>>/Contents 5 0 R>>endobj\n\
             4 0 obj<</Type/XObject/Subtype/Image/Width 8/Height 8/ColorSpace/DeviceRGB/BitsPerComponent 8/Filter/ASCIIHexDecode/Length {}>>stream\n{img}\nendstream endobj\n\
             5 0 obj<</Length {}>>stream\n{content}endstream endobj\ntrailer<</Root 1 0 R>>\n",
            img.len(),
            content.len()
        )
        .into_bytes()
    }

    fn near(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1.0)
    }

    #[test]
    fn placements_are_clipped_and_classified_by_crop() {
        let doc = mupdf::Document::from_bytes(&deck(), "application/pdf").unwrap();
        let page = doc.load_page(0).unwrap();
        let ps = placements(&page, 0);
        assert_eq!(ps.len(), 3);
        // pdf y is up: the top half of the image is page y 50..150
        assert!(near(ps[0].rect, [0.0, 50.0, 200.0, 150.0]), "{:?}", ps[0].rect);
        assert!(near(ps[1].rect, [0.0, 150.0, 200.0, 250.0]), "{:?}", ps[1].rect);
        assert!(near(ps[2].rect, [0.0, 50.0, 200.0, 250.0]));
        assert!(ps.iter().all(|p| near(p.full, [0.0, 50.0, 200.0, 250.0])));
        // crops of one stream get separate ids; the whole image keeps the plain stream id
        assert!(ps[0].id != ps[1].id && ps[1].id != ps[2].id && ps[0].id != ps[2].id);
        assert_eq!(ps[2].id & (1 << 63), 0);
        let cs = Colorspace::device_rgb();
        assert_eq!(classify(&ps[0], &cs), Verdict::plain(Class::Line));
        assert_eq!(classify(&ps[1], &cs), Verdict::plain(Class::Photo));
        let v = classify(&ps[2], &cs);
        assert_eq!(v.grid, Grid { nx: 4, ny: 4, bits: 0x00ff });
        assert_eq!(v.grid.show(), "LLLL/LLLL/PPPP/PPPP");
    }

    #[test]
    fn page_bounds_clip_the_visible_rect() {
        let bytes = String::from_utf8(deck()).unwrap().replace("0 50 cm /Im Do Q\nq 0 50", "-100 50 cm /Im Do Q\nq 0 50");
        let doc = mupdf::Document::from_bytes(bytes.as_bytes(), "application/pdf").unwrap();
        let ps = placements(&doc.load_page(0).unwrap(), 0);
        assert!(ps.iter().all(|p| p.rect[0] >= 0.0 && p.rect[2] <= 300.0));
    }

    #[test]
    fn cells_combine_into_a_grid_only_when_mixed() {
        assert_eq!(verdict_from_cells(&[true; 6], 3, 2), Verdict::plain(Class::Line));
        assert_eq!(verdict_from_cells(&[false; 4], 2, 2), Verdict::plain(Class::Photo));
        // 14 of 16 photo cells (or 12 of 16 line cells) collapse to one class
        let mut c = [false; 16];
        c[11] = true;
        c[15] = true;
        assert_eq!(verdict_from_cells(&c, 4, 4), Verdict::plain(Class::Photo));
        assert_eq!(verdict_from_cells(&c.map(|x| !x), 4, 4), Verdict::plain(Class::Line));
        c[..6].fill(true);
        assert!(verdict_from_cells(&c, 4, 4).grid.is_some());
        let v = verdict_from_cells(&[true, false, false, true], 2, 2);
        assert_eq!(v.class, Class::Photo);
        assert_eq!((v.grid.nx, v.grid.ny, v.grid.bits), (2, 2, 0b1001));
        assert_eq!(grid_dims(30.0, 500.0), (1, 4));
        assert_eq!(grid_dims(100.0, 100.0), (2, 2));
    }

    #[test]
    fn grid_round_trips_and_old_files_keep_only_overrides() {
        let d = tmpdir("grid");
        let mut m = ImageMap::load(&d);
        m.set_verdict(5, Verdict { class: Class::Photo, grid: Grid { nx: 3, ny: 2, bits: 0b101101 } });
        m.set_class(6, Class::Line);
        m.set_ovr(6, Ovr::Keep);
        m.sync().unwrap();
        let bytes = std::fs::read(map_path(&d)).unwrap();
        assert_eq!(bytes.len(), HEADER + 13 + 10); // only the gridded entry pays for its cells
        let l = ImageMap::load(&d);
        assert_eq!(l.grid(5), Grid { nx: 3, ny: 2, bits: 0b101101 });
        assert_eq!(l.grid(6), Grid::NONE);
        assert_eq!((l.class(5), l.class(6), l.ovr(6)), (Some(Class::Photo), Some(Class::Line), Ovr::Keep));
        // a format-1 file: classes are dropped (stale ids), overrides survive
        let mut v1 = b"SIMG".to_vec();
        v1.extend_from_slice(&[1, 3]);
        v1.extend_from_slice(&2u32.to_le_bytes());
        for (id, flags) in [(7u64, 1u8), (8, 1 | 2 << 2)] {
            v1.extend_from_slice(&id.to_le_bytes());
            v1.extend_from_slice(&[flags, 3]);
        }
        std::fs::write(map_path(&d), v1).unwrap();
        let l = ImageMap::load(&d);
        assert_eq!((l.len(), l.class(8), l.ovr(8)), (1, None, Ovr::Keep));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn splits_must_be_contiguous_and_photographic() {
        // top row line art, bottom row the rest
        let line = [true, true, true, true, false, false, false, false];
        assert!(genuine_split(&line, true, 4, 2));
        assert!(!genuine_split(&line, false, 4, 2));
        // line art in two separate blocks: no
        let sc = [true, false, false, true, false, false, false, false];
        assert!(!genuine_split(&sc, true, 4, 2));
    }
}
