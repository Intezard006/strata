//! Out-of-process whole-document analysis: `strata --analyze <file>` and `strata --write-index`.
//!
//! The child prints progress lines on stdout and writes its result to the cache directory, then
//! exits, so everything MuPDF accumulated while walking the document goes back to the OS.
//!
//!   `P <done> <total>`  progress     `S <text>`  status text
//!   `E <message>`       error        `D`         finished (result is in the cache)

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use mupdf::{Document, TextBlockContent, TextCharFlags, TextPageFlags};

use crate::cache::{self, Analysis, IndexEntry, OcrState};
use crate::heading::{self, size_key, Line};
use crate::render::open_doc;

const TEXT_MIN_CHARS: usize = 20;
const SCAN_SAMPLES: usize = 10;
const REOPEN_EVERY: usize = 100;
const PROGRESS_EVERY: usize = 8;

fn say(s: &str) {
    let mut o = std::io::stdout().lock();
    // the viewer went away: stop quietly
    if writeln!(o, "{s}").and_then(|_| o.flush()).is_err() {
        std::process::exit(0);
    }
}

/// One page of lines with their geometry, before gap analysis.
struct RawLine {
    text: String,
    size: f32,
    bold: bool,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

struct PageInfo {
    chars: usize,
    /// Short lines only (headings are short); this is what the index pass keeps.
    short: Vec<Line>,
    hist: Vec<(u32, u64)>,
}

fn analyze_page(doc: &Document, p: usize) -> Option<PageInfo> {
    let page = doc.load_page(p as i32).ok()?;
    let b = page.bounds().ok()?;
    let tp = page.to_text_page(TextPageFlags::empty()).ok()?;
    let st = tp.structured();
    let mut raw: Vec<RawLine> = Vec::new();
    let mut hist: HashMap<u32, u64> = HashMap::new();
    let mut chars = 0;
    for block in &st.blocks {
        let TextBlockContent::Text { lines } = &block.content else { continue };
        for l in lines {
            let mut sizes: HashMap<u32, usize> = HashMap::new();
            let (mut n, mut bold) = (0usize, 0usize);
            for c in l.chars.iter().filter(|c| !c.ch.is_whitespace()) {
                n += 1;
                bold += c.flags.contains(TextCharFlags::BOLD) as usize;
                *sizes.entry(size_key(c.size)).or_default() += 1;
            }
            if n == 0 {
                continue;
            }
            chars += n;
            let size_k = sizes.iter().max_by_key(|(_, &c)| c).map(|(&k, _)| k).unwrap_or(0);
            *hist.entry(size_k).or_default() += n as u64;
            let text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
            raw.push(RawLine {
                text,
                size: size_k as f32 / 2.0,
                bold: bold * 2 > n,
                x0: l.bounds.x0 - b.x0,
                x1: l.bounds.x1 - b.x0,
                y0: l.bounds.y0 - b.y0,
                y1: l.bounds.y1 - b.y0,
            });
        }
    }
    let mut short = Vec::new();
    for (i, l) in raw.iter().enumerate() {
        if l.text.split(' ').count() > 14 || l.text.len() > 140 {
            continue;
        }
        let (mut above, mut below) = (1000f32, 1000f32);
        for (j, o) in raw.iter().enumerate() {
            if i == j || o.x1 < l.x0 || o.x0 > l.x1 {
                continue;
            }
            if o.y1 <= l.y0 + 0.5 {
                above = above.min(l.y0 - o.y1);
            } else if o.y0 >= l.y1 - 0.5 {
                below = below.min(o.y0 - l.y1);
            }
        }
        short.push(Line {
            page: p as u32,
            text: l.text.clone(),
            size: l.size,
            bold: l.bold,
            y0: l.y0,
            y1: l.y1,
            gap_above: above,
            gap_below: below,
        });
    }
    Some(PageInfo { chars, short, hist: hist.into_iter().collect() })
}

/// Scanned = most of ~10 evenly spread pages have (almost) no text.
fn detect_scanned(doc: &Document, n: usize) -> bool {
    let samples = n.min(SCAN_SAMPLES);
    let mut low = 0;
    for i in 0..samples {
        let p = if samples == 1 { 0 } else { i * (n - 1) / (samples - 1) };
        let chars = analyze_page(doc, p).map_or(0, |i| i.chars);
        low += (chars < TEXT_MIN_CHARS) as usize;
    }
    low * 2 > samples
}

fn ocrmypdf_bin() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let local = home.join(".local/bin/ocrmypdf");
    if local.exists() { local } else { PathBuf::from("ocrmypdf") }
}

/// Run ocrmypdf into `<cache>/ocr.pdf`. The original is never modified.
fn run_ocr(src: &Path, dir: &Path) -> Result<(), String> {
    let out = cache::ocr_path(dir);
    let tmp = dir.join("ocr.pdf.tmp");
    let log = dir.join("ocr.log");
    let jobs = (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).max(1);
    say("S OCR running...");
    let status = Command::new(ocrmypdf_bin())
        .args(["--skip-text", "--output-type", "pdf", "--optimize", "0", "-l", "eng", "--jobs"])
        .arg(jobs.to_string())
        .arg(src)
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).map(Stdio::from).unwrap_or_else(|_| Stdio::null()))
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "ocrmypdf not found (pipx install ocrmypdf)".to_string()
            } else {
                format!("ocrmypdf: {e}")
            }
        })?;
    if !status.success() {
        let tail = std::fs::read_to_string(&log)
            .ok()
            .and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()))
            .unwrap_or_default();
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("ocrmypdf failed ({status}): {tail}"));
    }
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())
}

pub fn run(args: &[String]) -> ExitCode {
    let Some(file) = args.first().map(PathBuf::from) else {
        eprintln!("usage: strata --analyze <file> [--ocr] [--no-ocr]");
        return ExitCode::from(2);
    };
    let force_ocr = args.iter().any(|a| a == "--ocr");
    let no_ocr = args.iter().any(|a| a == "--no-ocr");
    match analyze(&file, force_ocr, no_ocr) {
        Ok(()) => {
            say("D");
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&format!("E {e}"));
            ExitCode::FAILURE
        }
    }
}

fn analyze(file: &Path, force_ocr: bool, no_ocr: bool) -> Result<(), String> {
    let dir = cache::dir_for(file).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let orig = open_doc(&file.to_path_buf())?;
    let n = orig.page_count().map_err(|e| e.to_string())? as usize;
    let scanned = detect_scanned(&orig, n);

    let ocr_file = cache::ocr_path(&dir);
    let mut ocr = OcrState::NotNeeded;
    if force_ocr || (scanned && !no_ocr) {
        if force_ocr {
            let _ = std::fs::remove_file(&ocr_file);
        }
        if !ocr_file.exists() {
            if let Err(e) = run_ocr(file, &dir) {
                say(&format!("E {e}"));
                ocr = OcrState::Failed(e);
            }
        }
    }
    // text source: the OCR copy when it exists and matches the page count
    let mut text_doc = None;
    if ocr_file.exists() {
        if let Ok(d) = open_doc(&ocr_file) {
            if d.page_count().map(|c| c as usize == n).unwrap_or(false) {
                text_doc = Some(d);
                ocr = OcrState::Done;
            }
        }
    }
    drop(orig);
    let src: &Path = if text_doc.is_some() { &ocr_file } else { file };
    let mut doc = match text_doc {
        Some(d) => d,
        None => open_doc(&file.to_path_buf())?,
    };

    say("S analysing...");
    let mut has_text = Vec::with_capacity(n);
    let mut lines: Vec<Line> = Vec::new();
    let mut hist: BTreeMap<u32, u64> = BTreeMap::new();
    for p in 0..n {
        if p > 0 && p % REOPEN_EVERY == 0 {
            // MuPDF keeps every touched page's objects; reopening bounds the child's peak memory
            if let Ok(d) = open_doc(&src.to_path_buf()) {
                doc = d;
            }
        }
        if p % PROGRESS_EVERY == 0 {
            say(&format!("P {p} {n}"));
        }
        match analyze_page(&doc, p) {
            Some(info) => {
                has_text.push(info.chars >= TEXT_MIN_CHARS);
                lines.extend(info.short);
                for (k, c) in info.hist {
                    *hist.entry(k).or_default() += c;
                }
            }
            None => has_text.push(false),
        }
    }
    let index = heading::build_index(&lines, heading::body_size(&hist), n);
    let a = Analysis { version: cache::VERSION, pages: n as u32, has_text, scanned, ocr, index, extras: Default::default() };
    cache::save(&dir, &a).map_err(|e| e.to_string())
}

/// `strata --dump-lines <file> [first] [last]`: developer aid showing the heading metrics.
pub fn dump_lines(args: &[String]) -> ExitCode {
    let Some(f) = args.first() else { return ExitCode::from(2) };
    let Ok(doc) = open_doc(&PathBuf::from(f)) else { return ExitCode::FAILURE };
    let n = doc.page_count().unwrap_or(0) as usize;
    let a: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let b: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(a).min(n.saturating_sub(1));
    eprintln!("outline entries: {} pages: {n}", doc.outlines().map_or(0, |o| o.len()));
    for p in a..=b {
        if let Some(i) = analyze_page(&doc, p) {
            for l in i.short {
                println!("{p:4} size {:5.1} b{} gap {:6.1}/{:6.1} y {:6.1} | {}", l.size, l.bold as u8, l.gap_above, l.gap_below, l.y0, l.text);
            }
        }
    }
    ExitCode::SUCCESS
}

/// `strata --write-index <src> <dst>`: copy of the PDF with the generated outline.
pub fn write_index(args: &[String]) -> ExitCode {
    let (Some(src), Some(dst)) = (args.first(), args.get(1)) else {
        eprintln!("usage: strata --write-index <src> <dst>");
        return ExitCode::from(2);
    };
    match write_index_inner(Path::new(src), dst) {
        Ok(()) => {
            say("D");
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(&format!("E {e}"));
            ExitCode::FAILURE
        }
    }
}

fn write_index_inner(src: &Path, dst: &str) -> Result<(), String> {
    let dir = cache::dir_for(src).map_err(|e| e.to_string())?;
    let a = cache::load(&dir).ok_or("no generated index (run :reindex first)")?;
    if a.index.is_empty() {
        return Err("generated index is empty".into());
    }
    let mut pdf = mupdf::pdf::PdfDocument::open(src).map_err(|_| "not a PDF".to_string())?;
    let origins: Vec<f32> = (0..a.pages as i32)
        .map(|p| pdf.load_page(p).and_then(|pg| pg.bounds()).map(|b| b.y0).unwrap_or(0.0))
        .collect();
    let toc = outline_tree(&a.index, &origins);
    pdf.set_outlines(&toc).map_err(|e| e.to_string())?;
    pdf.save(dst).map_err(|e| e.to_string())
}

/// Nest a flat, level-annotated list into outline items.
pub fn outline_tree(entries: &[IndexEntry], origins: &[f32]) -> Vec<mupdf::Outline> {
    use mupdf::document::Location;
    use mupdf::link::LinkDestination;
    fn build(entries: &[IndexEntry], i: &mut usize, level: u8, origins: &[f32]) -> Vec<mupdf::Outline> {
        let mut out = Vec::new();
        while *i < entries.len() && entries[*i].level >= level {
            let e = &entries[*i];
            *i += 1;
            let top = e.y + origins.get(e.page as usize).copied().unwrap_or(0.0);
            let down = build(entries, i, e.level + 1, origins);
            out.push(mupdf::Outline {
                title: e.title.clone(),
                uri: None,
                dest: Some(LinkDestination {
                    loc: Location { chapter: 0, page_in_chapter: e.page, page_number: e.page },
                    kind: mupdf::DestinationKind::XYZ { left: None, top: Some(top), zoom: None },
                }),
                down,
            });
        }
        out
    }
    let mut i = 0;
    build(entries, &mut i, 1, origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(t: &str, level: u8) -> IndexEntry {
        IndexEntry { title: t.into(), page: 0, y: 0.0, level }
    }

    #[test]
    fn tree_nesting() {
        let t = outline_tree(&[e("a", 1), e("a1", 2), e("a1x", 3), e("a2", 2), e("b", 1)], &[0.0]);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].down.len(), 2);
        assert_eq!(t[0].down[0].down[0].title, "a1x");
        assert!(t[1].down.is_empty());
    }
}
