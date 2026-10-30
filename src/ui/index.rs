//! Outline (index) panel state and drawing.

use std::collections::HashSet;

use super::{panel_bg, sel_bg, Canvas, ACCENT, DIM, FG};
use crate::text::TextRenderer;

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    pub depth: u32,
    pub page: Option<u32>,
    pub uri: Option<String>,
    pub has_children: bool,
}

#[derive(Default)]
pub struct Index {
    pub items: Vec<Item>,
    expanded: HashSet<usize>,
    /// Indices into `items` that are currently shown.
    pub visible: Vec<usize>,
    pub sel: usize,
    pub scroll: usize,
}

impl Index {
    pub fn new(items: Vec<Item>) -> Self {
        let mut i = Index { items, ..Default::default() };
        i.rebuild();
        i
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let mut skip_depth: Option<u32> = None;
        for (n, it) in self.items.iter().enumerate() {
            if let Some(d) = skip_depth {
                if it.depth > d {
                    continue;
                }
                skip_depth = None;
            }
            self.visible.push(n);
            if it.has_children && !self.expanded.contains(&n) {
                skip_depth = Some(it.depth);
            }
        }
        self.sel = self.sel.min(self.visible.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&Item> {
        self.visible.get(self.sel).map(|&i| &self.items[i])
    }

    pub fn down(&mut self, n: usize) {
        self.sel = (self.sel + n).min(self.visible.len().saturating_sub(1));
    }

    pub fn up(&mut self, n: usize) {
        self.sel = self.sel.saturating_sub(n);
    }

    pub fn first(&mut self) {
        self.sel = 0;
    }

    pub fn last(&mut self) {
        self.sel = self.visible.len().saturating_sub(1);
    }

    /// `l`: expand a collapsed entry, or step into an expanded one.
    pub fn expand(&mut self) {
        let Some(&i) = self.visible.get(self.sel) else { return };
        if !self.items[i].has_children {
            return;
        }
        if self.expanded.insert(i) {
            self.rebuild();
        } else {
            self.down(1);
        }
    }

    /// `h`: collapse an expanded entry, otherwise move to the parent.
    pub fn collapse(&mut self) {
        let Some(&i) = self.visible.get(self.sel) else { return };
        if self.items[i].has_children && self.expanded.remove(&i) {
            self.rebuild();
            return;
        }
        let d = self.items[i].depth;
        if let Some(p) = (0..i).rev().find(|&p| self.items[p].depth < d) {
            if let Some(pos) = self.visible.iter().position(|&v| v == p) {
                self.sel = pos;
            }
        }
    }

    fn keep_visible(&mut self, rows: usize) {
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + rows {
            self.scroll = self.sel + 1 - rows;
        }
    }
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, ix: &mut Index, bottom_reserved: i32) -> usize {
    cv.fill_rect(0, 0, cv.w, cv.h - bottom_reserved, panel_bg());
    let (lh, cw) = (t.line_h, t.width("M").max(1));
    let pad = cw * 2;
    t.draw(cv, pad, 6, "Index  (j/k move, l/h expand/collapse, Enter jump, Tab close)", ACCENT, cv.w - 2 * pad);
    let top = lh + 14;
    let rows = ((cv.h - bottom_reserved - top - 6) / lh).max(1) as usize;
    if ix.items.is_empty() {
        t.draw(cv, pad, top, "No index (auto-index arrives in phase 3)", DIM, cv.w - 2 * pad);
        return rows;
    }
    ix.keep_visible(rows);
    for r in 0..rows {
        let Some(&i) = ix.visible.get(ix.scroll + r) else { break };
        let it = &ix.items[i];
        let y = top + r as i32 * lh;
        if ix.scroll + r == ix.sel {
            cv.fill_rect(0, y, cv.w, lh, sel_bg());
        }
        let marker = if !it.has_children { "  " } else if ix.expanded.contains(&i) { "v " } else { "> " };
        let x = pad + it.depth as i32 * 2 * cw;
        let x = t.draw(cv, x, y, marker, DIM, cv.w);
        let page = it.page.map(|p| format!("{}", p + 1)).unwrap_or_default();
        let pw = t.width(&page) + pad;
        t.draw(cv, x, y, &it.title, FG, (cv.w - x - pw - pad).max(0));
        t.draw(cv, cv.w - pw, y, &page, DIM, pw);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(t: &str, depth: u32, kids: bool) -> Item {
        Item { title: t.into(), depth, page: Some(0), uri: None, has_children: kids }
    }

    #[test]
    fn tree_navigation() {
        let mut ix = Index::new(vec![it("a", 0, true), it("a1", 1, false), it("a2", 1, true), it("a2x", 2, false), it("b", 0, false)]);
        assert_eq!(ix.visible, vec![0, 4]);
        ix.expand();
        assert_eq!(ix.visible, vec![0, 1, 2, 4]);
        assert_eq!(ix.sel, 0);
        ix.expand(); // already expanded: step in
        assert_eq!(ix.sel, 1);
        ix.down(1);
        ix.expand();
        assert_eq!(ix.visible, vec![0, 1, 2, 3, 4]);
        ix.down(1);
        ix.collapse(); // leaf -> parent
        assert_eq!(ix.current().unwrap().title, "a2");
        ix.collapse();
        assert_eq!(ix.visible, vec![0, 1, 2, 4]);
    }
}
