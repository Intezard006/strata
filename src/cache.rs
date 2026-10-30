//! On-disk analysis cache: one directory per document, keyed by a cheap fingerprint.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump to invalidate every cached result.
pub const VERSION: u32 = 4;
const SAMPLE: u64 = 64 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub title: String,
    pub page: u32,
    /// Points from the page origin to the top of the heading line.
    pub y: f32,
    /// 1-based nesting level.
    pub level: u8,
}

/// A jump target found in the text (equation, definition, figure, ...). Filled by phase 4.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub kind: String,
    pub label: String,
    pub page: u32,
    pub y: f32,
}

/// A reference to a target in running text. Filled by phase 4.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Reference {
    pub page: u32,
    pub rect: [f32; 4],
    pub label: String,
}

/// Everything later phases add lives here so the single analysis pass stays single.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Extras {
    #[serde(default)]
    pub targets: Vec<Target>,
    #[serde(default)]
    pub refs: Vec<Reference>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub enum OcrState {
    #[default]
    NotNeeded,
    Done,
    Failed(String),
}

/// Where the index came from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum IndexSource {
    /// Font-size/spacing heuristics.
    #[default]
    Heuristic,
    /// The book's own table of contents.
    Contents,
}

/// Whether `previews.json` / `refs.bin` were produced, and how much they hold.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PreviewInfo {
    pub done: bool,
    pub targets: u32,
    pub refs: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub version: u32,
    pub pages: u32,
    pub has_text: Vec<bool>,
    pub scanned: bool,
    pub ocr: OcrState,
    pub index: Vec<IndexEntry>,
    #[serde(default)]
    pub index_source: IndexSource,
    /// Printed page numbers <-> PDF pages (for a printed-page `:goto`).
    #[serde(default)]
    pub page_map: crate::pages::PageMap,
    #[serde(default)]
    pub previews: PreviewInfo,
    #[serde(default)]
    pub extras: Extras,
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// Fingerprint of (size, mtime, first and last 64 KB).
pub fn key(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let mut h = 0xcbf29ce484222325u64;
    fnv(&mut h, &meta.len().to_le_bytes());
    if let Ok(Ok(d)) = meta.modified().map(|m| m.duration_since(std::time::UNIX_EPOCH)) {
        fnv(&mut h, &d.as_secs().to_le_bytes());
        fnv(&mut h, &d.subsec_nanos().to_le_bytes());
    }
    let mut buf = vec![0u8; SAMPLE as usize];
    let n = f.read(&mut buf)?;
    fnv(&mut h, &buf[..n]);
    if meta.len() > SAMPLE {
        f.seek(SeekFrom::Start(meta.len().saturating_sub(SAMPLE)))?;
        let n = f.read(&mut buf)?;
        fnv(&mut h, &buf[..n]);
    }
    Ok(format!("{h:016x}"))
}

pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("strata")
}

pub fn dir_for(path: &Path) -> std::io::Result<PathBuf> {
    Ok(cache_root().join(key(path)?))
}

pub fn analysis_path(dir: &Path) -> PathBuf {
    dir.join("analysis.json")
}

pub fn ocr_path(dir: &Path) -> PathBuf {
    dir.join("ocr.pdf")
}

/// Load a valid cached result, or `None` when missing, unreadable or from another version.
pub fn load(dir: &Path) -> Option<Analysis> {
    let a: Analysis = serde_json::from_slice(&std::fs::read(analysis_path(dir)).ok()?).ok()?;
    let a = (a.version == VERSION).then_some(a)?;
    touch(dir);
    Some(a)
}

/// Mark the directory as recently used (the LRU key for eviction).
pub fn touch(dir: &Path) {
    if let Ok(f) = File::open(dir) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("lock")
}

/// Exclusive advisory lock on the cache dir; released when the returned file is dropped or the
/// process dies. Blocks (no polling) while another process holds it, calling `on_wait` first.
pub fn lock(dir: &Path, on_wait: impl FnOnce()) -> std::io::Result<File> {
    use std::os::fd::AsRawFd;
    let f = File::options().create(true).append(true).open(lock_path(dir))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        on_wait();
        while unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(f)
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Delete least-recently-used cache dirs (by mtime) until the total fits `max_bytes`.
/// `keep` is never removed; dirs locked by a running analysis are skipped. Returns bytes freed.
pub fn evict(root: &Path, keep: &Path, max_bytes: u64) -> u64 {
    use std::os::fd::AsRawFd;
    let mut dirs: Vec<(std::time::SystemTime, PathBuf, u64)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok().filter(|m| m.is_dir())?;
            Some((m.modified().ok()?, e.path(), dir_size(&e.path())))
        })
        .collect();
    let mut total: u64 = dirs.iter().map(|d| d.2).sum();
    if total <= max_bytes {
        return 0;
    }
    dirs.sort_by_key(|d| d.0);
    let mut freed = 0;
    for (_, path, size) in dirs {
        if total <= max_bytes {
            break;
        }
        if path == keep {
            continue;
        }
        // skip dirs another analysis is working in
        if let Ok(f) = File::options().create(true).append(true).open(lock_path(&path)) {
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                continue;
            }
            if std::fs::remove_dir_all(&path).is_ok() {
                total -= size;
                freed += size;
            }
        }
    }
    freed
}

/// Write atomically (temp file + rename).
pub fn save(dir: &Path, a: &Analysis) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("analysis.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(a)?)?;
    std::fs::rename(tmp, analysis_path(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_version() {
        let dir = std::env::temp_dir().join(format!("strata-test-{}", std::process::id()));
        let a = Analysis {
            version: VERSION,
            pages: 3,
            has_text: vec![true, false, true],
            index: vec![IndexEntry { title: "A".into(), page: 1, y: 2.0, level: 1 }],
            ..Default::default()
        };
        save(&dir, &a).unwrap();
        assert_eq!(load(&dir), Some(a.clone()));
        save(&dir, &Analysis { version: VERSION + 1, ..a }).unwrap();
        assert_eq!(load(&dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn key_changes_with_content() {
        let dir = std::env::temp_dir().join(format!("strata-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![1u8; 200_000]).unwrap();
        let k1 = key(&f).unwrap();
        assert_eq!(k1, key(&f).unwrap());
        let mut d = vec![1u8; 200_000];
        *d.last_mut().unwrap() = 9;
        std::fs::write(&f, d).unwrap();
        assert_ne!(k1, key(&f).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn evicts_oldest_first_and_keeps_current() {
        let root = std::env::temp_dir().join(format!("strata-evict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("ocr.pdf"), vec![0u8; 1000]).unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 * (i as u64 + 1));
            File::open(&d).unwrap().set_modified(t).unwrap();
        }
        let freed = evict(&root, &root.join("old"), 2000);
        assert_eq!(freed, 1000);
        assert!(root.join("old").exists(), "current dir is kept");
        assert!(!root.join("mid").exists());
        assert!(root.join("new").exists());
        assert_eq!(evict(&root, &root.join("old"), 1 << 30), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lock_is_exclusive_across_handles() {
        use std::os::fd::AsRawFd;
        let dir = std::env::temp_dir().join(format!("strata-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = lock(&dir, || panic!("uncontended")).unwrap();
        let b = File::options().create(true).append(true).open(lock_path(&dir)).unwrap();
        assert_ne!(unsafe { libc::flock(b.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        drop(a);
        assert_eq!(unsafe { libc::flock(b.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extras_default_when_absent() {
        let j = r#"{"version":3,"pages":1,"has_text":[true],"scanned":false,"ocr":"NotNeeded","index":[]}"#;
        let a: Analysis = serde_json::from_str(j).unwrap();
        assert!(a.extras.targets.is_empty());
    }
}
