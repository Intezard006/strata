//! Previews on disk: `previews.json` (targets) and `refs.bin` (references, indexed by page so
//! the viewer reads only the pages on screen).

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::cache::IndexEntry;
use crate::previews::{link_target, section_targets, Kind, RawRef, Ref, Resolver, Target};

const MAGIC: &[u8; 4] = b"SPR1";
const REC: usize = 20;

/// An internal PDF link found on `page`.
pub struct RawLink {
    pub page: u32,
    pub rect: [f32; 4],
    pub dest_page: u32,
    pub dest_y: f32,
}

pub struct Built {
    pub targets: Vec<Target>,
    /// Sorted by page.
    pub refs: Vec<(u32, Ref)>,
}

/// Chapter id per page from the index (the level of the first `Chapter` entry, else level 1).
pub fn chapter_ids(index: &[IndexEntry], pages: usize) -> Vec<u32> {
    let level = index
        .iter()
        .find(|e| crate::previews::section_label(&e.title).is_some_and(|k| k.0 == Kind::Chapter))
        .map_or(1, |e| e.level);
    let mut starts: Vec<u32> = index.iter().filter(|e| e.level <= level).map(|e| e.page).collect();
    starts.sort_unstable();
    (0..pages as u32).map(|p| starts.partition_point(|&s| s <= p) as u32).collect()
}

fn overlaps(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

pub fn build(
    mut targets: Vec<Target>,
    raw: Vec<RawRef>,
    links: Vec<RawLink>,
    index: &[IndexEntry],
    sizes: &[(f32, f32)],
    skip: &[bool],
) -> Built {
    let skipped = |p: u32| skip.get(p as usize).copied().unwrap_or(false);
    targets.retain(|t| !skipped(t.page));
    targets.extend(section_targets(index, sizes).into_iter().filter(|t| !skipped(t.page)));
    // the same label twice on a page is one target
    let mut seen = std::collections::HashSet::new();
    targets.retain(|t| seen.insert((t.kind, t.label.clone(), t.page)));

    let chapters = chapter_ids(index, sizes.len());
    let mut refs: Vec<(u32, Ref)> = Vec::new();
    {
        let mut by_key: HashMap<(Kind, &str), Vec<u32>> = HashMap::new();
        for (i, t) in targets.iter().enumerate() {
            by_key.entry((t.kind, t.label.as_str())).or_default().push(i as u32);
        }
        let rs = Resolver { targets: &targets, chapter_of: &chapters };
        for r in raw.iter().filter(|r| !skipped(r.page)) {
            if let Some(t) = rs.resolve(&by_key, r) {
                refs.push((r.page, Ref { rect: r.rect, target: t }));
            }
        }
    }
    // internal links: exact, so they win over a text reference at the same spot
    let mut synth = HashMap::new();
    let mut link_refs: Vec<(u32, Ref)> = Vec::new();
    for l in links.iter().filter(|l| !skipped(l.page)) {
        let Some(&size) = sizes.get(l.dest_page as usize) else { continue };
        let t = link_target(&mut targets, &mut synth, l.dest_page, l.dest_y, size);
        let tg = &targets[t as usize];
        if tg.page == l.page && overlaps(&l.rect, &tg.rect) {
            continue; // links to itself
        }
        link_refs.push((l.page, Ref { rect: l.rect, target: t }));
    }
    refs.retain(|(p, r)| !link_refs.iter().any(|(lp, l)| lp == p && overlaps(&l.rect, &r.rect)));
    refs.extend(link_refs);
    refs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.rect[1].total_cmp(&b.1.rect[1])).then(a.1.rect[0].total_cmp(&b.1.rect[0])));
    Built { targets, refs }
}

pub fn json_path(dir: &Path) -> std::path::PathBuf {
    dir.join("previews.json")
}

pub fn refs_path(dir: &Path) -> std::path::PathBuf {
    dir.join("refs.bin")
}

pub fn write(dir: &Path, b: &Built, pages: usize) -> std::io::Result<()> {
    let mut offs = vec![0u32; pages + 1];
    for (p, _) in &b.refs {
        if (*p as usize) < pages {
            offs[*p as usize + 1] += 1;
        }
    }
    for i in 0..pages {
        offs[i + 1] += offs[i];
    }
    let mut buf = Vec::with_capacity(8 + 4 * offs.len() + REC * b.refs.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&(pages as u32).to_le_bytes());
    for o in &offs {
        buf.extend_from_slice(&o.to_le_bytes());
    }
    for (_, r) in &b.refs {
        for v in r.rect {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&r.target.to_le_bytes());
    }
    let tmp = dir.join("refs.bin.tmp");
    std::fs::write(&tmp, buf)?;
    std::fs::rename(&tmp, refs_path(dir))?;
    // rects rounded to 0.1 pt keep the JSON small
    let round = |v: f32| (v * 10.0).round() / 10.0;
    let ts: Vec<Target> = b.targets.iter().cloned().map(|mut t| {
        t.rect = t.rect.map(round);
        t
    }).collect();
    let tmp = dir.join("previews.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&ts)?)?;
    std::fs::rename(&tmp, json_path(dir))
}

/// Read side: targets in memory, references read per page.
pub struct Store {
    pub targets: Vec<Target>,
    file: File,
    offs: Vec<u32>,
}

impl Store {
    pub fn open(dir: &Path) -> Option<Store> {
        let targets: Vec<Target> = serde_json::from_slice(&std::fs::read(json_path(dir)).ok()?).ok()?;
        let file = File::open(refs_path(dir)).ok()?;
        let mut head = [0u8; 8];
        file.read_exact_at(&mut head, 0).ok()?;
        if &head[..4] != MAGIC {
            return None;
        }
        let n = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
        let mut raw = vec![0u8; 4 * (n + 1)];
        file.read_exact_at(&mut raw, 8).ok()?;
        let offs = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        Some(Store { targets, file, offs })
    }

    pub fn ref_count(&self) -> usize {
        self.offs.last().copied().unwrap_or(0) as usize
    }

    pub fn refs_on(&self, page: u32) -> Vec<Ref> {
        let Some((&a, &b)) = self.offs.get(page as usize).zip(self.offs.get(page as usize + 1)) else { return vec![] };
        let mut raw = vec![0u8; (b - a) as usize * REC];
        let base = 8 + 4 * self.offs.len() as u64;
        if self.file.read_exact_at(&mut raw, base + a as u64 * REC as u64).is_err() {
            return vec![];
        }
        raw.chunks_exact(REC)
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap());
                Ref { rect: [f(0), f(1), f(2), f(3)], target: u32::from_le_bytes(c[16..20].try_into().unwrap()) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_per_page_reads() {
        let dir = std::env::temp_dir().join(format!("strata-pstore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = |k, l: &str, p| Target { kind: k, label: l.into(), page: p, rect: [10.0, 20.0, 300.0, 80.0], text: "x".into() };
        let targets = vec![t(Kind::Theorem, "1", 2), t(Kind::Equation, "3.2", 4)];
        let raw = vec![
            RawRef { page: 4, rect: [50.0, 300.0, 90.0, 310.0], kind: Kind::Theorem, label: "1".into() },
            RawRef { page: 0, rect: [50.0, 100.0, 90.0, 110.0], kind: Kind::Equation, label: "3.2".into() },
            RawRef { page: 0, rect: [50.0, 200.0, 90.0, 210.0], kind: Kind::Lemma, label: "9".into() }, // unknown
        ];
        let b = build(targets, raw, vec![], &[], &[(612.0, 792.0); 6], &[]);
        assert_eq!(b.refs.len(), 2);
        write(&dir, &b, 6).unwrap();
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.targets.len(), 2);
        assert_eq!(s.ref_count(), 2);
        assert_eq!(s.refs_on(0).len(), 1);
        assert_eq!(s.refs_on(0)[0].target, 1);
        assert_eq!(s.refs_on(4)[0].rect, [50.0, 300.0, 90.0, 310.0]);
        assert!(s.refs_on(1).is_empty() && s.refs_on(99).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn links_attach_to_targets_and_beat_text_refs() {
        let t = Target { kind: Kind::Theorem, label: "1".into(), page: 2, rect: [10.0, 100.0, 300.0, 180.0], text: String::new() };
        let raw = vec![RawRef { page: 5, rect: [50.0, 300.0, 90.0, 310.0], kind: Kind::Theorem, label: "1".into() }];
        let links = vec![
            RawLink { page: 5, rect: [48.0, 298.0, 92.0, 312.0], dest_page: 2, dest_y: 110.0 },
            RawLink { page: 5, rect: [50.0, 500.0, 90.0, 510.0], dest_page: 3, dest_y: 400.0 },
        ];
        let b = build(vec![t], raw, links, &[], &[(612.0, 792.0); 6], &[]);
        assert_eq!(b.refs.len(), 2, "text ref replaced by the link, plus the unrelated link");
        assert_eq!(b.targets[b.refs[0].1.target as usize].kind, Kind::Theorem);
        assert_eq!(b.targets[b.refs[1].1.target as usize].kind, Kind::Link);
    }

    #[test]
    fn chapters_from_index() {
        let e = |t: &str, p, l| IndexEntry { title: t.into(), page: p, y: 0.0, level: l };
        let idx = vec![e("Part 1 A", 1, 1), e("Chapter 1 X", 2, 2), e("1 Sec", 3, 3), e("Chapter 2 Y", 10, 2)];
        let c = chapter_ids(&idx, 14);
        assert_eq!(c[3], c[9]);
        assert_ne!(c[9], c[10]);
    }
}
