//! Printed page numbers: detect `pdf_page - printed_page` offsets (arabic body, roman front matter).

use serde::{Deserialize, Serialize};

/// A page-number candidate read from the first/last lines of a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cand {
    Arabic(u32),
    Roman(u32),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Segment {
    /// First PDF page (0-based) of the run and one past its last.
    pub start: u32,
    pub end: u32,
    /// `pdf_page_1based - printed_page`.
    pub offset: i32,
    pub roman: bool,
    /// Number of pages whose number voted for this offset.
    pub support: u32,
}

/// Mapping between PDF pages and printed page numbers, usable by a printed-page `:goto`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PageMap {
    pub segments: Vec<Segment>,
}

pub fn roman_value(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 8 {
        return None;
    }
    let (lower, upper) = (s.chars().all(|c| c.is_lowercase()), s.chars().all(|c| c.is_uppercase()));
    if !(lower || upper) {
        return None;
    }
    let mut total = 0i32;
    let mut prev = 0i32;
    for c in s.chars().rev() {
        let v = match c.to_ascii_lowercase() {
            'i' => 1,
            'v' => 5,
            'x' => 10,
            'l' => 50,
            'c' => 100,
            'd' => 500,
            'm' => 1000,
            _ => return None,
        };
        if v < prev { total -= v } else { total += v }
        prev = prev.max(v);
    }
    (total > 0).then_some(total as u32)
}

#[allow(dead_code)] // used by the printed-page :goto (phase 6)
pub fn to_roman(mut n: u32) -> String {
    let t = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in t {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

const NOT_PAGE_WORDS: [&str; 10] = ["chapter", "part", "section", "figure", "fig.", "table", "appendix", "lecture", "slide", "page"];

/// Page-number candidates from a few lines (first/last of the page).
pub fn candidates(lines: &[&str]) -> Vec<Cand> {
    let mut out = Vec::new();
    for l in lines {
        let words: Vec<&str> = l.split_whitespace().collect();
        if words.is_empty() || words.len() > 9 {
            continue;
        }
        let parse = |w: &str| -> Option<Cand> {
            if (1..=4).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit()) {
                return w.parse().ok().filter(|&n| n > 0).map(Cand::Arabic);
            }
            roman_value(w).map(Cand::Roman)
        };
        if words.len() == 1 {
            out.extend(parse(words[0]));
            continue;
        }
        // "25 Relations" / "Relations 25": the number sits at an end, not after a label word
        if let Some(c) = parse(words[0]) {
            out.push(c);
        }
        let last = words[words.len() - 1];
        let before = words[words.len() - 2].to_lowercase();
        if !NOT_PAGE_WORDS.iter().any(|k| before.starts_with(k)) {
            if let Some(c @ Cand::Arabic(_)) = parse(last) {
                out.push(c);
            }
        }
    }
    out
}

/// Find offset runs. `per_page[p]` are the candidates of PDF page `p`.
pub fn detect(per_page: &[Vec<Cand>]) -> PageMap {
    use std::collections::BTreeMap;
    let n = per_page.len();
    let min_support = (n / 4).clamp(2, 3) as u32;
    let mut votes: BTreeMap<(bool, i32), Vec<u32>> = BTreeMap::new();
    for (p, cs) in per_page.iter().enumerate() {
        for c in cs {
            let (roman, v) = match *c {
                Cand::Arabic(v) => (false, v),
                Cand::Roman(v) => (true, v),
            };
            let pages = votes.entry((roman, p as i32 + 1 - v as i32)).or_default();
            if pages.last() != Some(&(p as u32)) {
                pages.push(p as u32);
            }
        }
    }
    // a run is a cluster of voting pages; a far-away stray vote must not stretch it
    let mut cands: Vec<Segment> = Vec::new();
    for ((roman, offset), pages) in votes {
        let mut start = 0;
        for i in 1..=pages.len() {
            if i == pages.len() || pages[i] - pages[i - 1] > 40 {
                let run = &pages[start..i];
                if run.len() as u32 >= min_support {
                    cands.push(Segment { start: run[0], end: run[run.len() - 1] + 1, offset, roman, support: run.len() as u32 });
                }
                start = i;
            }
        }
    }
    cands.sort_by_key(|s| std::cmp::Reverse(s.support));
    let mut kept: Vec<Segment> = Vec::new();
    for c in cands {
        let clash = kept.iter().any(|k| k.roman == c.roman && c.start < k.end && k.start < c.end);
        if !clash {
            kept.push(c);
        }
    }
    kept.sort_by_key(|s| s.start);
    PageMap { segments: kept }
}

impl PageMap {
    /// Printed label of PDF page `p` (0-based), e.g. `"25"` or `"xii"`.
    pub fn pdf_to_printed(&self, p: u32) -> Option<String> {
        let s = self.segments.iter().find(|s| p >= s.start && p < s.end)?;
        let v = p as i32 + 1 - s.offset;
        (v > 0).then(|| if s.roman { to_roman(v as u32) } else { v.to_string() })
    }

    /// PDF page (0-based) showing printed page `n`. Pages outside every numbered run (a chapter
    /// opener without a visible number, say) extrapolate from the nearest earlier run.
    pub fn printed_to_pdf(&self, n: u32, roman: bool) -> Option<u32> {
        let segs = || self.segments.iter().filter(|s| s.roman == roman);
        let at = |s: &Segment| u32::try_from(n as i32 + s.offset - 1).ok();
        if let Some(p) = segs().filter_map(|s| at(s).filter(|&p| p >= s.start && p < s.end)).next() {
            return Some(p);
        }
        // beyond a run's last numbered page: use the latest run starting before the target
        segs().filter(|s| at(s).is_some_and(|p| p >= s.start)).max_by_key(|s| s.start).and_then(at)
    }
}

/// What `:goto` was given.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum GotoArg {
    /// `#120`: a PDF page, 1-based.
    Pdf(u32),
    /// `120`: a printed page when a map exists, else a PDF page.
    Number(u32),
    /// `xii`: a roman-numbered (front matter) page.
    Roman(u32),
}

pub fn parse_goto(s: &str) -> Result<GotoArg, String> {
    let s = s.trim();
    if let Some(n) = s.strip_prefix('#') {
        return n.trim().parse().map(GotoArg::Pdf).map_err(|_| format!("goto: bad PDF page: {s}"));
    }
    if let Ok(n) = s.parse::<u32>() {
        return Ok(GotoArg::Number(n));
    }
    roman_value(&s.to_ascii_lowercase()).map(GotoArg::Roman).ok_or_else(|| format!("goto: not a page: {s}"))
}

/// Resolve to a 0-based PDF page and an optional statusbar note (when a fallback was used).
pub fn resolve_goto(map: Option<&PageMap>, arg: GotoArg, printed: bool, total: u32) -> Result<(u32, Option<String>), String> {
    let last = total.saturating_sub(1);
    let pdf = |n: u32| (n.max(1) - 1).min(last);
    let map = map.filter(|m| !m.segments.is_empty());
    match arg {
        GotoArg::Pdf(n) => Ok((pdf(n), None)),
        GotoArg::Number(n) => match (printed, map) {
            (true, Some(m)) => match m.printed_to_pdf(n, false).filter(|&p| p <= last) {
                Some(p) => Ok((p, None)),
                None => Ok((pdf(n), Some(format!("printed page {n} not found; went to PDF page {}", pdf(n) + 1)))),
            },
            (true, None) => Ok((pdf(n), Some("no printed page numbers detected; went to PDF page".into()))),
            (false, _) => Ok((pdf(n), None)),
        },
        GotoArg::Roman(n) => match map.and_then(|m| m.printed_to_pdf(n, true)).filter(|&p| p <= last) {
            Some(p) => Ok((p, None)),
            None => Err(format!("no roman page {} in this document", to_roman(n))),
        },
    }
}

/// Page label for lists: the printed number when mapped, else the PDF page number.
pub fn label(map: Option<&PageMap>, p: u32) -> String {
    map.and_then(|m| m.pdf_to_printed(p)).unwrap_or_else(|| (p + 1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Vec<Vec<Cand>> {
        // 6 roman front-matter pages (i..vi), then arabic 1.. starting at PDF page 7 (index 6)
        let mut v: Vec<Vec<Cand>> = (1..=6).map(|r| vec![Cand::Roman(r)]).collect();
        v.extend((1..=20).map(|a| if a % 5 == 0 { vec![] } else { vec![Cand::Arabic(a)] }));
        v
    }

    #[test]
    fn offsets_with_roman_front_matter() {
        let m = detect(&book());
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.pdf_to_printed(0).as_deref(), Some("i"));
        assert_eq!(m.pdf_to_printed(3).as_deref(), Some("iv"));
        assert_eq!(m.pdf_to_printed(6).as_deref(), Some("1"));
        assert_eq!(m.pdf_to_printed(10).as_deref(), Some("5")); // page without a number inside the run
        assert_eq!(m.printed_to_pdf(10, false), Some(15));
        assert_eq!(m.printed_to_pdf(3, true), Some(2));
    }

    #[test]
    fn noise_is_outvoted_and_two_arabic_runs() {
        let mut b: Vec<Vec<Cand>> = (1..=10).map(|a| vec![Cand::Arabic(a)]).collect();
        b[4].push(Cand::Arabic(77)); // stray number
        // second run with another offset (e.g. an inserted insert/plate section)
        b.extend((20..=30).map(|a| vec![Cand::Arabic(a)]));
        let m = detect(&b);
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.segments[0].offset, 0);
        assert_eq!(m.segments[1].offset, 10 + 1 - 20);
        assert_eq!(m.pdf_to_printed(12).as_deref(), Some("22"));
    }

    #[test]
    fn candidate_extraction() {
        assert_eq!(candidates(&["25", "§3 Relations 27"]), vec![Cand::Arabic(25), Cand::Arabic(27)]);
        assert_eq!(candidates(&["xii"]), vec![Cand::Roman(12)]);
        assert_eq!(candidates(&["Chapter 3"]), vec![]);
        assert_eq!(candidates(&["12 Topological Spaces"]), vec![Cand::Arabic(12)]);
        assert!(candidates(&["this is a long sentence with a number 5 inside it ok yes"]).is_empty());
    }

    #[test]
    fn roman_roundtrip() {
        for n in [1, 4, 9, 14, 40, 90, 400, 1994] {
            assert_eq!(roman_value(&to_roman(n)), Some(n));
        }
        assert_eq!(roman_value("Ii"), None);
    }

    #[test]
    fn goto_parsing_and_mapping() {
        assert_eq!(parse_goto("120"), Ok(GotoArg::Number(120)));
        assert_eq!(parse_goto("#120"), Ok(GotoArg::Pdf(120)));
        assert_eq!(parse_goto("xii"), Ok(GotoArg::Roman(12)));
        assert_eq!(parse_goto("XII"), Ok(GotoArg::Roman(12)));
        assert!(parse_goto("zz").is_err());
        assert!(parse_goto("#x").is_err());
        let m = detect(&book()); // arabic page 1 is PDF page 7
        let go = |a, pr| resolve_goto(Some(&m), a, pr, 26);
        assert_eq!(go(GotoArg::Number(3), true), Ok((8, None)));
        assert_eq!(go(GotoArg::Number(3), false), Ok((2, None)), "option off: PDF page");
        assert_eq!(go(GotoArg::Pdf(3), true), Ok((2, None)));
        assert_eq!(go(GotoArg::Roman(4), true), Ok((3, None)));
        let (p, note) = go(GotoArg::Number(500), true).unwrap();
        assert_eq!(p, 25, "beyond the book clamps to the last page");
        assert!(note.is_some());
        assert!(resolve_goto(None, GotoArg::Roman(4), true, 26).is_err());
        let (p, note) = resolve_goto(None, GotoArg::Number(9), true, 26).unwrap();
        assert_eq!((p, note.is_some()), (8, true), "unmapped: PDF page plus a note");
        assert_eq!(resolve_goto(Some(&PageMap::default()), GotoArg::Number(9), true, 26).unwrap().0, 8);
        assert_eq!(label(Some(&m), 6), "1");
        assert_eq!(label(None, 6), "7");
    }
}
