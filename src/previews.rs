//! Preview targets (equations, statements, figures, tables, sections) and the references that
//! point at them. Pure functions over words with positions; the MuPDF side lives in `analyze`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Definition,
    Theorem,
    Lemma,
    Corollary,
    Proposition,
    Example,
    Remark,
    Exercise,
    Axiom,
    Rule,
    Problem,
    Equation,
    Figure,
    Table,
    Scheme,
    Chart,
    Section,
    Chapter,
    /// Destination of an internal PDF link that is not inside any other target.
    Link,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Definition => "Definition",
            Kind::Theorem => "Theorem",
            Kind::Lemma => "Lemma",
            Kind::Corollary => "Corollary",
            Kind::Proposition => "Proposition",
            Kind::Example => "Example",
            Kind::Remark => "Remark",
            Kind::Exercise => "Exercise",
            Kind::Axiom => "Axiom",
            Kind::Rule => "Rule",
            Kind::Problem => "Problem",
            Kind::Equation => "Equation",
            Kind::Figure => "Figure",
            Kind::Table => "Table",
            Kind::Scheme => "Scheme",
            Kind::Chart => "Chart",
            Kind::Section => "Section",
            Kind::Chapter => "Chapter",
            Kind::Link => "Link",
        }
    }

    pub fn is_statement(self) -> bool {
        self <= Kind::Problem
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Target {
    #[serde(rename = "k")]
    pub kind: Kind,
    #[serde(rename = "l")]
    pub label: String,
    #[serde(rename = "p")]
    pub page: u32,
    /// Page points from the page origin: x0, y0, x1, y1.
    #[serde(rename = "r")]
    pub rect: [f32; 4],
    /// First line, for lists and the clipboard.
    #[serde(rename = "t")]
    pub text: String,
}

impl Target {
    pub fn title(&self) -> String {
        if self.label.is_empty() { self.kind.name().to_string() } else { format!("{} {}", self.kind.name(), self.label) }
    }
}

/// A reference before resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct RawRef {
    pub page: u32,
    pub rect: [f32; 4],
    pub kind: Kind,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ref {
    pub rect: [f32; 4],
    pub target: u32,
}

// ---- input -------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct W {
    pub t: String,
    pub x0: f32,
    pub x1: f32,
}

#[derive(Clone, Debug)]
pub struct PLine {
    pub words: Vec<W>,
    pub y0: f32,
    pub y1: f32,
    pub size: f32,
    pub bold: bool,
}

pub struct PageIn {
    pub page: u32,
    pub w: f32,
    pub h: f32,
    pub lines: Vec<PLine>,
    /// Image blocks that do not cover most of the page.
    pub images: Vec<[f32; 4]>,
}

struct Row {
    words: Vec<W>,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    size: f32,
    bold: bool,
    text: String,
}

/// Fragments sharing a baseline (OCR words, equation labels, scripts) become one row.
/// `Table6.1` (kerned small caps drop the space): split a caption keyword from the number glued to it.
fn split_glued(w: &W) -> Vec<W> {
    for kw in ["Figure", "Table", "Scheme", "Chart", "Fig."] {
        if let Some(rest) = w.t.strip_prefix(kw) {
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                let cut = w.x0 + (w.x1 - w.x0) * kw.len() as f32 / w.t.chars().count().max(1) as f32;
                return vec![W { t: kw.to_string(), x1: cut, ..w.clone() }, W { t: rest.to_string(), x0: cut, ..w.clone() }];
            }
        }
    }
    vec![w.clone()]
}

fn rows(lines: &[PLine]) -> Vec<Row> {
    let mut sorted: Vec<&PLine> = lines.iter().filter(|l| !l.words.is_empty()).collect();
    sorted.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut groups: Vec<Vec<&PLine>> = Vec::new();
    for l in sorted {
        // same baseline band, and not a tall drawing fragment swallowing the small text beside it
        let same = groups.last().is_some_and(|g| {
            g.iter().any(|f| {
                let (hf, hl) = ((f.y1 - f.y0).max(0.1), (l.y1 - l.y0).max(0.1));
                let overlap = f.y1.min(l.y1) - f.y0.max(l.y0);
                let dc = ((f.y0 + f.y1) - (l.y0 + l.y1)).abs() / 2.0;
                overlap >= 0.5 * hf.min(hl) && overlap > 0.0 && hf.max(hl) <= 1.8 * hf.min(hl) && dc <= 0.4 * hf.max(hl)
            })
        });
        if same {
            groups.last_mut().unwrap().push(l);
        } else {
            groups.push(vec![l]);
        }
    }
    let mk = |words: Vec<W>, y0: f32, y1: f32, size: f32, bold: bool| Row {
        x0: words[0].x0,
        x1: words.iter().map(|w| w.x1).fold(0.0, f32::max),
        y0,
        y1,
        size,
        bold,
        text: words.iter().map(|w| w.t.as_str()).collect::<Vec<_>>().join(" "),
        words,
    };
    let mut out = Vec::new();
    for g in groups {
        let main = g.iter().max_by_key(|l| l.words.len()).unwrap();
        let mut words: Vec<W> = g.iter().flat_map(|l| l.words.iter().flat_map(split_glued)).collect();
        words.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let y0 = g.iter().map(|l| l.y0).fold(f32::MAX, f32::min);
        let y1 = g.iter().map(|l| l.y1).fold(0.0, f32::max);
        // a caption sharing a baseline with figure labels on its left (`cr(out)   Figure 1.10`) is its own row
        let cut = (1..words.len()).find(|&k| {
            is_caption_kw(&words[k].t) && words.get(k + 1).is_some_and(|n| plain_label(trim_tail(&n.t)).is_some()) && words[k].x0 - words[k - 1].x1 > 3.0 * main.size.max(6.0)
        });
        match cut {
            Some(k) => {
                let right = words.split_off(k);
                out.push(mk(words, y0, y1, main.size, main.bold));
                out.push(mk(right, y0, y1, main.size, main.bold));
            }
            None => out.push(mk(words, y0, y1, main.size, main.bold)),
        }
    }
    out
}

fn is_caption_kw(t: &str) -> bool {
    matches!(trim_tail(t), "Figure" | "Table" | "Scheme" | "Chart")
}

struct Metrics {
    lm: f32,
    rm: f32,
    bw: f32,
    /// Median font size of body-text rows (0 when the page has too few to tell).
    body: f32,
}

fn metrics(rows: &[Row], pw: f32) -> Metrics {
    let mut sz: Vec<f32> = rows.iter().filter(|r| prose_like(r)).map(|r| r.size).collect();
    sz.sort_by(f32::total_cmp);
    let body = if sz.len() >= 3 { sz[sz.len() / 2] } else { 0.0 };
    let mut x0: Vec<f32> = rows.iter().filter(|r| r.words.len() >= 5).map(|r| r.x0).collect();
    let mut x1: Vec<f32> = rows.iter().filter(|r| r.words.len() >= 5).map(|r| r.x1).collect();
    if x0.len() < 4 {
        return Metrics { lm: pw * 0.1, rm: pw * 0.9, bw: pw * 0.8, body };
    }
    x0.sort_by(f32::total_cmp);
    x1.sort_by(f32::total_cmp);
    let lm = x0[x0.len() / 5];
    let rm = x1[x1.len() * 4 / 5];
    Metrics { lm, rm, bw: (rm - lm).max(pw * 0.3), body }
}

// ---- token helpers -----------------------------------------------------------------------

fn trim_tail(w: &str) -> &str {
    w.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':'))
}

/// `13`, `13.1`, `2.3.4`, `3.2a`.
fn plain_label(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut groups = 0;
    loop {
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st || i - st > 3 {
            return None;
        }
        groups += 1;
        if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    if i < b.len() && b[i].is_ascii_lowercase() {
        i += 1;
    }
    (i == b.len() && groups <= 4).then_some(s)
}

/// Appendix style `A.1`.
fn appendix_label(s: &str) -> Option<&str> {
    let mut c = s.chars();
    let (a, dot) = (c.next()?, c.next()?);
    (a.is_ascii_uppercase() && dot == '.' && plain_label(&s[2..]).is_some()).then_some(s)
}

fn any_label(s: &str) -> Option<&str> {
    plain_label(s).or_else(|| appendix_label(s))
}

/// A numbered-equation tag: `(3.2)`, `(3.2a)`, `(12)`, `[3.2]`, or bare dotted `3.2`.
/// Returns (label, bracketed).
pub fn eq_tag(w: &str) -> Option<(String, bool)> {
    let w = trim_tail(w);
    for (o, c) in [('(', ')'), ('[', ']')] {
        if let Some(inner) = w.strip_prefix(o).and_then(|r| r.strip_suffix(c)) {
            return any_label(inner).map(|l| (l.to_string(), true));
        }
    }
    (w.contains('.') && plain_label(w).is_some()).then(|| (w.to_string(), false))
}

fn kind_for_keyword(k: &str) -> Option<Kind> {
    Some(match k {
        "eq" | "eqs" | "eqn" | "eqns" | "equation" | "equations" => Kind::Equation,
        "theorem" | "theorems" | "thm" | "thms" => Kind::Theorem,
        "lemma" | "lemmas" | "lem" | "lemmata" => Kind::Lemma,
        "corollary" | "corollaries" | "cor" => Kind::Corollary,
        "proposition" | "propositions" | "prop" => Kind::Proposition,
        "definition" | "definitions" | "def" | "defn" => Kind::Definition,
        "example" | "examples" => Kind::Example,
        "remark" | "remarks" => Kind::Remark,
        "exercise" | "exercises" => Kind::Exercise,
        "axiom" | "axioms" => Kind::Axiom,
        "rule" | "rules" => Kind::Rule,
        "problem" | "problems" => Kind::Problem,
        "figure" | "figures" | "fig" | "figs" => Kind::Figure,
        "table" | "tables" => Kind::Table,
        "scheme" | "schemes" => Kind::Scheme,
        "chart" | "charts" => Kind::Chart,
        "section" | "sections" | "sec" | "secs" | "§" | "§§" => Kind::Section,
        "chapter" | "chapters" | "ch" => Kind::Chapter,
        _ => return None,
    })
}

/// Words that can introduce a bare `(12)` as an equation reference.
const PREPS: [&str; 10] = ["by", "in", "from", "using", "see", "of", "to", "into", "via", "with"];

fn rect_of(ws: &[&W], y0: f32, y1: f32) -> [f32; 4] {
    let x0 = ws.iter().map(|w| w.x0).fold(f32::MAX, f32::min);
    let x1 = ws.iter().map(|w| w.x1).fold(0.0, f32::max);
    [x0, y0, x1, y1]
}

/// Split `Eq.(3.2)` and `§12` into keyword and label tokens, sharing the word's extent.
fn tokens(r: &Row) -> Vec<W> {
    let mut out = Vec::with_capacity(r.words.len());
    for w in &r.words {
        let split = if let Some(rest) = w.t.strip_prefix("§§").or_else(|| w.t.strip_prefix('§')).filter(|r| !r.is_empty()) {
            Some((w.t.len() - rest.len(), true))
        } else {
            w.t.find(['(', '[']).filter(|&i| i >= 2 && w.t[..i].chars().all(|c| c.is_alphabetic() || c == '.')).map(|i| (i, false))
        };
        match split {
            Some((i, _)) => {
                let f = i as f32 / w.t.len() as f32;
                let xm = w.x0 + (w.x1 - w.x0) * f;
                out.push(W { t: w.t[..i].to_string(), x0: w.x0, x1: xm });
                out.push(W { t: w.t[i..].to_string(), x0: xm, x1: w.x1 });
            }
            None => out.push(w.clone()),
        }
    }
    out
}

fn refs_in_row(page: u32, r: &Row, out: &mut Vec<RawRef>) {
    let t = tokens(r);
    // a row that opens with `Theorem 13.1.` / `Figure 2` is the target itself, not a reference
    let mut i = if statement_head(r).is_some() || caption_head(r).is_some() { 2 } else { 0 };
    let mut consumed = vec![false; t.len()];
    while i < t.len() {
        let kw = trim_tail(&t[i].t).to_lowercase();
        let Some(kind) = kind_for_keyword(&kw).filter(|_| i + 1 < t.len()) else {
            i += 1;
            continue;
        };
        // labels: the one after the keyword, then `and`/`,` lists
        let label_at = |k: usize| -> Option<String> {
            let tok = trim_tail(&t[k].t).trim_end_matches(')');
            if kind == Kind::Equation {
                eq_tag(&t[k].t).map(|l| l.0).or_else(|| any_label(tok).map(str::to_string))
            } else {
                any_label(tok.trim_start_matches('(')).map(str::to_string)
            }
        };
        let mut j = i + 1;
        let mut first = true;
        while let Some(label) = label_at(j) {
            let ws: Vec<&W> = if first { vec![&t[i], &t[j]] } else { vec![&t[j]] };
            out.push(RawRef { page, rect: rect_of(&ws, r.y0, r.y1), kind, label });
            consumed[j] = true;
            first = false;
            let sep = j + 2 < t.len() && matches!(t[j + 1].t.to_lowercase().as_str(), "and" | "&" | "or") && label_at(j + 2).is_some();
            let comma = t[j].t.ends_with(',') && j + 1 < t.len() && label_at(j + 1).is_some();
            if sep {
                j += 2;
            } else if comma {
                j += 1;
            } else {
                break;
            }
        }
        i = j.max(i) + 1;
    }
    // bare "(3.2)" and "by (12)" refer to equations
    for k in 0..t.len() {
        if consumed[k] {
            continue;
        }
        let Some((label, true)) = eq_tag(&t[k].t) else { continue };
        let dotted = label.contains('.');
        let prev = k.checked_sub(1).map(|p| trim_tail(&t[p].t).to_lowercase());
        let after_prep = prev.as_deref().is_some_and(|p| PREPS.contains(&p));
        if (dotted && k > 0) || after_prep {
            out.push(RawRef { page, rect: rect_of(&[&t[k]], r.y0, r.y1), kind: Kind::Equation, label });
        }
    }
}

// ---- targets -----------------------------------------------------------------------------

fn truncate(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(n).collect::<String>().trim_end())
    }
}

fn gap_above(rows: &[Row], i: usize) -> f32 {
    if i == 0 { 1000.0 } else { rows[i].y0 - rows[i - 1].y1 }
}

fn prose_like(r: &Row) -> bool {
    r.words.iter().filter(|w| w.t.chars().filter(|c| c.is_alphabetic()).count() >= 3).count() >= 7
}

/// A body-text line: prose-like, nearly full width and without the big gaps of figure labels
/// that merely share a baseline.
fn solid_prose(r: &Row, m: &Metrics) -> bool {
    let gap = r.words.windows(2).map(|w| w[1].x0 - w[0].x1).fold(0.0, f32::max);
    prose_like(r) && r.size >= 0.8 * m.body && r.x1 - r.x0 > 0.6 * m.bw && gap < 2.5 * r.size.max(6.0)
}

fn is_label_row(r: &Row) -> bool {
    r.words.last().is_some_and(|w| eq_tag(&w.t).is_some())
}

fn display_like(r: &Row, m: &Metrics) -> bool {
    r.x0 >= m.lm + 0.05 * m.bw && r.words.len() <= 18 && r.x1 <= m.rm - 0.02 * m.bw && !prose_like(r)
}

/// `Theorem 13.1.` / `Definition 2` at the start of a row: (kind, label).
/// `B illustrates`: a panel letter followed by a lowercase word is prose, not a caption.
fn panel_prose(r: &Row) -> bool {
    let (a, b) = (r.words.get(2), r.words.get(3));
    let letter = a.and_then(|w| (w.t.chars().count() == 1).then(|| w.t.chars().next().unwrap()));
    let lower = b.filter(|w| w.t.chars().next().is_some_and(char::is_lowercase));
    match (letter, lower) {
        (Some('A'), Some(w)) => w.t.ends_with('s'), // "A shows" is prose, "A diagram of" is a caption
        (Some(c), Some(_)) => ('B'..='H').contains(&c),
        _ => false,
    }
}

fn statement_head(r: &Row) -> Option<(Kind, String)> {
    let w0 = &r.words[0];
    let kw = trim_tail(&w0.t);
    let kind = kind_for_keyword(&kw.to_lowercase()).filter(|k| k.is_statement())?;
    if !kw.chars().next().is_some_and(char::is_uppercase) || kw.ends_with('s') || r.words.len() < 2 || panel_prose(r) {
        return None;
    }
    let lab_tok = &r.words[1].t;
    let label = plain_label(trim_tail(lab_tok).trim_end_matches(')'))?;
    let delim = lab_tok.ends_with('.') || lab_tok.ends_with(':');
    let next = r.words.get(2).map(|w| w.t.as_str());
    let next_ok = next.is_none_or(|n| n.starts_with('(') || n.chars().next().is_some_and(|c| !c.is_lowercase()));
    (delim || (next_ok && next.is_some_and(|n| n.starts_with('(') || n.chars().next().is_some_and(char::is_uppercase)))).then(|| (kind, label.to_string()))
}

fn caption_head(r: &Row) -> Option<(Kind, String)> {
    let kw = trim_tail(&r.words[0].t);
    let kind = kind_for_keyword(&kw.to_lowercase()).filter(|k| matches!(k, Kind::Figure | Kind::Table | Kind::Scheme | Kind::Chart))?;
    if !kw.chars().next().is_some_and(char::is_uppercase) || kw.ends_with('s') || r.words.len() < 2 || panel_prose(r) {
        return None;
    }
    let lab_tok = &r.words[1].t;
    let label = plain_label(trim_tail(lab_tok).trim_end_matches(['|', '-']))?;
    let delim = lab_tok.ends_with(['.', ':', '|']) || r.words.get(2).is_some_and(|w| matches!(w.t.as_str(), "|" | "-" | "–" | "—" | ":"));
    let next_upper = r.words.get(2).is_some_and(|w| w.t.chars().next().is_some_and(char::is_uppercase));
    (delim || next_upper || r.words.len() == 2).then(|| (kind, label.to_string()))
}

fn starts_target(r: &Row) -> bool {
    statement_head(r).is_some() || caption_head(r).is_some() || r.text.to_lowercase().starts_with("proof")
}

fn find_equations(p: &PageIn, rows: &[Row], m: &Metrics, out: &mut Vec<Target>) {
    for (i, r) in rows.iter().enumerate() {
        let n = r.words.len();
        let last = &r.words[n - 1];
        let Some((label, bracketed)) = eq_tag(&last.t) else { continue };
        if r.text.contains('…') || last.x1 < m.rm - 0.12 * m.bw {
            continue;
        }
        let gap = if n >= 2 { last.x0 - r.words[n - 2].x1 } else { 1000.0 };
        let need = if bracketed { (2.5 * r.size).max(18.0) } else { (4.0 * r.size).max(30.0) };
        if gap < need {
            continue;
        }
        let before = &r.words[..n - 1];
        // table rows end in numbers too: a bare `4.8` after other numbers is a value, not a tag
        let numeric = |w: &W| w.t.chars().any(|c| c.is_ascii_digit()) && w.t.chars().filter(|c| c.is_alphabetic()).count() <= 1;
        // an unbracketed tag must sit on a line of mathematics, not a table
        if !bracketed && (before.iter().any(numeric) || !(r.text.contains('=') || r.text.contains('≈') || r.text.contains('∑'))) {
            continue;
        }
        if before.iter().filter(|w| w.t.chars().filter(|c| c.is_alphabetic()).count() >= 4).count() > 6 {
            continue; // prose that happens to end in a number
        }
        if starts_target(r) {
            continue;
        }
        let alone = n == 1;
        let cap = 0.3 * p.h;
        let (mut top, mut bot) = (i, i);
        while top > 0 && display_like(&rows[top - 1], m) && !is_label_row(&rows[top - 1]) && rows[top].y0 - rows[top - 1].y1 <= 1.6 * r.size && rows[bot].y1 - rows[top - 1].y0 < cap {
            top -= 1;
        }
        while bot + 1 < rows.len() && display_like(&rows[bot + 1], m) && !is_label_row(&rows[bot + 1]) && rows[bot + 1].y0 - rows[bot].y1 <= 1.6 * r.size && rows[bot + 1].y1 - rows[top].y0 < cap {
            bot += 1;
        }
        if alone && top == bot {
            continue; // a lone number with nothing around it
        }
        let blk = &rows[top..=bot];
        let x0 = blk.iter().map(|r| r.x0).fold(f32::MAX, f32::min);
        let x1 = blk.iter().map(|r| r.x1).fold(0.0, f32::max);
        let text: Vec<&str> = blk.iter().flat_map(|r| r.words.iter()).filter(|w| !std::ptr::eq(*w, last)).map(|w| w.t.as_str()).collect();
        out.push(Target {
            kind: Kind::Equation,
            label,
            page: p.page,
            rect: [(x0 - 3.0).max(0.0), (blk[0].y0 - 2.0).max(0.0), (x1 + 3.0).min(p.w), (blk[blk.len() - 1].y1 + 2.0).min(p.h)],
            text: truncate(&text.join(" "), 70),
        });
    }
}

fn find_statements(p: &PageIn, rows: &[Row], m: &Metrics, out: &mut Vec<Target>) {
    for (i, r) in rows.iter().enumerate() {
        let Some((kind, label)) = statement_head(r) else { continue };
        if r.text.contains('…') {
            continue;
        }
        let starts = i == 0 || gap_above(rows, i) > 0.35 * r.size || r.x0 > rows[i - 1].x0 + 5.0 || rows[i - 1].text.ends_with(['.', ':', '?', '!']);
        if !starts {
            continue;
        }
        let cap = 0.4 * p.h;
        let mut j = i + 1;
        while j < rows.len() {
            let n = &rows[j];
            let para = gap_above(rows, j) > 0.55 * r.size && !display_like(n, m);
            let heading = n.size > 1.15 * r.size && n.bold;
            if para || heading || starts_target(n) || n.y1 - r.y0 > cap {
                break;
            }
            j += 1;
        }
        let last = &rows[j - 1];
        let body: Vec<&str> = r.words.iter().skip(2).map(|w| w.t.as_str()).collect();
        out.push(Target {
            kind,
            label,
            page: p.page,
            rect: [(m.lm - 4.0).min(r.x0).max(0.0), (r.y0 - 2.0).max(0.0), (m.rm + 4.0).max(last.x1).min(p.w), (last.y1 + 2.0).min(p.h)],
            text: truncate(&body.join(" "), 80),
        });
    }
}

fn find_captions(p: &PageIn, rows: &[Row], m: &Metrics, out: &mut Vec<Target>) {
    for (i, r) in rows.iter().enumerate() {
        let Some((kind, label)) = caption_head(r) else { continue };
        if r.text.contains('…') {
            continue;
        }
        // a full-width body-size sentence that merely opens with `Figure 16.2.` is prose, not a caption
        if m.body > 0.0 && r.size >= 0.97 * m.body && solid_prose(r, m) && r.x1 - r.x0 > 0.9 * m.bw {
            continue;
        }
        let starts = i == 0 || gap_above(rows, i) > 0.3 * r.size || r.x0 > rows[i - 1].x0 + 5.0 || rows[i - 1].text.ends_with(['.', ':']) || !prose_like(&rows[i - 1]);
        if !starts {
            continue;
        }
        // caption continues over wrapped lines
        let mut cend = i;
        while cend + 1 < rows.len() && gap_above(rows, cend + 1) < 0.6 * r.size && !starts_target(&rows[cend + 1]) && (cend == i || prose_like(&rows[cend]) || rows[cend].words.len() >= 3) && (rows[cend + 1].size - r.size).abs() < 0.12 * r.size {
            cend += 1;
        }
        let cap_y0 = r.y0;
        let cap_y1 = rows[cend].y1;
        let cap_h = if kind == Kind::Table { 0.88 * p.h } else { 0.6 * p.h };
        // region above: images chained to the caption, else back to the previous prose row
        let above = |from: f32| -> f32 {
            let mut top = from;
            let mut again = true;
            while again {
                again = false;
                for im in &p.images {
                    if im[3] <= top + 4.0 && im[3] >= top - 40.0 && im[1] < top && (im[2] - im[0]) > 30.0 {
                        top = im[1];
                        again = true;
                    }
                }
            }
            if top < from - 1.0 {
                return top;
            }
            // never above the running page header
            let header = rows.iter().take_while(|x| x.y0 < 0.06 * p.h && x.y1 < 0.09 * p.h).map(|x| x.y1).fold(0.0, f32::max);
            let prose_end = rows[..i].iter().rev().find(|x| solid_prose(x, m)).map_or(header, |x| x.y1.max(header));
            prose_end + 3.0
        };
        let below = || -> f32 {
            let prose = rows[cend + 1..].iter().find(|x| solid_prose(x, m)).map_or(p.h - 20.0, |x| x.y0);
            prose - 2.0
        };
        // `caption_last`: the caption sits at the bottom of the region (figure above its caption)
        let (y0, y1, caption_last) = if kind == Kind::Table {
            let b = below();
            if b - cap_y1 > 30.0 { (cap_y0, b, false) } else { (above(cap_y0), cap_y1, true) }
        } else {
            let a = above(cap_y0);
            if cap_y0 - a > 25.0 { (a, cap_y1, true) } else { (cap_y0, below().min(cap_y1 + 200.0), false) }
        };
        // an over-tall region keeps the end that holds the caption
        let (y0, y1) = if y1 - y0 <= cap_h {
            (y0, y1)
        } else if caption_last {
            (y1 - cap_h, y1)
        } else {
            (y0, y0 + cap_h)
        };
        let mut x0 = m.lm.min(r.x0);
        let mut x1 = m.rm;
        for im in p.images.iter().filter(|im| im[1] >= y0 - 2.0 && im[3] <= y1 + 2.0) {
            x0 = x0.min(im[0]);
            x1 = x1.max(im[2]);
        }
        let body: Vec<&str> = r.words.iter().skip(2).map(|w| w.t.as_str()).collect();
        out.push(Target {
            kind,
            label,
            page: p.page,
            rect: [(x0 - 6.0).max(0.0), (y0 - 6.0).max(0.0), (x1 + 6.0).min(p.w), (y1 + 4.0).min(p.h)],
            text: truncate(&body.join(" "), 80),
        });
    }
}

/// Developer aid: the merged rows of a page with their geometry.
pub fn debug_rows(p: &PageIn) -> Vec<String> {
    rows(&p.lines).iter().map(|r| format!("y {:6.1}-{:6.1} x {:5.1}-{:5.1} sz {:4.1} | {}", r.y0, r.y1, r.x0, r.x1, r.size, r.text)).collect()
}

/// Targets and references found on one page.
pub fn extract_page(p: &PageIn) -> (Vec<Target>, Vec<RawRef>) {
    let rows = rows(&p.lines);
    if rows.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let m = metrics(&rows, p.w);
    let mut targets = Vec::new();
    find_equations(p, &rows, &m, &mut targets);
    find_statements(p, &rows, &m, &mut targets);
    find_captions(p, &rows, &m, &mut targets);
    let mut refs = Vec::new();
    for r in &rows {
        // running headers / footers (`§21 Connected Subspaces ... 147`) are not references
        if r.words.len() <= 10 && (r.y1 < 0.085 * p.h || r.y0 > 0.925 * p.h) {
            continue;
        }
        refs_in_row(p.page, r, &mut refs);
    }
    (targets, refs)
}

// ---- sections from the index -------------------------------------------------------------

/// `§13 Basis`, `1.2 Foo`, `Chapter 3 Bar` -> (kind, label).
pub fn section_label(title: &str) -> Option<(Kind, String)> {
    let t = title.trim();
    let mut it = t.split_whitespace();
    let first = it.next()?;
    let lower = first.to_lowercase();
    if lower == "chapter" || lower == "section" {
        let lab = trim_tail(it.next()?);
        return any_label(lab).map(|l| (if lower == "chapter" { Kind::Chapter } else { Kind::Section }, l.to_string()));
    }
    let tok = trim_tail(first.trim_start_matches('§'));
    // "3" alone is a chapter/section number only when a title follows
    (it.next().is_some()).then(|| plain_label(tok)).flatten().map(|l| (Kind::Section, l.to_string()))
}

pub fn section_targets(index: &[crate::cache::IndexEntry], sizes: &[(f32, f32)]) -> Vec<Target> {
    let mut out = Vec::new();
    for (i, e) in index.iter().enumerate() {
        let Some((kind, label)) = section_label(&e.title) else { continue };
        let Some(&(w, h)) = sizes.get(e.page as usize) else { continue };
        let next_y = index.get(i + 1).filter(|n| n.page == e.page).map(|n| n.y);
        let y1 = next_y.unwrap_or(e.y + 0.2 * h).min(e.y + 0.3 * h).max(e.y + 40.0).min(h);
        out.push(Target { kind, label, page: e.page, rect: [0.0, (e.y - 4.0).max(0.0), w, y1], text: truncate(&e.title, 80) });
    }
    out
}

// ---- resolution --------------------------------------------------------------------------

pub struct Resolver<'a> {
    pub targets: &'a [Target],
    /// Chapter number of each PDF page (any consistent id).
    pub chapter_of: &'a [u32],
}

fn inside(r: &[f32; 4], t: &[f32; 4]) -> bool {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    cx >= t[0] && cx <= t[2] && cy >= t[1] && cy <= t[3]
}

impl Resolver<'_> {
    /// Target index for a reference, or `None` when missing or ambiguous.
    pub fn resolve(&self, by_key: &HashMap<(Kind, &str), Vec<u32>>, r: &RawRef) -> Option<u32> {
        let mut cands: &[u32] = by_key.get(&(r.kind, r.label.as_str())).map_or(&[], |v| v.as_slice());
        let base;
        if cands.is_empty() && r.label.ends_with(|c: char| c.is_ascii_lowercase()) {
            base = &r.label[..r.label.len() - 1];
            cands = by_key.get(&(r.kind, base)).map_or(&[], |v| v.as_slice());
        }
        let ok = |&&i: &&u32| {
            let t = &self.targets[i as usize];
            !(t.page == r.page && inside(&r.rect, &t.rect)) // a reference inside its own target
        };
        // a bare number (`Rule 4`, `Exercise 7`) is numbered within its chapter: the target must be close
        // (same chapter, else within 40 pages), not the same-numbered item of a distant appendix
        let bare = !r.label.contains('.') && !matches!(r.kind, Kind::Section | Kind::Chapter | Kind::Equation | Kind::Figure | Kind::Table | Kind::Scheme | Kind::Chart);
        let ch = self.chapter_of.get(r.page as usize).copied();
        let near = |i: &u32| {
            let t = &self.targets[*i as usize];
            !bare || (t.page as i64 - r.page as i64).abs() <= 40 || (ch.is_some() && self.chapter_of.get(t.page as usize).copied() == ch)
        };
        let cands: Vec<u32> = cands.iter().filter(ok).filter(|i| near(i)).copied().collect();
        match cands.len() {
            0 => None,
            1 => Some(cands[0]),
            _ => {
                let ch = self.chapter_of.get(r.page as usize).copied();
                let same: Vec<u32> = cands.iter().copied().filter(|&i| self.chapter_of.get(self.targets[i as usize].page as usize).copied() == ch).collect();
                // numbering restarts per chapter or section: the nearest one in the same chapter
                let dist = |i: u32| {
                    let t = &self.targets[i as usize];
                    // a target after the reference is a worse guess than one before
                    if t.page <= r.page { (r.page - t.page) * 2 } else { (t.page - r.page) * 2 + 1 }
                };
                let mut best: Vec<(u32, u32)> = same.iter().map(|&i| (dist(i), i)).collect();
                best.sort();
                match best.as_slice() {
                    [] => None,
                    [(d, i)] => (*d < 120).then_some(*i),
                    [(d1, i), (d2, _), ..] => (d1 < d2 && *d1 < 120).then_some(*i),
                }
            }
        }
    }
}

/// An internal link destination `(page, y)`: the target containing it, or a new `Link` target.
pub fn link_target(targets: &mut Vec<Target>, synth: &mut HashMap<(u32, i32), u32>, page: u32, y: f32, size: (f32, f32)) -> u32 {
    let hit = targets.iter().position(|t| t.kind != Kind::Link && t.page == page && y >= t.rect[1] - 14.0 && y <= t.rect[3]);
    if let Some(i) = hit {
        return i as u32;
    }
    let key = (page, (y / 4.0) as i32);
    *synth.entry(key).or_insert_with(|| {
        targets.push(Target { kind: Kind::Link, label: String::new(), page, rect: [0.0, (y - 6.0).max(0.0), size.0, (y + 0.25 * size.1).min(size.1)], text: String::new() });
        (targets.len() - 1) as u32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of words 6 pt per character starting at `x`.
    fn ln(y: f32, x: f32, text: &str) -> PLine {
        let mut cur = x;
        let words = text
            .split(' ')
            .map(|t| {
                let w = W { t: t.to_string(), x0: cur, x1: cur + 6.0 * t.chars().count() as f32 };
                cur = w.x1 + 6.0;
                w
            })
            .collect();
        PLine { words, y0: y, y1: y + 10.0, size: 10.0, bold: false }
    }

    /// A line whose last word sits at `x_last`.
    fn ln_label(y: f32, x: f32, text: &str, label: &str, x_last: f32) -> PLine {
        let mut l = ln(y, x, text);
        l.words.push(W { t: label.into(), x0: x_last, x1: x_last + 6.0 * label.len() as f32 });
        l
    }

    fn prose(y: f32) -> PLine {
        ln(y, 72.0, "the quick brown fox jumps over the lazy dog and keeps running far away from here today now")
    }

    fn page(lines: Vec<PLine>) -> PageIn {
        PageIn { page: 3, w: 612.0, h: 792.0, lines, images: vec![] }
    }

    /// A column of prose lines (body width 72..~560) so metrics are sensible.
    fn body(n: usize, skip: &[usize]) -> Vec<PLine> {
        (0..n).filter(|i| !skip.contains(i)).map(|i| {
            let mut l = prose(100.0 + 14.0 * i as f32);
            // stretch to the right margin
            let last = l.words.len() - 1;
            l.words[last].x1 = 540.0;
            l
        }).collect()
    }

    #[test]
    fn right_aligned_label_is_an_equation_but_inline_numbers_are_not() {
        let mut lines = body(10, &[4, 5, 6]);
        lines.push(ln_label(170.0, 200.0, "E = m c ^ 2", "(3.2)", 520.0));
        // prose line ending in a parenthesised number: no big gap
        let mut inline = prose(240.0);
        inline.words.push(W { t: "(3.2)".into(), x0: 542.0, x1: 570.0 });
        lines.push(inline);
        let (t, _) = extract_page(&page(lines));
        let eqs: Vec<&Target> = t.iter().filter(|t| t.kind == Kind::Equation).collect();
        assert_eq!(eqs.len(), 1, "{eqs:?}");
        assert_eq!(eqs[0].label, "3.2");
        assert_eq!(eqs[0].text, "E = m c ^ 2");
        assert!(eqs[0].rect[1] < 170.0 && eqs[0].rect[3] > 180.0);
    }

    #[test]
    fn equation_block_extends_over_indented_display_lines() {
        let mut lines = body(10, &[4, 5, 6, 7]);
        lines.push(ln(156.0, 220.0, "a = b + c"));
        lines.push(ln_label(170.0, 220.0, "d = e + f", "(12)", 530.0));
        lines.push(ln(184.0, 220.0, "g = h"));
        let (t, _) = extract_page(&page(lines));
        let e = t.iter().find(|t| t.kind == Kind::Equation).unwrap();
        assert_eq!(e.label, "12");
        assert!(e.rect[1] <= 156.0 && e.rect[3] >= 194.0, "{:?}", e.rect);
    }

    #[test]
    fn statements_with_run_in_headings() {
        let mut lines = body(10, &[4, 5, 6, 7]);
        lines.push(ln(160.0, 72.0, "Theorem 13.1. Let X be a set and B a basis for a topology on X"));
        lines.push(ln(174.0, 72.0, "then the collection is a topology on X as claimed above here"));
        lines.push(ln(204.0, 72.0, "Proof. Straightforward from the definition of a basis element"));
        // prose mention, not a heading: lowercase continuation, no delimiter
        lines.push(ln(240.0, 72.0, "Theorem 13.2 shows that every basis generates a topology"));
        let (t, _) = extract_page(&page(lines));
        let s: Vec<&Target> = t.iter().filter(|t| t.kind == Kind::Theorem).collect();
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].label, "13.1");
        assert!(s[0].rect[3] >= 184.0 && s[0].rect[3] < 204.0, "ends before the proof: {:?}", s[0].rect);
        assert!(s[0].text.starts_with("Let X be"));
    }

    #[test]
    fn bold_runin_without_delimiter() {
        let mut lines = body(8, &[4, 5, 6]);
        let d = ln(160.0, 72.0, "Definition 2.1 A group is a set with an operation");
        lines.push(d);
        let (t, _) = extract_page(&page(lines));
        assert!(t.iter().any(|t| t.kind == Kind::Definition && t.label == "2.1"));
    }

    #[test]
    fn captions_figure_scheme_table() {
        let mut lines = body(6, &[2, 3, 4, 5]);
        lines.push(ln(300.0, 72.0, "Figure 1.4. Orbital overlap in the sigma bond"));
        lines.push(ln(500.0, 72.0, "Scheme 3 | Synthesis of the target"));
        lines.push(ln(520.0, 72.0, "Table 2: Bond lengths"));
        lines.push(ln(540.0, 100.0, "C-C 1.54 C=C 1.34"));
        lines.push(ln(600.0, 72.0, "Figure 3 shows that things happen in a continuing sentence like this one"));
        let mut p = page(lines);
        p.images.push([100.0, 180.0, 400.0, 295.0]);
        let (t, _) = extract_page(&p);
        let f = t.iter().find(|t| t.kind == Kind::Figure).unwrap();
        assert_eq!(f.label, "1.4");
        assert!(f.rect[1] <= 181.0 && f.rect[3] >= 310.0, "image included: {:?}", f.rect);
        assert!(t.iter().any(|t| t.kind == Kind::Scheme && t.label == "3"));
        let tb = t.iter().find(|t| t.kind == Kind::Table).unwrap();
        assert!(tb.rect[3] > 550.0, "table region runs below the caption: {:?}", tb.rect);
        assert_eq!(t.iter().filter(|t| t.kind == Kind::Figure).count(), 1, "prose mention is not a caption");
    }

    fn refs(text: &str) -> Vec<(Kind, String)> {
        let mut v = Vec::new();
        let row = rows(&[ln(100.0, 72.0, text)]);
        refs_in_row(0, &row[0], &mut v);
        v.into_iter().map(|r| (r.kind, r.label)).collect()
    }

    #[test]
    fn reference_patterns() {
        use Kind::*;
        assert_eq!(refs("as shown in Eq. (3.2) above"), [(Equation, "3.2".into())]);
        assert_eq!(refs("combining Eqs. (1) and (2) we get"), [(Equation, "1".into()), (Equation, "2".into())]);
        assert_eq!(refs("by equation (3.2), the claim"), [(Equation, "3.2".into())]);
        assert_eq!(refs("see Eq.(7) for details"), [(Equation, "7".into())]);
        assert_eq!(refs("using (3.4) we find"), [(Equation, "3.4".into())]);
        assert_eq!(refs("by (12) it follows"), [(Equation, "12".into())]);
        assert!(refs("the items (1) and (2) are listed").is_empty(), "plain bare numbers need a preposition");
        assert_eq!(refs("By Theorem 13.1, and Lemma 2 we see"), [(Theorem, "13.1".into()), (Lemma, "2".into())]);
        assert_eq!(refs("as in Fig. 1.4a and Table 2."), [(Figure, "1.4a".into()), (Table, "2".into())]);
        assert_eq!(refs("see Scheme 3 and Figures 1 and 2"), [(Scheme, "3".into()), (Figure, "1".into()), (Figure, "2".into())]);
        assert_eq!(refs("recall §12 and Section 3.2 or Chapter 4"), [(Section, "12".into()), (Section, "3.2".into()), (Chapter, "4".into())]);
        assert_eq!(refs("see § 12 here"), [(Section, "12".into())]);
        assert!(refs("Theorem is great and Table of contents").is_empty());
    }

    fn tgt(kind: Kind, label: &str, page: u32) -> Target {
        Target { kind, label: label.into(), page, rect: [50.0, 100.0, 500.0, 150.0], text: String::new() }
    }

    fn rr(kind: Kind, label: &str, page: u32) -> RawRef {
        RawRef { page, rect: [60.0, 400.0, 100.0, 410.0], kind, label: label.into() }
    }

    fn resolve_all(targets: &[Target], chapters: &[u32], r: &RawRef) -> Option<u32> {
        let mut by: HashMap<(Kind, &str), Vec<u32>> = HashMap::new();
        for (i, t) in targets.iter().enumerate() {
            by.entry((t.kind, t.label.as_str())).or_default().push(i as u32);
        }
        Resolver { targets, chapter_of: chapters }.resolve(&by, r)
    }

    #[test]
    fn resolution_rules() {
        let chapters: Vec<u32> = (0..40).map(|p| if p < 20 { 1 } else { 2 }).collect();
        let t = vec![tgt(Kind::Lemma, "2", 5), tgt(Kind::Lemma, "2", 25), tgt(Kind::Theorem, "13.1", 8), tgt(Kind::Equation, "3.2", 9)];
        // unique label resolves anywhere
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Theorem, "13.1", 30)), Some(2));
        // chapter-local numbering: pick the one in the reference's chapter
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Lemma, "2", 7)), Some(0));
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Lemma, "2", 27)), Some(1));
        // two in the same chapter at equal distance: ambiguous
        let t2 = vec![tgt(Kind::Lemma, "2", 4), tgt(Kind::Lemma, "2", 4)];
        assert_eq!(resolve_all(&t2, &chapters, &rr(Kind::Lemma, "2", 6)), None);
        // nothing in the reference's chapter: skip rather than guess
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Lemma, "2", 39)), Some(1));
        let t3 = vec![tgt(Kind::Lemma, "2", 5), tgt(Kind::Lemma, "2", 8)];
        assert_eq!(resolve_all(&t3, &chapters, &rr(Kind::Lemma, "2", 30)), None);
        // missing target
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Equation, "9.9", 3)), None);
        // sub-label falls back to the base label
        assert_eq!(resolve_all(&t, &chapters, &rr(Kind::Equation, "3.2a", 12)), Some(3));
    }

    #[test]
    fn self_references_are_skipped() {
        let chapters = vec![1; 40];
        let t = vec![tgt(Kind::Theorem, "1", 5)];
        let mut r = rr(Kind::Theorem, "1", 5);
        r.rect = [60.0, 110.0, 100.0, 120.0]; // inside the target's own box
        assert_eq!(resolve_all(&t, &chapters, &r), None);
        r.rect = [60.0, 400.0, 100.0, 410.0];
        assert_eq!(resolve_all(&t, &chapters, &r), Some(0));
    }

    #[test]
    fn section_labels_and_link_targets() {
        assert_eq!(section_label("§13 Basis for a Topology"), Some((Kind::Section, "13".into())));
        assert_eq!(section_label("1.2.3 Foo"), Some((Kind::Section, "1.2.3".into())));
        assert_eq!(section_label("Chapter 4 Foo"), Some((Kind::Chapter, "4".into())));
        assert_eq!(section_label("Preface"), None);
        assert_eq!(section_label("2024"), None);
        let mut ts = vec![tgt(Kind::Theorem, "1", 5)];
        let mut syn = HashMap::new();
        assert_eq!(link_target(&mut ts, &mut syn, 5, 110.0, (612.0, 792.0)), 0, "inside an existing target");
        let a = link_target(&mut ts, &mut syn, 7, 300.0, (612.0, 792.0));
        assert_eq!(link_target(&mut ts, &mut syn, 7, 301.0, (612.0, 792.0)), a, "same spot reuses the region");
        assert_eq!(ts[a as usize].kind, Kind::Link);
        assert!(ts[a as usize].rect[3] - ts[a as usize].rect[1] > 150.0);
    }
}
