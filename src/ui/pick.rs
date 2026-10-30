//! A small fuzzy picker panel (`:blist`, `:recent`): rows of label / detail / right-aligned note.

use super::{fuzzy, panel_bg, sel_bg, Canvas, ACCENT, DIM, FG};
use crate::text::TextRenderer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Bookmarks,
    Recent,
}

pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub right: String,
    /// What Enter acts on: the bookmark name or the file path.
    pub arg: String,
}

pub struct Pick {
    pub kind: PickKind,
    pub title: &'static str,
    pub query: String,
    pub sel: usize,
    pub scroll: usize,
    pub items: Vec<PickItem>,
}

impl Pick {
    pub fn new(kind: PickKind, title: &'static str, items: Vec<PickItem>) -> Self {
        Pick { kind, title, query: String::new(), sel: 0, scroll: 0, items }
    }

    pub fn shown(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| fuzzy(&self.query, &format!("{} {}", self.items[i].label, self.items[i].detail))).collect()
    }

    pub fn current(&self) -> Option<&PickItem> {
        self.shown().get(self.sel).map(|&i| &self.items[i])
    }

    pub fn move_sel(&mut self, d: i32) {
        let n = self.shown().len();
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

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, p: &mut Pick, bottom_reserved: i32) {
    cv.fill_rect(0, 0, cv.w, cv.h - bottom_reserved, panel_bg());
    let lh = t.line_h;
    let cw = t.width("M").max(1);
    let pad = cw * 2;
    let shown = p.shown();
    let title = format!("{}  ({} shown; type to filter, Enter select, Esc close)   filter: {}_", p.title, shown.len(), p.query);
    t.draw(cv, pad, 6, &title, ACCENT, cv.w - 2 * pad);
    let top = lh + 14;
    let nrows = ((cv.h - bottom_reserved - top - 6) / lh).max(1) as usize;
    if p.sel < p.scroll {
        p.scroll = p.sel;
    } else if p.sel >= p.scroll + nrows {
        p.scroll = p.sel + 1 - nrows;
    }
    if shown.is_empty() {
        t.draw(cv, pad, top, "Nothing here", DIM, cv.w - 2 * pad);
    }
    for (i, &id) in shown.iter().skip(p.scroll).take(nrows).enumerate() {
        let y = top + i as i32 * lh;
        let it = &p.items[id];
        if p.scroll + i == p.sel {
            cv.fill_rect(0, y, cv.w, lh, sel_bg());
        }
        let col2 = pad + (cw * 24).min(cv.w / 3);
        let rw = t.width(&it.right);
        t.draw(cv, pad, y, &it.label, FG, col2 - pad - cw);
        t.draw(cv, col2, y, &it.detail, DIM, (cv.w - col2 - rw - 3 * pad).max(0));
        t.draw(cv, cv.w - rw - pad, y, &it.right, DIM, rw + 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_and_select() {
        let it = |l: &str, d: &str| PickItem { label: l.into(), detail: d.into(), right: String::new(), arg: l.into() };
        let mut p = Pick::new(PickKind::Bookmarks, "B", vec![it("intro", "p.2"), it("chapter3", "p.30"), it("proofs", "p.99")]);
        assert_eq!(p.shown().len(), 3);
        p.type_char('p');
        p.type_char('r');
        assert_eq!(p.shown(), vec![1, 2]);
        p.move_sel(5);
        assert_eq!(p.current().unwrap().arg, "proofs");
        p.backspace();
        p.backspace();
        assert_eq!(p.current().unwrap().arg, "intro", "selection resets on edit");
    }
}
