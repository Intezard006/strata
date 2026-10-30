//! On-disk analysis cache: one directory per document, keyed by a cheap fingerprint.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump to invalidate every cached result.
pub const VERSION: u32 = 1;
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

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub version: u32,
    pub pages: u32,
    pub has_text: Vec<bool>,
    pub scanned: bool,
    pub ocr: OcrState,
    pub index: Vec<IndexEntry>,
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
    (a.version == VERSION).then_some(a)
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
    fn extras_default_when_absent() {
        let j = r#"{"version":1,"pages":1,"has_text":[true],"scanned":false,"ocr":"NotNeeded","index":[]}"#;
        let a: Analysis = serde_json::from_str(j).unwrap();
        assert!(a.extras.targets.is_empty());
    }
}
