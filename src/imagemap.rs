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
pub const CLASSIFIER_VERSION: u8 = 2;

const MAGIC: &[u8; 4] = b"SIMG";
const FORMAT: u8 = 1;
const HEADER: usize = 10;
const ENTRY: usize = 10;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub class: Class,
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
    let mut out = Vec::with_capacity(HEADER + ids.len() * ENTRY);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.push(complete);
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in ids {
        let e = map[id];
        out.extend_from_slice(&id.to_le_bytes());
        out.push(e.class as u8 | (e.ovr as u8) << 2);
        out.push(e.ver);
    }
    out
}

fn decode(b: &[u8]) -> Option<(u8, HashMap<u64, Entry>)> {
    if b.len() < HEADER || &b[..4] != MAGIC || b[4] != FORMAT {
        return None;
    }
    let n = u32::from_le_bytes(b[6..10].try_into().ok()?) as usize;
    if b.len() != HEADER + n * ENTRY {
        return None;
    }
    let map = b[HEADER..]
        .chunks_exact(ENTRY)
        .map(|c| {
            let id = u64::from_le_bytes(c[..8].try_into().unwrap());
            (id, Entry { class: Class::from_bits(c[8]), ovr: Ovr::from_bits(c[8] >> 2), ver: c[9] })
        })
        .collect();
    Some((b[5], map))
}

/// Merge the entries `mine` marked in `pending` into `base`.
///  - an override always wins over what is on disk, and is never touched by a classification;
///  - a classification replaces a missing, older-version or `Scan` placeholder entry, but never an entry
///    of the current version (the first writer wins, so a lazy result is not clobbered by the child);
///  - entries without information (unknown class, no override) are dropped.
fn merge(base: &mut HashMap<u64, Entry>, mine: &HashMap<u64, Entry>, pending: &HashMap<u64, u8>) {
    for (id, &flags) in pending {
        let Some(m) = mine.get(id) else { continue };
        let mut b = base.get(id).copied().unwrap_or(Entry { class: Class::Unknown, ver: 0, ovr: Ovr::Auto });
        if flags & CLASS_DIRTY != 0 && m.class != Class::Unknown {
            let replace = b.class == Class::Unknown || b.ver < m.ver || (b.ver == m.ver && b.class == Class::Scan && m.class != Class::Scan);
            if replace {
                b.class = m.class;
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

    pub fn ovr(&self, id: u64) -> Ovr {
        self.map.get(&id).map_or(Ovr::Auto, |e| e.ovr)
    }

    pub fn set_class(&mut self, id: u64, class: Class) {
        let e = self.map.entry(id).or_insert(Entry { class, ver: CLASSIFIER_VERSION, ovr: Ovr::Auto });
        e.class = class;
        e.ver = CLASSIFIER_VERSION;
        *self.pending.entry(id).or_default() |= CLASS_DIRTY;
    }

    pub fn set_ovr(&mut self, id: u64, ovr: Ovr) {
        let e = self.map.entry(id).or_insert(Entry { class: Class::Unknown, ver: 0, ovr });
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

/// Id of `img` placed at `rect` on `page`.
pub fn image_id(img: &Image, page: u32, rect: [f32; 4]) -> u64 {
    let c = ctx();
    if !c.is_null() {
        unsafe {
            let cb = mupdf_sys::fz_compressed_image_buffer(c, raw(img));
            if !cb.is_null() && !(*cb).buffer.is_null() {
                let mut data: *mut u8 = std::ptr::null_mut();
                let n = mupdf_sys::fz_buffer_storage(c, (*cb).buffer, &mut data);
                if !data.is_null() && n > 0 {
                    return stream_id(img.width(), img.height(), std::slice::from_raw_parts(data, n));
                }
            }
        }
    }
    placement_id(page, rect, img.width(), img.height())
}

// ---- placements and classification -------------------------------------------------------------

/// An image placement on a page (points from the page origin).
pub struct Placement {
    pub rect: [f32; 4],
    pub id: u64,
    pub image: Image,
}

impl Placement {
    pub fn area(&self) -> f32 {
        (self.rect[2] - self.rect[0]) * (self.rect[3] - self.rect[1])
    }
}

/// Image placements of a page via the structured-text device (images are referenced, not decoded).
pub fn placements(page: &mupdf::Page, page_no: u32) -> Vec<Placement> {
    let Ok(b) = page.bounds() else { return Vec::new() };
    let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::PRESERVE_IMAGES) else { return Vec::new() };
    let mut out = Vec::new();
    for blk in tp.blocks() {
        if blk.r#type() != mupdf::text_page::TextBlockType::Image {
            continue;
        }
        let r = blk.bounds();
        let rect = [r.x0 - b.x0, r.y0 - b.y0, r.x1 - b.x0, r.y1 - b.y0];
        if rect[2] - rect[0] < 4.0 || rect[3] - rect[1] < 4.0 {
            continue; // rules and bullets drawn as images
        }
        let Some(image) = blk.image() else { continue };
        out.push(Placement { rect, id: image_id(&image, page_no, rect), image });
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

/// Classify the whole image (not a tile of it, so the result does not depend on zoom): drawn alone onto
/// a white 96 px thumbnail, which makes MuPDF decode it subsampled instead of at full size.
pub fn classify(img: &Image, cs: &Colorspace) -> Class {
    CLASSIFY_CALLS.fetch_add(1, Ordering::Relaxed);
    if std::env::var_os("STRATA_DEBUG").is_some() {
        eprintln!("imagemap: classifying {}x{} image", img.width(), img.height());
    }
    if img.color_space().is_none() {
        return Class::Line; // a 1-bit stencil: ink by definition
    }
    let (w, h) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let s = (96.0 / w.max(h)).min(1.0);
    let (pw, ph) = ((w * s).ceil().max(1.0) as i32, (h * s).ceil().max(1.0) as i32);
    let Ok(mut pix) = Pixmap::new_with_rect(cs, IRect::new(0, 0, pw, ph), false) else { return Class::Photo };
    if pix.clear_with(255).is_err() {
        return Class::Photo;
    }
    {
        let Ok(dev) = Device::from_pixmap(&pix) else { return Class::Photo };
        if dev.fill_image(img, &Matrix::new(pw as f32, 0.0, 0.0, ph as f32, 0.0, 0.0), 1.0, ColorParams::default()).is_err() {
            return Class::Photo;
        }
    }
    let n = pix.n() as usize;
    let (stride, sm) = (pix.stride() as usize, pix.samples());
    let line = crate::recolor::is_line_art((0..ph as usize).flat_map(|y| (0..pw as usize).map(move |x| (x, y))).map(|(x, y)| {
        let p = &sm[y * stride + x * n..];
        [p[0], p[1], p[2]]
    }));
    if line { Class::Line } else { Class::Photo }
}

/// Class of a placement: from the map if known, else classified (and recorded). `Scan` is by geometry.
pub fn class_of(map: &std::sync::Mutex<ImageMap>, p: &Placement, cs: &Colorspace) -> Class {
    if let Some(c) = map.lock().ok().and_then(|m| m.class(p.id)) {
        if c != Class::Scan {
            return c;
        }
    }
    let c = classify(&p.image, cs);
    if let Ok(mut m) = map.lock() {
        m.set_class(p.id, c);
    }
    c
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
        assert_eq!(std::fs::metadata(map_path(&d)).unwrap().len() as usize, HEADER + 4 * ENTRY);
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
        base.insert(1, Entry { class: Class::Line, ver: CLASSIFIER_VERSION - 1, ovr: Ovr::Auto });
        base.insert(2, Entry { class: Class::Line, ver: CLASSIFIER_VERSION - 1, ovr: Ovr::Keep });
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
}
