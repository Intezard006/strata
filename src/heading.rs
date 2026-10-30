//! Heading detection from per-line text metrics. Pure functions, no MuPDF types.

use std::collections::{BTreeMap, HashMap};

use crate::cache::IndexEntry;

#[derive(Clone, Debug)]
pub struct Line {
    pub page: u32,
    pub x0: f32,
    pub text: String,
    pub size: f32,
    pub bold: bool,
    pub y0: f32,
    pub y1: f32,
    /// Vertical distance to the nearest line above / below (overlapping horizontally).
    pub gap_above: f32,
    pub gap_below: f32,
}

/// Font size bucket (0.5 pt).
pub fn size_key(size: f32) -> u32 {
    (size * 2.0).round().max(0.0) as u32
}

/// Most common font size, weighted by characters.
pub fn body_size(hist: &BTreeMap<u32, u64>) -> f32 {
    hist.iter().max_by_key(|(_, &n)| n).map_or(10.0, |(&k, _)| k as f32 / 2.0)
}

const KEYWORDS: [(&str, u8); 6] =
    [("chapter", 1), ("part", 1), ("appendix", 1), ("section", 2), ("lecture", 1), ("unit", 1)];
const MAX_WORDS: usize = 12;

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Lowercase with digits collapsed, so "Page 12" and "Page 13" match.
fn normalize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        let c = if c.is_ascii_digit() { '#' } else { c };
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else if c != '#' || !out.ends_with('#') {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn number_like(s: &str) -> bool {
    let t = s.trim();
    if t.chars().all(|c| c.is_ascii_digit() || " .,-–—()[]/".contains(c)) {
        return true;
    }
    let roman = |c: char| "ivxlcdm".contains(c.to_ascii_lowercase());
    t.len() <= 8 && t.chars().all(roman) && (t.chars().all(|c| c.is_lowercase()) || t.chars().all(|c| c.is_uppercase()))
}

/// Leading `1`, `1.2`, `1.2.3` (optionally dotted) followed by text: returns (depth, rest).
pub fn numbering(s: &str) -> Option<(usize, &str)> {
    let s = s.strip_prefix('§').map_or(s, str::trim_start);
    let b = s.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    loop {
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits = i - st;
        if digits == 0 || (depth == 0 && digits > 2) || digits > 3 {
            return None;
        }
        depth += 1;
        if i < b.len() && b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
    }
    let rest = &s[i..];
    if depth > 3 || !rest.starts_with(char::is_whitespace) || rest.trim().is_empty() {
        return None;
    }
    Some((depth, rest.trim_start()))
}

fn keyword(s: &str) -> Option<u8> {
    let mut it = s.split_whitespace();
    let first = it.next()?.to_lowercase();
    let second = it.next()?;
    let lvl = KEYWORDS.iter().find(|(k, _)| *k == first)?.1;
    let ok = second.chars().next().is_some_and(|c| c.is_ascii_digit() || c.is_uppercase());
    ok.then_some(lvl)
}

/// Mostly symbols/digits: formulas, table rows, figure axes.
fn equation_like(s: &str) -> bool {
    let (mut letters, mut total) = (0, 0);
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        total += 1;
        letters += c.is_alphabetic() as usize;
    }
    let symbols = s.chars().filter(|c| "=<>+±×÷∈∑∫{}[]|^~_\\".contains(*c)).count();
    total == 0 || (letters as f32) < 0.6 * total as f32 || symbols >= 2 || (symbols >= 1 && total < 14)
}

/// At least one run of 4+ letters (filters `AU B`, `f : A`).
fn has_real_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphabetic()).any(|w| w.chars().count() >= 4)
}

/// Run-in headings that are content, not sections.
fn run_in_label(s: &str) -> bool {
    let w = s.split_whitespace().next().unwrap_or("").trim_end_matches(['.', ':']).to_lowercase();
    ["definition", "theorem", "lemma", "corollary", "example", "proof", "remark", "exercise", "figure", "table", "proposition", "claim", "note"]
        .contains(&w.as_str())
}

/// A lowercase letter directly followed by an uppercase one inside a word: OCR garbage like `AlUAU`.
fn odd_case(s: &str) -> bool {
    s.split_whitespace().any(|w| {
        let c: Vec<char> = w.chars().collect();
        c.windows(2).any(|p| p[0].is_lowercase() && p[1].is_uppercase()) && c.iter().filter(|x| x.is_alphabetic()).count() > 2
    })
}

const IMPERATIVES: [&str; 26] = [
    "show", "prove", "let", "find", "suppose", "determine", "verify", "compute", "check", "give", "consider", "explain",
    "write", "express", "describe", "define", "assume", "if", "is", "does", "which", "what", "why", "how", "note", "use",
];

/// Exercise lines: `3. (a) Show that`, or a leading imperative maths verb (after any numbering).
fn exercise_like(s: &str) -> bool {
    let t = s.trim();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 3 {
        let rest = t[digits..].trim_start_matches('.').trim_start();
        let mut c = rest.chars();
        if let (Some(a), Some(b)) = (c.next(), c.next()) {
            if (a == '(' && b.is_alphabetic() && c.next() == Some(')')) || (a.is_lowercase() && b == ')') {
                return true;
            }
        }
    }
    let body = numbering(t).map_or(t, |n| n.1);
    let first = body.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphabetic()).to_lowercase();
    IMPERATIVES.contains(&first.as_str())
}

/// Back-of-book index entry: `Ordered square, 13, 80`, `13 (see Ordered square)`.
pub fn index_line(s: &str) -> bool {
    let t = s.trim();
    if t.contains("(see ") || t.starts_with("see ") || t.contains(" see also ") {
        return true;
    }
    // trailing run of page numbers after a comma: ", 12, 34-36"
    let mut nums = 0;
    let mut rest = t;
    while let Some((head, last)) = rest.rsplit_once(|c| c == ',' || c == ' ') {
        let l = last.trim();
        if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '–') && l.chars().any(|c| c.is_ascii_digit()) {
            nums += 1;
            rest = head.trim_end_matches(',').trim_end();
        } else {
            break;
        }
    }
    nums >= 1 && t.contains(',') && rest.chars().any(char::is_alphabetic) && rest.split_whitespace().count() <= 8 && {
        // "Word, 12" - the comma must precede the numbers
        t[rest.len()..].trim_start().starts_with(',')
    }
}

/// Pages that look like a back-of-book index: most short lines are index entries.
pub fn index_pages(lines: &[Line], pages: usize) -> Vec<bool> {
    let mut tot = vec![0u32; pages];
    let mut hit = vec![0u32; pages];
    for l in lines {
        if (l.page as usize) < pages {
            tot[l.page as usize] += 1;
            hit[l.page as usize] += index_line(&l.text) as u32;
        }
    }
    (0..pages).map(|p| tot[p] >= 8 && hit[p] * 2 >= tot[p]).collect()
}

/// `1)`, `(a)`, `ii)` list markers.
fn list_marker(s: &str) -> bool {
    let w = s.split_whitespace().next().unwrap_or("");
    let t = w.trim_start_matches('(');
    t.len() <= 4 && t.ends_with(')') && t.len() >= 2
}

struct Cand<'a> {
    line: &'a Line,
    text: String,
    depth: Option<usize>,
    kw: Option<u8>,
    big: bool,
}

fn candidate<'a>(l: &'a Line, body: f32, repeated: &dyn Fn(&str) -> bool) -> Option<Cand<'a>> {
    let t = l.text.trim();
    if t.chars().count() < 2 || words(t) > MAX_WORDS || number_like(t) || repeated(&normalize(t)) {
        return None;
    }
    if !t.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '§') || list_marker(t) {
        return None;
    }
    let num = numbering(t);
    let kw = keyword(t);
    if run_in_label(num.map_or(t, |n| n.1)) || run_in_label(t) || exercise_like(t) || t.contains("(see ") || odd_case(t) || (t.contains(',') && words(t) >= 6) {
        return None;
    }
    // fragments of sentences start in lowercase; real headings do not
    if !t.chars().find(|c| c.is_alphanumeric()).is_some_and(|c| !c.is_lowercase()) {
        return None;
    }
    if equation_like(num.map_or(t, |n| n.1)) {
        return None;
    }
    let last = t.chars().last().unwrap();
    if matches!(last, ',' | ';') || (last == ':' && kw.is_none()) || (last == '.' && words(t) >= 4) {
        return None;
    }
    let big = l.size >= body * 1.15;
    let gap_hi = l.gap_above > 0.6 * l.size;
    let bold_iso = l.bold && l.size >= body * 0.95 && gap_hi && l.gap_below > 0.4 * l.size;
    let spaced = gap_hi || l.gap_below > 0.5 * l.size;
    let ok = (kw.is_some() && (big || l.bold || gap_hi))
        || (num.is_some_and(|n| words(n.1) >= 2 || n.0 > 1) && (big || l.bold || l.gap_above > l.size))
        || (big && spaced && has_real_word(t))
        || bold_iso;
    ok.then(|| Cand { line: l, text: t.to_string(), depth: num.map(|n| n.0), kw, big })
}

struct Merged {
    page: u32,
    text: String,
    y: f32,
    y1: f32,
    size: f32,
    bold: bool,
    depth: Option<usize>,
    kw: Option<u8>,
    big: bool,
}

/// `Chapter 3`, `Part II`, `Appendix A`, `§4` on their own.
pub fn bare_marker(s: &str) -> bool {
    let t = s.trim();
    let mut it = t.split_whitespace();
    let (Some(a), Some(b), None) = (it.next(), it.next(), it.next()) else {
        return t.strip_prefix('§').is_some_and(|r| !r.is_empty() && r.len() <= 5 && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
    };
    KEYWORDS.iter().any(|(k, _)| a.eq_ignore_ascii_case(k)) && b.len() <= 5 && b.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Join a bare `Chapter N` line with the title line that follows it (same page, close below).
pub fn merge_markers(mut lines: Vec<Line>) -> Vec<Line> {
    lines.sort_by(|a, b| a.page.cmp(&b.page).then(a.y0.total_cmp(&b.y0)));
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].clone();
        if bare_marker(&l.text) {
            if let Some(n) = lines.get(i + 1).filter(|n| {
                n.page == l.page
                    && n.y0 - l.y1 < 2.5 * l.size.max(8.0)
                    && n.y0 >= l.y0 - 1.0
                    && !bare_marker(&n.text)
                    && numbering(&n.text).is_none()
                    && keyword(&n.text).is_none()
                    && words(&n.text) <= MAX_WORDS
                    && n.text.chars().find(|c| c.is_alphabetic()).is_some_and(char::is_uppercase)
                    && !n.text.ends_with('.')
            }) {
                out.push(Line {
                    text: format!("{} {}", l.text.trim(), n.text.trim()),
                    size: l.size.max(n.size),
                    bold: l.bold || n.bold,
                    y1: n.y1,
                    gap_below: n.gap_below,
                    ..l
                });
                i += 2;
                continue;
            }
        }
        out.push(l);
        i += 1;
    }
    out
}

fn preface_like(t: &str) -> bool {
    let w = t.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphabetic()).to_lowercase();
    ["preface", "introduction", "foreword", "prologue", "acknowledgments", "acknowledgements", "abstract", "overview"].contains(&w.as_str())
}

/// Build the index from the short lines of a whole document (any order).
/// `skip[p]`: pages excluded from heading detection (contents, back-of-book index).
/// `contents_start`: first Contents page, when one was found.
pub fn build_index(lines: &[Line], body: f32, pages: usize, skip: &[bool], contents_start: Option<u32>) -> Vec<IndexEntry> {
    let kept: Vec<Line> = lines.iter().filter(|l| !skip.get(l.page as usize).copied().unwrap_or(false)).cloned().collect();
    let lines = &merge_markers(kept)[..];
    // running headers and footers: the same normalised text on many pages
    let mut seen: HashMap<String, (u32, u32)> = HashMap::new();
    for l in lines {
        let e = seen.entry(normalize(l.text.trim())).or_insert((u32::MAX, 0));
        if e.0 != l.page {
            *e = (l.page, e.1 + 1);
        }
    }
    let limit = ((pages as f32 * 0.3) as u32).max(2);
    let repeated = |n: &str| seen.get(n).is_some_and(|e| e.1 > limit);

    let mut cands: Vec<Cand> = lines.iter().filter_map(|l| candidate(l, body, &repeated)).collect();
    cands.sort_by(|a, b| a.line.page.cmp(&b.line.page).then(a.line.y0.total_cmp(&b.line.y0)));

    let mut merged: Vec<Merged> = Vec::new();
    for c in cands {
        let l = c.line;
        if let Some(m) = merged.last_mut() {
            let same_row = m.page == l.page && (l.y0 - m.y).abs() < 2.0 && c.depth.is_none() && c.kw.is_none();
            let continues = same_row
                || (m.page == l.page
                    && (m.size - l.size).abs() < 0.3
                    && m.bold == l.bold
                    && l.y0 - m.y1 < 0.8 * l.size
                    && c.depth.is_none()
                    && c.kw.is_none());
            if continues {
                m.text.push(' ');
                m.text.push_str(&c.text);
                m.y1 = l.y1;
                continue;
            }
        }
        merged.push(Merged {
            page: l.page,
            text: c.text,
            y: l.y0,
            y1: l.y1,
            size: l.size,
            bold: l.bold,
            depth: c.depth,
            kw: c.kw,
            big: c.big,
        });
    }

    // distinct large sizes, clustered within 1 pt, largest = level 1
    let mut sizes: Vec<f32> = merged.iter().filter(|m| m.big).map(|m| m.size).collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    let mut clusters: Vec<f32> = Vec::new();
    for s in sizes {
        if clusters.last().is_none_or(|&c| c - s > 1.0) {
            clusters.push(s);
        }
    }
    let nlev = clusters.len().min(3) as u8;
    let size_level = |s: f32| -> u8 {
        let i = clusters.iter().position(|&c| (c - s).abs() <= 1.0).unwrap_or(clusters.len());
        (i as u8 + 1).min(nlev.max(1))
    };

    let mut out: Vec<IndexEntry> = Vec::new();
    for m in merged {
        let mut level = if m.big { size_level(m.size) } else { (nlev + 1).min(4) };
        if let Some(d) = m.depth.filter(|&d| d > 1 || m.big) {
            level = d as u8;
        } else if let Some(k) = m.kw {
            level = k;
        }
        if out.last().is_some_and(|p| p.page == m.page && p.title == m.text) {
            continue;
        }
        // the same heading again within a few pages (a running chapter title, a repeated caption)
        if out.iter().rev().take(6).any(|p| p.title == m.text && p.level == level && m.page.saturating_sub(p.page) <= 3) {
            continue;
        }
        out.push(IndexEntry { title: m.text, page: m.page, y: m.y, level });
        if out.len() >= 5000 {
            break;
        }
    }
    drop_front_matter(out, contents_start)
}

/// Turn natural levels into a proper nesting: an entry is a child of the nearest earlier entry
/// with a smaller natural level, so `3 3 3 1 3` becomes `1 1 1 1 2`.
pub fn relevel(mut entries: Vec<IndexEntry>) -> Vec<IndexEntry> {
    let mut stack: Vec<u8> = Vec::new();
    for e in &mut entries {
        while stack.last().is_some_and(|&t| t >= e.level) {
            stack.pop();
        }
        stack.push(e.level);
        e.level = stack.len() as u8;
    }
    entries
}

/// Title-page junk: everything before the first Contents page, or (without one) before the
/// first Chapter/Part, except Preface/Introduction-like entries. Without either, a lone title
/// page (page 0) in front of a longer index is dropped.
fn drop_front_matter(out: Vec<IndexEntry>, contents_start: Option<u32>) -> Vec<IndexEntry> {
    let first_kw = out
        .iter()
        .find(|e| keyword(&e.title).is_some_and(|k| k == 1) && !e.title.to_lowercase().starts_with("lecture"))
        .map(|e| e.page);
    let cut = contents_start.or(first_kw).or_else(|| (out.len() >= 5 && out.iter().filter(|e| e.page == 0).count() <= 3).then_some(1));
    let Some(cut) = cut else { return relevel(out) };
    relevel(out.into_iter().filter(|e| e.page >= cut || preface_like(&e.title)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(page: u32, y0: f32, text: &str, size: f32, bold: bool) -> Line {
        Line { page, x0: 50.0, text: text.into(), size, bold, y0, y1: y0 + size, gap_above: 30.0, gap_below: 30.0 }
    }

    /// Body-size paragraph line (tight spacing, so never "isolated").
    fn body_line(page: u32, y0: f32, text: &str) -> Line {
        Line { gap_above: 1.0, gap_below: 1.0, ..line(page, y0, text, 10.0, false) }
    }

    #[test]
    fn body_size_is_char_weighted() {
        let mut h = BTreeMap::new();
        h.insert(size_key(10.0), 5000);
        h.insert(size_key(18.0), 300);
        h.insert(size_key(8.0), 900);
        assert_eq!(body_size(&h), 10.0);
    }

    #[test]
    fn numbering_parse() {
        assert_eq!(numbering("1.2.3 Foo bar").unwrap().0, 3);
        assert_eq!(numbering("4. Intro").unwrap(), (1, "Intro"));
        assert!(numbering("1.2.3.4 Foo").is_none());
        assert!(numbering("2024 was a year").is_none());
        assert!(numbering("12").is_none());
        assert!(numbering("3.14159 is pi").is_none());
    }

    #[test]
    fn big_and_bold_headings_with_levels() {
        let lines = vec![
            line(1, 50.0, "Introduction", 18.0, true),
            body_line(1, 90.0, "some body text here that is long enough to be a paragraph of text"),
            line(1, 200.0, "Background", 14.0, true),
            line(2, 40.0, "Methods", 18.0, true),
            line(2, 120.0, "Sampling details", 10.0, true),
        ];
        let idx = build_index(&lines, 10.0, 10, &[], None);
        let t: Vec<(&str, u8)> = idx.iter().map(|e| (e.title.as_str(), e.level)).collect();
        assert_eq!(t, [("Introduction", 1), ("Background", 2), ("Methods", 1), ("Sampling details", 2)]);
        assert_eq!(idx[2].page, 2);
        assert_eq!(idx[0].y, 50.0);
    }

    #[test]
    fn numbering_depth_overrides_level() {
        let lines = vec![
            line(1, 10.0, "1 Introduction", 16.0, true),
            line(1, 100.0, "1.1 Motivation", 12.0, true),
            line(1, 200.0, "1.1.1 Details here", 11.0, true),
        ];
        let lv: Vec<u8> = build_index(&lines, 10.0, 5, &[], None).iter().map(|e| e.level).collect();
        assert_eq!(lv, [1, 2, 3]);
    }

    #[test]
    fn level_never_skips() {
        let lines = vec![line(1, 10.0, "1.2.3 Deep first", 16.0, true)];
        assert_eq!(build_index(&lines, 10.0, 5, &[], None)[0].level, 1);
    }

    #[test]
    fn running_headers_and_page_numbers_rejected() {
        let mut lines = Vec::new();
        for p in 1..=20 {
            lines.push(line(p, 5.0, "Introduction to Topology", 14.0, true)); // running header
            lines.push(line(p, 780.0, &format!("{p}"), 14.0, false)); // page number
        }
        lines.push(line(3, 100.0, "Real Heading", 16.0, true));
        let idx = build_index(&lines, 10.0, 20, &[], None);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].title, "Real Heading");
    }

    #[test]
    fn sentences_and_equations_rejected() {
        let lines = vec![
            line(1, 10.0, "This is a large sentence that ends here.", 16.0, false),
            line(1, 50.0, "x = y + 2z", 16.0, false),
            line(1, 90.0, "1 2 3 4 5", 16.0, false),
            line(1, 130.0, "Proof, by induction,", 16.0, false),
            line(1, 170.0, "x<y<z meanr", 16.0, false),
            line(1, 210.0, "of", 16.0, false),
            line(1, 250.0, "1) if either", 16.0, false),
            line(1, 290.0, "is an equivalence relation", 16.0, false),
            line(1, 330.0, "EXAMPLE 1.", 16.0, false),
            line(1, 370.0, "Definition.", 16.0, false),
            line(1, 410.0, "AU B", 16.0, false),
            line(1, 450.0, "AlUAU-..", 16.0, false),
            line(1, 490.0, "If x is odd, then it is not even", 16.0, false),
            line(1, 530.0, "1. Let", 16.0, false),
        ];
        assert!(build_index(&lines, 10.0, 5, &[], None).is_empty());
    }

    #[test]
    fn multi_line_titles_merge() {
        let mut a = line(1, 100.0, "A Very Long Chapter Title That", 20.0, true);
        a.gap_below = 2.0;
        let mut b = line(1, 124.0, "Wraps Onto Two Lines", 20.0, true);
        b.gap_above = 2.0;
        let c = line(1, 300.0, "Another Heading", 20.0, true);
        let idx = build_index(&[a, b, c], 10.0, 5, &[], None);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].title, "A Very Long Chapter Title That Wraps Onto Two Lines");
    }

    #[test]
    fn same_row_fragments_merge_and_section_sign() {
        let a = line(1, 100.0, "Relations", 12.0, false);
        let b = line(1, 100.0, "Orders", 12.5, false);
        let idx = build_index(&[a, b], 10.0, 5, &[], None);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].title, "Relations Orders");
        assert_eq!(numbering("§3 Relations").unwrap(), (1, "Relations"));
    }

    #[test]
    fn keywords_are_headings() {
        let lines = vec![line(1, 100.0, "Chapter 3", 10.0, false), line(2, 50.0, "Appendix A", 10.0, false)];
        let idx = build_index(&lines, 10.0, 5, &[], None);
        assert_eq!(idx.len(), 2);
        assert!(idx.iter().all(|e| e.level == 1));
    }

    #[test]
    fn chapter_label_merges_with_title_line() {
        let lines = vec![line(5, 100.0, "Chapter 2", 20.0, true), line(5, 135.0, "Topological Spaces", 20.0, true), line(7, 90.0, "Part II", 20.0, false)];
        let idx = build_index(&lines, 10.0, 10, &[], None);
        assert_eq!(idx[0].title, "Chapter 2 Topological Spaces");
        assert_eq!(idx[0].y, 100.0);
        assert_eq!(idx.len(), 2, "a bare marker with no title below stays alone");
    }

    #[test]
    fn exercises_and_run_ins_rejected() {
        for t in ["3. (a) Show that A is open", "12. Show that every compact set is closed", "5. Theorem.", "7. Lemma.", "4. Corollary.", "Let X be a set", "Prove the following"] {
            assert!(build_index(&[line(1, 100.0, t, 16.0, true)], 10.0, 5, &[], None).is_empty(), "{t}");
        }
        assert_eq!(build_index(&[line(1, 100.0, "3. Compact Spaces", 16.0, true)], 10.0, 5, &[], None).len(), 1);
    }

    #[test]
    fn back_index_pages_detected_and_skipped() {
        assert!(index_line("Ordered square, 13, 80"));
        assert!(index_line("13 (see Ordered square)"));
        assert!(index_line("Basis, 78-82"));
        assert!(!index_line("Chapter 3 Connectedness 148"));
        assert!(!index_line("Section 4, Theorem 7"));
        let mut lines: Vec<Line> = (0..10).map(|i| line(9, 20.0 + 12.0 * i as f32, &format!("Term {}, {}, {}", (b'a' + i as u8) as char, i + 3, i + 40), 10.0, false)).collect();
        lines.extend((0..10).map(|i| body_line(2, 20.0 + 12.0 * i as f32, &format!("an ordinary sentence {i}"))));
        let skip = index_pages(&lines, 10);
        assert!(skip[9] && !skip[2]);
        lines.push(line(9, 300.0, "Zebra Heading", 18.0, true));
        assert!(build_index(&lines, 10.0, 10, &skip, None).is_empty());
    }

    #[test]
    fn front_matter_dropped_but_preface_kept() {
        let lines = vec![
            line(0, 50.0, "Topology Second Edition", 24.0, true),
            line(2, 50.0, "Preface", 18.0, true),
            line(6, 50.0, "Chapter 1 Set Theory", 18.0, true),
        ];
        let idx = build_index(&lines, 10.0, 10, &[], None);
        assert_eq!(idx.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Preface", "Chapter 1 Set Theory"]);
        let idx = build_index(&lines, 10.0, 10, &[], Some(1));
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn duplicates_nearby_collapse() {
        let lines = vec![line(3, 50.0, "Basis for a Topology", 18.0, true), line(4, 50.0, "Basis for a Topology", 18.0, true), line(40, 50.0, "Basis for a Topology", 18.0, true)];
        assert_eq!(build_index(&lines, 10.0, 50, &[], None).len(), 2);
    }

    #[test]
    fn relevel_nests_by_natural_level() {
        let e = |l: u8| IndexEntry { title: "t".into(), page: 1, y: 0.0, level: l };
        let lv: Vec<u8> = relevel(vec![e(3), e(3), e(3), e(1), e(3), e(2)]).iter().map(|e| e.level).collect();
        assert_eq!(lv, [1, 1, 1, 1, 2, 2]);
    }

    #[test]
    fn lone_title_page_dropped_without_contents() {
        let mut lines = vec![line(0, 50.0, "CH3143 Self Assembly", 24.0, true)];
        for p in 1..=6 {
            lines.push(line(p, 59.0, &format!("Slide Title Number{}", (b'a' + p as u8) as char), 20.0, true));
        }
        let idx = build_index(&lines, 10.0, 8, &[], None);
        assert_eq!(idx.len(), 6);
        assert!(idx.iter().all(|e| e.level == 1 && e.page >= 1));
    }
}
