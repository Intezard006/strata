//! `:previews` — fuzzy-filterable list of all targets grouped by kind.

use super::{panel_bg, pm, sel_bg, Canvas, ACCENT, DIM, FG};
use crate::previews::{Kind, Target};
use crate::text::TextRenderer;

pub fn plural(k: Kind) -> &'static str {
    match k {
        Kind::Definition => "Definitions",
        Kind::Theorem => "Theorems",
        Kind::Lemma => "Lemmas",
        Kind::Corollary => "Corollaries",
        Kind::Proposition => "Propositions",
        Kind::Example => "Examples",
        Kind::Remark => "Remarks",
        Kind::Exercise => "Exercises",
        Kind::Axiom => "Axioms",
        Kind::Rule => "Rules",
        Kind::Problem => "Problems",
        Kind::Equation => "Equations",
        Kind::Figure => "Figures",
        Kind::Table => "Tables",
        Kind::Scheme => "Schemes",
        Kind::Chart => "Charts",
        Kind::Section => "Sections",
        Kind::Chapter => "Chapters",
        Kind::Link => "Links",
    }
}

pub struct PList {
    pub query: String,
    /// Index into the filtered items.
    pub sel: usize,
    pub scroll: usize,
    /// Target ids sorted by (kind, page, y).
    all: Vec<u32>,
}

#[derive(Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    Item(u32),
}

fn haystack(t: &Target) -> String {
    format!("{} {} {} p{}", t.kind.name(), t.label, t.text, t.page + 1)
}

impl PList {
    pub fn new(targets: &[Target]) -> Self {
        let mut all: Vec<u32> = (0..targets.len() as u32).filter(|&i| targets[i as usize].kind != Kind::Link).collect();
        all.sort_by(|&a, &b| {
            let (x, y) = (&targets[a as usize], &targets[b as usize]);
            x.kind.cmp(&y.kind).then(x.page.cmp(&y.page)).then(x.rect[1].total_cmp(&y.rect[1]))
        });
        PList { query: String::new(), sel: 0, scroll: 0, all }
    }

    /// Filtered target ids in display order.
    pub fn items(&self, targets: &[Target]) -> Vec<u32> {
        let mut v: Vec<u32> = self.all.iter().copied().filter(|&i| super::pick::score(&self.query, &haystack(&targets[i as usize])).is_some()).collect();
        let q = self.query.trim().to_lowercase();
        if !q.is_empty() {
            // exact `figure 6.7` / `6.7` label matches lead their group
            let exact = |i: &u32| {
                let t = &targets[*i as usize];
                q == format!("{} {}", t.kind.name(), t.label).to_lowercase() || q == t.label.to_lowercase()
            };
            v.sort_by_key(|i| (targets[*i as usize].kind, !exact(i)));
        }
        v
    }

    pub fn rows(&self, targets: &[Target]) -> (Vec<Row>, Option<usize>) {
        let mut rows = Vec::new();
        let mut sel_row = None;
        let mut last = None;
        for (n, id) in self.items(targets).into_iter().enumerate() {
            let k = targets[id as usize].kind;
            if last != Some(k) {
                rows.push(Row::Header(plural(k)));
                last = Some(k);
            }
            if n == self.sel {
                sel_row = Some(rows.len());
            }
            rows.push(Row::Item(id));
        }
        (rows, sel_row)
    }

    pub fn current(&self, targets: &[Target]) -> Option<u32> {
        self.items(targets).get(self.sel).copied()
    }

    pub fn move_sel(&mut self, d: i32, targets: &[Target]) {
        let n = self.items(targets).len();
        self.sel = (self.sel as i64 + d as i64).clamp(0, n.saturating_sub(1) as i64) as usize;
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.sel = 0;
        self.scroll = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.sel = 0;
        self.scroll = 0;
    }
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, l: &mut PList, targets: &[Target], map: Option<&crate::pages::PageMap>, bottom_reserved: i32) {
    cv.fill_rect(0, 0, cv.w, cv.h - bottom_reserved, panel_bg());
    let lh = t.line_h;
    let cw = t.width("M").max(1);
    let pad = cw * 2;
    let title = format!("Previews  ({} shown; type to filter, Enter jump, Space/Tab preview, Esc close)   filter: {}_", l.items(targets).len(), l.query);
    t.draw(cv, pad, 6, &title, ACCENT, cv.w - 2 * pad);
    let top = lh + 14;
    let nrows = ((cv.h - bottom_reserved - top - 6) / lh).max(1) as usize;
    let (rows, sel_row) = l.rows(targets);
    if let Some(s) = sel_row {
        if s < l.scroll {
            l.scroll = s.saturating_sub(1);
        } else if s >= l.scroll + nrows {
            l.scroll = s + 1 - nrows;
        }
    }
    if rows.is_empty() {
        t.draw(cv, pad, top, "No previewable items (analysis not finished, or previews are off)", DIM, cv.w - 2 * pad);
    }
    for (i, r) in rows.iter().skip(l.scroll).take(nrows).enumerate() {
        let y = top + i as i32 * lh;
        match r {
            Row::Header(s) => {
                cv.fill_rect(0, y, cv.w, lh, pm([0x45, 0x47, 0x5a], 0x90));
                t.draw(cv, pad, y, s, ACCENT, cv.w - 2 * pad);
            }
            Row::Item(id) => {
                let tg = &targets[*id as usize];
                if sel_row == Some(l.scroll + i) {
                    cv.fill_rect(0, y, cv.w, lh, sel_bg());
                }
                let lab = format!("{} {}", tg.kind.name(), tg.label);
                let col2 = pad + (cw * 22).min(cv.w / 3);
                let page = format!("p.{}", crate::pages::label(map, tg.page));
                let pw = t.width(&page);
                t.draw(cv, pad, y, &lab, FG, col2 - pad);
                t.draw(cv, col2, y, &tg.text, DIM, cv.w - col2 - pw - 3 * pad);
                t.draw(cv, cv.w - pw - pad, y, &page, DIM, pw + 4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(kind: Kind, label: &str, page: u32, text: &str) -> Target {
        Target { kind, label: label.into(), page, rect: [0.0, 10.0, 100.0, 50.0], text: text.into() }
    }

    #[test]
    fn grouped_filtered_and_navigable() {
        let ts = vec![
            t(Kind::Equation, "3.2", 9, "E = mc"),
            t(Kind::Theorem, "13.2", 8, "Every basis"),
            t(Kind::Theorem, "13.1", 7, "Let X be a set"),
            t(Kind::Link, "", 3, ""),
            t(Kind::Figure, "1.4", 2, "Orbital overlap"),
        ];
        let mut l = PList::new(&ts);
        let (rows, sel) = l.rows(&ts);
        assert_eq!(rows[0], Row::Header("Theorems"));
        assert_eq!(rows[1], Row::Item(2), "sorted by page inside a group");
        assert_eq!(sel, Some(1));
        assert_eq!(rows.len(), 4 + 3, "links are hidden; four items, three groups");
        l.type_char('e');
        l.type_char('q');
        assert_eq!(l.items(&ts), vec![0], "fuzzy match across kind and text");
        l.backspace();
        l.backspace();
        l.move_sel(2, &ts);
        assert_eq!(l.current(&ts), Some(0));
        l.move_sel(99, &ts);
        assert_eq!(l.current(&ts), Some(4));
    }
}
