//! The book's own table of contents: find Contents pages, parse `title ... N` lines, map printed
//! page numbers to PDF pages and locate each title on its page.

use crate::cache::IndexEntry;
use crate::heading::{numbering, Line};
use crate::pages::{roman_value, PageMap};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Printed {
    Arabic(u32),
    Roman(u32),
}

/// One parsed contents line.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub title: String,
    pub printed: Option<Printed>,
    pub x0: f32,
}

/// Indentation step (points) that changes the level of an unmarked entry.
const INDENT: f32 = 15.0;
const LEADER: &str = ".·•…_";
const FILLER: &str = " ,";

/// Dot leaders (`. . . .`, `.....`, `…`) become a single ` … `.
pub fn squash_leaders(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if LEADER.contains(chars[i]) {
            let (mut j, mut n) = (i, 0);
            while j < chars.len() && (LEADER.contains(chars[j]) || FILLER.contains(chars[j])) {
                n += LEADER.contains(chars[j]) as usize;
                j += 1;
            }
            if n >= 3 {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str("… ");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out.trim().to_string()
}

/// Digits with OCR confusions (`l`, `I`, `|` for 1; `O`, `o` for 0).
fn ocr_number(t: &str, leader: bool) -> Option<u32> {
    if t.is_empty() || t.len() > 4 {
        return None;
    }
    let mut v = 0u32;
    let mut real = 0;
    for c in t.chars() {
        let d = match c {
            '0'..='9' => {
                real += 1;
                c as u32 - '0' as u32
            }
            'l' | 'I' | '|' => 1,
            'O' | 'o' => 0,
            _ => return None,
        };
        v = v * 10 + d;
    }
    ((real > 0 || (leader && t.len() <= 2)) && v > 0).then_some(v)
}

fn bare_keyword(t: &str) -> bool {
    let last = t.split_whitespace().last().unwrap_or("").to_lowercase();
    matches!(last.as_str(), "chapter" | "part" | "section" | "appendix" | "lecture" | "unit" | "§" | "page" | "pp." | "p.")
}

/// Drop OCR debris in front of a title (`W N Functions`, `& The Integers`) and split `Part1`.
pub fn clean_title(t: &str) -> String {
    let t = t.trim().trim_start_matches('*').trim_start();
    let mut toks: Vec<String> = t.split_whitespace().map(str::to_string).collect();
    let junk = |w: &str| {
        w.chars().count() <= 2
            && w != "A"
            && w != "§"
            && !w.chars().any(|c| c.is_ascii_digit())
            && roman_value(w).is_none()
            && w.chars().all(|c| !c.is_alphabetic() || c.is_uppercase())
    };
    while toks.len() > 1 && junk(&toks[0]) {
        toks.remove(0);
    }
    if let Some(f) = toks.first_mut() {
        let letters = f.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (word, num) = f.split_at(letters);
        if !num.is_empty() && matches!(word.to_lowercase().as_str(), "chapter" | "part" | "section" | "appendix" | "lecture" | "unit") {
            *f = format!("{word} {num}");
        }
    }
    toks.join(" ")
}

/// `Title ... 75` -> (title, page). Without a leader the title needs two words.
pub fn parse_line(text: &str) -> Option<(String, Printed)> {
    let t = squash_leaders(text);
    let leader = t.contains('…');
    let (head, tail) = t.rsplit_once(|c: char| c == ' ' || c == '…')?;
    let tail = tail.trim();
    // anything between the leader and the number is OCR debris
    let title = head.split('…').next().unwrap_or("").trim().trim_end_matches(|c: char| c == ' ' || c == '.' || c == ',').trim();
    if title.chars().filter(|c| c.is_alphabetic()).count() < 3 || bare_keyword(title) {
        return None;
    }
    let real_word = title.split(|c: char| !c.is_alphabetic()).any(|w| w.chars().count() >= 4);
    if !leader && (title.split_whitespace().count() < 2 || tail.len() > 3 || !real_word) {
        return None;
    }
    let printed = if let Some(n) = ocr_number(tail, leader) {
        Printed::Arabic(n)
    } else if (leader || title.split_whitespace().count() >= 2) && tail.len() <= 6 && !tail.chars().any(|c| c.is_ascii_digit()) {
        // roman front matter: lowercase, or any case after a leader
        if !(leader || tail.chars().all(|c| c.is_lowercase())) {
            return None;
        }
        Printed::Roman(roman_value(tail)?)
    } else {
        return None;
    };
    Some((clean_title(title), printed))
}

/// Hierarchy rank from the title's own marker: Part 0 > Chapter 1 > Section/§ 2 > 1.2.3 ...
/// The flag is true for a plain number (`12 Topological Spaces`), which sits below `Chapter N`
/// when a book uses both.
pub fn rank_kind(title: &str) -> Option<(u8, bool)> {
    let first = title.split_whitespace().next()?.to_lowercase();
    match first.trim_end_matches(|c: char| c.is_ascii_digit()) {
        "part" | "book" => return Some((0, false)),
        // back matter sits at the top level whatever precedes it
        "bibliography" | "index" | "references" | "glossary" | "notation" | "solutions" | "answers" | "afterword" | "epilogue" | "appendices" => {
            return Some((0, false))
        }
        "chapter" | "appendix" | "lecture" | "unit" => return Some((1, false)),
        "section" => return Some((2, false)),
        _ => {}
    }
    if title.starts_with('§') {
        return Some((2, false));
    }
    numbering(title).map(|n| (n.0 as u8, true))
}

pub fn rank(title: &str) -> Option<u8> {
    rank_kind(title).map(|r| r.0)
}

/// A contents line whose page number the OCR lost: `43 Complete Metric Spaces . . .`.
pub fn leader_title(text: &str) -> Option<String> {
    let t = squash_leaders(text);
    let (head, _) = t.split_once('…')?;
    let title = head.trim().trim_start_matches('*').trim();
    (title.chars().filter(|c| c.is_alphabetic()).count() >= 3 && title.split_whitespace().count() <= 14).then(|| clean_title(title))
}

/// Merge fragments sharing a baseline into one line (OCR text arrives word by word).
pub fn rows(lines: &[&Line]) -> Vec<Line> {
    let mut sorted: Vec<&Line> = lines.to_vec();
    sorted.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    let mut out: Vec<Line> = Vec::new();
    let mut group: Vec<&Line> = Vec::new();
    let flush = |group: &mut Vec<&Line>, out: &mut Vec<Line>| {
        if group.is_empty() {
            return;
        }
        group.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let text = squash_leaders(&group.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" "));
        let mut row = group[0].clone();
        row.text = text;
        row.x0 = group[0].x0;
        row.size = group.iter().map(|l| l.size).fold(0.0, f32::max);
        row.y1 = group.iter().map(|l| l.y1).fold(row.y1, f32::max);
        out.push(row);
        group.clear();
    };
    for l in sorted {
        if group.first().is_some_and(|f| (l.y0 - f.y0).abs() > 2.0) {
            flush(&mut group, &mut out);
        }
        group.push(l);
    }
    flush(&mut group, &mut out);
    out
}

fn is_contents_heading(t: &str) -> bool {
    // a running page number may sit next to the word ("viii Contents")
    let words: Vec<&str> = t.split_whitespace().filter(|w| !(w.chars().all(|c| c.is_ascii_digit()) || (w.len() <= 5 && roman_value(w).is_some()))).collect();
    let n: String = words.join(" ").chars().filter(|c| c.is_alphabetic() || *c == ' ').collect::<String>().to_lowercase();
    matches!(n.trim(), "contents" | "table of contents" | "detailed contents" | "brief contents" | "contents in brief" | "contents overview" | "short contents" | "full contents")
}

fn connective_end(s: &str) -> bool {
    let w = s.split_whitespace().last().unwrap_or("").to_lowercase();
    s.ends_with(',') || s.ends_with('-') || matches!(w.as_str(), "and" | "of" | "the" | "to" | "in" | "for" | "a" | "an" | "on" | "with" | "or")
}

fn page_entries(lines: &[&Line]) -> Vec<Raw> {
    let mut out: Vec<Raw> = Vec::new();
    let mut marker: Option<&Line> = None;
    let mut wrap: Option<&Line> = None;
    for l in lines {
        if is_contents_heading(&l.text) {
            continue;
        }
        if let Some((title, printed)) = parse_line(&l.text) {
            let (mut title, mut x0) = (title, l.x0);
            if let Some(m) = marker.take() {
                title = format!("{} {}", m.text.trim(), title);
                x0 = m.x0;
            } else if let Some(w) = wrap.take() {
                if title.chars().next().is_some_and(char::is_lowercase) || connective_end(&w.text) {
                    title = format!("{} {}", w.text.trim(), title);
                    x0 = w.x0;
                }
            }
            out.push(Raw { title, printed: Some(printed), x0 });
            wrap = None;
            continue;
        }
        if let Some(title) = leader_title(&l.text) {
            let mut title = title;
            let mut x0 = l.x0;
            if let Some(m) = marker.take() {
                title = format!("{} {}", m.text.trim(), title);
                x0 = m.x0;
            }
            out.push(Raw { title, printed: None, x0 });
            wrap = None;
            continue;
        }
        if crate::heading::bare_marker(&l.text) {
            marker = Some(l);
            wrap = None;
        } else if rank(&l.text) == Some(0) {
            // "Part II  General Topology": no page number of its own
            out.push(Raw { title: clean_title(&l.text), printed: None, x0: l.x0 });
            marker = None;
            wrap = None;
        } else if l.text.chars().any(char::is_alphabetic) && l.text.split_whitespace().count() <= 10 {
            wrap = Some(l);
            marker = None;
        }
    }
    out
}

pub struct Detected {
    pub pages: Vec<u32>,
    pub raw: Vec<Raw>,
}

/// Find the Contents pages in the front of the document and parse their entries.
pub fn detect(lines: &[Line], pages: usize) -> Detected {
    let limit = pages.min(30.max(pages / 4));
    let mut raw_pages: Vec<Vec<&Line>> = vec![Vec::new(); limit];
    for l in lines.iter().filter(|l| (l.page as usize) < limit) {
        raw_pages[l.page as usize].push(l);
    }
    let rows_by_page: Vec<Vec<Line>> = raw_pages.iter().map(|p| rows(p)).collect();
    let by_page: Vec<Vec<&Line>> = rows_by_page.iter().map(|r| r.iter().collect()).collect();
    let pages = limit;
    let parsed = |p: usize| by_page[p].iter().filter(|l| parse_line(&l.text).is_some() || leader_title(&l.text).is_some()).count();
    let mut det = Detected { pages: Vec::new(), raw: Vec::new() };
    let mut p = 0;
    while p < limit {
        let heading = by_page[p].iter().any(|l| is_contents_heading(&l.text) && l.y0 < 400.0);
        if !(heading && parsed(p) >= 3) {
            p += 1;
            continue;
        }
        let mut q = p;
        loop {
            det.pages.push(q as u32);
            det.raw.extend(page_entries(&by_page[q]));
            q += 1;
            let next_ok = q < pages
                && by_page[q].len() >= 5
                && parsed(q) * 2 >= by_page[q].len()
                && !by_page[q].iter().any(|l| l.text.to_lowercase().starts_with("list of "));
            if !next_ok {
                break;
            }
        }
        p = q;
    }
    det
}

fn words_of(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// Title without its Chapter/Part/§/number marker.
fn core_words(title: &str) -> Vec<String> {
    let mut t = title.trim();
    if let Some(r) = numbering(t) {
        t = r.1;
    } else if let Some((k, rest)) = t.split_once(' ') {
        if rank(t).is_some() && !t.starts_with('§') && matches!(k.to_lowercase().as_str(), "chapter" | "part" | "section" | "appendix" | "lecture" | "unit" | "book") {
            t = rest.split_once(' ').map_or("", |x| x.1);
        }
    }
    if t.starts_with('§') {
        t = t.split_once(' ').map_or("", |x| x.1);
    }
    words_of(t)
}

fn compact(words: &[String]) -> Vec<char> {
    words.concat().chars().collect()
}

fn bigrams(c: &[char]) -> Vec<(char, char)> {
    c.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Dice similarity of character bigrams (robust to OCR run-together words and typos), plus the
/// share of `a`'s bigrams found in `b`.
fn similarity(a: &[char], b: &[char]) -> (f32, f32) {
    if a.len() < 2 || b.len() < 2 {
        return ((a == b && !a.is_empty()) as u8 as f32, 0.0);
    }
    let (ba, mut bb) = (bigrams(a), bigrams(b));
    let mut common = 0;
    for g in &ba {
        if let Some(i) = bb.iter().position(|x| x == g) {
            bb.swap_remove(i);
            common += 1;
        }
    }
    (2.0 * common as f32 / (ba.len() + bigrams(b).len()) as f32, common as f32 / ba.len() as f32)
}

/// Best matching row of `page` for the title's core text: (index, score).
fn match_page(core: &[char], page: &[Line]) -> Option<(usize, f32)> {
    if core.len() < 3 {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for (i, l) in page.iter().enumerate() {
        let lw = compact(&core_words(&l.text));
        if lw.len() < 3 {
            continue;
        }
        let (dice, _) = similarity(core, &lw);
        // a title broken over lines: this row is (nearly) all inside the title
        let (_, inside) = similarity(&lw, core);
        let wrapped = lw.len() >= 6 && inside >= 0.9 && lw.len() * 3 >= core.len();
        let score = if dice >= 0.72 { dice } else if wrapped { inside * 0.8 } else { continue };
        let score = score + l.size / 1000.0;
        if best.is_none_or(|b| score > b.1) {
            best = Some((i, score));
        }
    }
    best
}

pub struct Resolved {
    pub entries: Vec<IndexEntry>,
    pub located: usize,
}

/// Map printed numbers to PDF pages, refine y by finding each title, build levels.
pub fn resolve(det: &Detected, lines: &[Line], pages: usize, map: &PageMap) -> Resolved {
    let mut raw_pages: Vec<Vec<&Line>> = vec![Vec::new(); pages];
    for l in lines.iter().filter(|l| (l.page as usize) < pages) {
        raw_pages[l.page as usize].push(l);
    }
    let mut row_cache: Vec<Option<Vec<Line>>> = vec![None; pages];
    // printed numbers -> PDF pages (entries without one are searched for below)
    let pdf: Vec<Option<u32>> = det
        .raw
        .iter()
        .map(|r| match r.printed {
            Some(Printed::Arabic(n)) => map.printed_to_pdf(n, false),
            Some(Printed::Roman(n)) => map.printed_to_pdf(n, true),
            None => None,
        })
        .map(|p| p.filter(|&p| (p as usize) < pages))
        .collect();
    // the next entry that has a usable page number bounds the search for those that lost theirs
    let mut next_known: Vec<Option<u32>> = vec![None; det.raw.len()];
    let mut nk = None;
    for i in (0..det.raw.len()).rev() {
        next_known[i] = nk;
        if pdf[i].is_some() {
            nk = pdf[i];
        }
    }
    // with both `Chapter N` and plain `12 Title` entries, the plain numbers are the sections
    let has_chapters = det.raw.iter().any(|r| rank_kind(&r.title).is_some_and(|k| k.0 == 1 && !k.1));

    struct Item {
        title: String,
        page: u32,
        y: f32,
        rank: Option<u8>,
        x0: f32,
        order: usize,
    }
    // y of the title on a page, including the "Chapter N" / "Part N" label above it
    let find_on = |pg: &[Line], core: &[char], title: &str, strict: bool| -> Option<f32> {
        let (li, score) = match_page(core, pg)?;
        if strict && score < 0.8 {
            return None;
        }
        let mut y = pg[li].y0;
        let first = title.split_whitespace().next().unwrap_or("").to_lowercase();
        for l in pg.iter().filter(|l| l.y1 <= pg[li].y0 + 1.0 && pg[li].y0 - l.y1 < 70.0) {
            if l.text.split_whitespace().next().is_some_and(|w| w.to_lowercase() == first) {
                y = y.min(l.y0);
            }
        }
        Some(y)
    };
    let mut items: Vec<Item> = Vec::new();
    let mut located = 0;
    let mut cursor = det.pages.iter().max().map_or(0, |&p| p + 1);
    for (i, r) in det.raw.iter().enumerate() {
        let core = compact(&core_words(&r.title));
        let mut hit: Option<(u32, f32)> = None;
        let mut page_only = None;
        if r.printed.is_some() {
            let Some(p) = pdf[i] else { continue };
            let key: String = words_of(&r.title).join("");
            if items.iter().any(|x| words_of(&x.title).join("") == key && x.page.abs_diff(p) <= 1) {
                continue; // brief and detailed contents list the same chapter
            }
            for cand in [p as i64, p as i64 + 1, p as i64 - 1] {
                if cand < 0 || cand as usize >= pages {
                    continue;
                }
                let c = cand as usize;
                let pg = row_cache[c].get_or_insert_with(|| rows(&raw_pages[c]));
                if let Some(y) = find_on(pg, &core, &r.title, false) {
                    hit = Some((cand as u32, y));
                    break;
                }
            }
            page_only = Some(p);
        } else {
            // no number (lost to OCR, or a Part title): look for the title between its neighbours
            let hi = next_known[i].map_or((cursor as usize + 150).min(pages - 1), |h| h as usize).min(pages - 1);
            for c in (cursor as usize)..=hi {
                let pg = row_cache[c].get_or_insert_with(|| rows(&raw_pages[c]));
                if let Some(y) = find_on(pg, &core, &r.title, true) {
                    hit = Some((c as u32, y));
                    break;
                }
            }
            if hit.is_none() && rank(&r.title) == Some(0) {
                page_only = next_known[i];
            }
        }
        let (page, y) = match (hit, page_only) {
            (Some(h), _) => {
                located += 1;
                h
            }
            (None, Some(p)) => (p, 0.0),
            (None, None) => continue,
        };
        let key: String = words_of(&r.title).join("");
        if r.printed.is_none() && items.iter().any(|x| words_of(&x.title).join("") == key && x.page.abs_diff(page) <= 1) {
            continue;
        }
        cursor = cursor.max(page);
        let rank = rank_kind(&r.title).map(|(rk, numeric)| if numeric && has_chapters { rk + 1 } else { rk });
        items.push(Item { title: r.title.clone(), page, y, rank, x0: r.x0, order: i });
    }
    items.sort_by(|a, b| a.page.cmp(&b.page).then(a.rank.unwrap_or(1).cmp(&b.rank.unwrap_or(1)).then(a.order.cmp(&b.order))));

    // natural levels from the markers (relevel() nests them); unmarked entries go by indentation
    let mut prev = 0u8;
    let mut prev_x = 0.0f32;
    let entries: Vec<IndexEntry> = items
        .into_iter()
        .map(|it| {
            let lvl = match it.rank {
                Some(r) => r + 1,
                // unmarked (`Preface`, `Supplementary Exercises`): level of the previous entry,
                // one up or down when clearly outdented / indented
                None if prev == 0 => 2, // as a chapter: the first entry has nothing to compare with
                None if it.x0 < prev_x - INDENT => prev.saturating_sub(1).max(1),
                None if it.x0 > prev_x + INDENT => prev + 1,
                None => prev,
            };
            let level = lvl.max(1);
            prev = level;
            prev_x = it.x0;
            IndexEntry { title: it.title, page: it.page, y: it.y, level }
        })
        .collect();
    Resolved { entries: crate::heading::relevel(entries), located }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::Segment;

    #[test]
    fn parses_dot_leaders() {
        assert_eq!(parse_line("Topological Spaces ........ 75"), Some(("Topological Spaces".into(), Printed::Arabic(75))));
        assert_eq!(parse_line("§13 Basis for a Topology . . . . . . 78"), Some(("§13 Basis for a Topology".into(), Printed::Arabic(78))));
        assert_eq!(parse_line("Preface …… xi"), Some(("Preface".into(), Printed::Roman(11))));
        assert_eq!(parse_line("Introduction to Rings 12"), Some(("Introduction to Rings".into(), Printed::Arabic(12))));
    }

    #[test]
    fn parses_ocr_noise() {
        assert_eq!(parse_line("Set Theory and Logic ..... l"), Some(("Set Theory and Logic".into(), Printed::Arabic(1))));
        assert_eq!(parse_line("Fundamental Concepts . . . . 4l"), Some(("Fundamental Concepts".into(), Printed::Arabic(41))));
        assert_eq!(parse_line("Compactness ,.,.,.,. 1O8"), Some(("Compactness".into(), Printed::Arabic(108))));
    }

    #[test]
    fn title_cleanup() {
        assert_eq!(clean_title("W N Functions"), "Functions");
        assert_eq!(clean_title("& The Integers and the Real Numbers"), "The Integers and the Real Numbers");
        assert_eq!(clean_title("A Note to the Reader"), "A Note to the Reader");
        assert_eq!(clean_title("*44 A Space-Filling Curve"), "44 A Space-Filling Curve");
        assert_eq!(clean_title("Part1 GENERAL TOPOLOGY"), "Part 1 GENERAL TOPOLOGY");
        assert_eq!(clean_title("II Basics"), "II Basics");
    }

    #[test]
    fn rejects_non_entries() {
        assert_eq!(parse_line("Chapter 3"), None);
        assert_eq!(parse_line("Contents"), None);
        assert_eq!(parse_line("a sentence with no number at end"), None);
        assert_eq!(parse_line("Let x be 7"), None);
        assert_eq!(parse_line("12"), None);
    }

    #[test]
    fn ranks() {
        assert_eq!(rank("Part II General"), Some(0));
        assert_eq!(rank("Chapter 4 Foo"), Some(1));
        assert_eq!(rank("§13 Basis"), Some(2));
        assert_eq!(rank("2.3 Foo bar"), Some(2));
        assert_eq!(rank("12 Topological Spaces"), Some(1));
        assert_eq!(rank("Preface"), None);
    }

    fn l(page: u32, y0: f32, x0: f32, text: &str) -> Line {
        Line { page, x0, text: text.into(), size: 10.0, bold: false, y0, y1: y0 + 10.0, gap_above: 5.0, gap_below: 5.0 }
    }

    fn book() -> (Vec<Line>, PageMap) {
        let mut v = vec![l(1, 40.0, 100.0, "Contents")];
        let rows = [
            "Preface ........ vii", "Chapter 1", "Set Theory and Logic ..... 1", "§1 Fundamental Concepts ....... 4",
            "§2 Functions ....... 10", "Chapter 2 Topological Spaces ...... 20", "§3 Basis for a Topology ...... 22",
        ];
        for (i, r) in rows.iter().enumerate() {
            let x = if r.starts_with('§') { 120.0 } else { 100.0 };
            v.push(l(1, 80.0 + 14.0 * i as f32, x, r));
        }
        // body pages (offset 5: printed 1 is PDF page index 5)
        v.push(l(5, 60.0, 90.0, "Chapter 1"));
        v.push(l(5, 80.0, 90.0, "Set Theory and Logic"));
        v.push(l(5, 200.0, 90.0, "1 Fundamental Concepts"));
        v.push(l(8, 100.0, 90.0, "§1 Fundamental Concepts"));
        v.push(l(14, 100.0, 90.0, "§2 Functions"));
        v.push(l(24, 50.0, 90.0, "Chapter 2"));
        v.push(l(24, 75.0, 90.0, "Topological Spaces"));
        v.push(l(26, 300.0, 90.0, "§3 Basis for a Topology"));
        v.push(l(3, 100.0, 90.0, "Preface"));
        let map = PageMap {
            segments: vec![
                Segment { start: 2, end: 4, offset: -4, roman: true, support: 2 },
                Segment { start: 5, end: 30, offset: 5, roman: false, support: 20 },
            ],
        };
        (v, map)
    }

    #[test]
    fn detects_contents_and_resolves() {
        let (lines, map) = book();
        let det = detect(&lines, 40);
        assert_eq!(det.pages, vec![1]);
        let titles: Vec<&str> = det.raw.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles[1], "Chapter 1 Set Theory and Logic", "bare marker joins the next line");
        let r = resolve(&det, &lines, 40, &map);
        assert_eq!(r.entries.len(), 6);
        assert_eq!(r.located, 6);
        let e = &r.entries;
        assert_eq!(e[0].title, "Preface");
        assert_eq!((e[1].page, e[1].y), (5, 60.0), "label line above the title");
        assert_eq!((e[2].page, e[2].level), (8, 2));
        assert_eq!((e[4].page, e[4].y), (24, 50.0));
        assert_eq!(e[1].level, 1);
        assert_eq!(e[5].level, 2);
        assert_eq!(e[5].y, 300.0);
    }

    #[test]
    fn unlocated_titles_fall_back_to_page_top() {
        let (mut lines, map) = book();
        lines.retain(|x| x.page != 14);
        let det = detect(&lines, 40);
        let r = resolve(&det, &lines, 40, &map);
        let f = r.entries.iter().find(|e| e.title.contains("Functions")).unwrap();
        assert_eq!((f.page, f.y), (14, 0.0));
        assert_eq!(r.located, 5);
    }

    #[test]
    fn numberless_entries_are_found_between_their_neighbours() {
        let (mut lines, map) = book();
        // OCR lost the page number of "§2 Functions ...": the leader is all that is left
        for x in lines.iter_mut().filter(|x| x.text.starts_with("§2 Functions")) {
            if x.page == 1 {
                x.text = "§2 Functions . . . . . . .".into();
            }
        }
        let det = detect(&lines, 40);
        assert!(det.raw.iter().any(|r| r.printed.is_none() && r.title == "§2 Functions"));
        let r = resolve(&det, &lines, 40, &map);
        let f = r.entries.iter().find(|e| e.title.contains("Functions")).unwrap();
        assert_eq!((f.page, f.y), (14, 100.0));
        assert_eq!(r.entries.len(), 6);
    }

    #[test]
    fn continuation_pages_and_wrapped_titles() {
        let mut lines = vec![l(0, 40.0, 50.0, "Table of Contents")];
        lines.push(l(0, 70.0, 50.0, "Connectedness and"));
        lines.push(l(0, 82.0, 50.0, "compactness ........ 30"));
        for i in 0..4 {
            lines.push(l(0, 100.0 + 12.0 * i as f32, 50.0, &format!("Topic number {i} ...... {}", 31 + i)));
        }
        for i in 0..6 {
            lines.push(l(1, 30.0 + 12.0 * i as f32, 50.0, &format!("Another topic {i} ...... {}", 40 + i)));
        }
        lines.push(l(2, 30.0, 50.0, "An ordinary body line"));
        let det = detect(&lines, 40);
        assert_eq!(det.pages, vec![0, 1]);
        assert_eq!(det.raw[0].title, "Connectedness and compactness");
        assert_eq!(det.raw.len(), 11);
    }
}
