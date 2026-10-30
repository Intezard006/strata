//! Fuzzy picker panel: `:blist`, `:recent` and the command palette. Items are built when the panel
//! opens and dropped when it closes.

use super::{panel_bg, sel_bg, Canvas, ACCENT, DIM, FG};
use crate::text::TextRenderer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Bookmarks,
    Recent,
    Palette,
}

/// What Enter does with an item.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Run a command; when `needs_args` open the `:` bar prefilled instead.
    Command { name: &'static str, needs_args: bool },
    /// Jump to a page and y offset (index entries, previews).
    Goto(u32, f32),
    Bookmark(String),
    Open(String),
}

/// Palette groups: the prefix typed to restrict to one, and a priority for ranking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Command,
    Index,
    Bookmark,
    Preview,
    Recent,
    /// Plain pickers (`:blist`, `:recent`).
    Plain,
}

impl Group {
    pub fn prefix(self) -> Option<char> {
        Some(match self {
            Group::Command => '>',
            Group::Index => '#',
            Group::Bookmark => '@',
            Group::Preview => '=',
            Group::Recent => '%',
            Group::Plain => return None,
        })
    }

    pub fn from_prefix(c: char) -> Option<Group> {
        [Group::Command, Group::Index, Group::Bookmark, Group::Preview, Group::Recent].into_iter().find(|g| g.prefix() == Some(c))
    }

    fn priority(self) -> i32 {
        match self {
            Group::Command => 30,
            Group::Bookmark => 26,
            Group::Recent => 18,
            Group::Index => 20,
            Group::Preview => 10,
            Group::Plain => 0,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Group::Command => "cmd",
            Group::Index => "idx",
            Group::Bookmark => "bm",
            Group::Preview => "ref",
            Group::Recent => "file",
            Group::Plain => "",
        }
    }
}

pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub right: String,
    pub act: Act,
    pub group: Group,
    /// Extra rank (recent files: newer is higher).
    pub bonus: i32,
}

pub struct Pick {
    pub kind: PickKind,
    pub title: &'static str,
    pub query: String,
    pub sel: usize,
    pub scroll: usize,
    pub items: Vec<PickItem>,
    shown: Vec<usize>,
}

/// Fuzzy score of `needle` in `hay` (subsequence, case-insensitive; whitespace in the needle ignored).
/// Consecutive runs, word starts and a contiguous match score higher. `None` when it does not match.
pub fn score(needle: &str, hay: &str) -> Option<i32> {
    let n: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).filter(|c| !c.is_whitespace()).collect();
    if n.is_empty() {
        return Some(0);
    }
    let h: Vec<char> = hay.chars().flat_map(|c| c.to_lowercase()).collect();
    let (mut ni, mut total, mut last): (usize, i32, Option<usize>) = (0, 0, None);
    for (i, &c) in h.iter().enumerate() {
        if ni < n.len() && c == n[ni] {
            total += 10;
            if last == Some(i.wrapping_sub(1)) {
                total += 8;
            }
            if i == 0 || !h[i - 1].is_alphanumeric() {
                total += 6;
            }
            last = Some(i);
            ni += 1;
        }
    }
    if ni < n.len() {
        return None;
    }
    let joined: String = n.iter().collect();
    if h.iter().collect::<String>().contains(&joined) {
        total += 30;
    }
    Some(total - (h.len() as i32 / 16).min(10))
}

impl Pick {
    pub fn new(kind: PickKind, title: &'static str, items: Vec<PickItem>) -> Self {
        let mut p = Pick { kind, title, query: String::new(), sel: 0, scroll: 0, items, shown: Vec::new() };
        p.refilter();
        p
    }

    /// Recompute the shown list: restrict by prefix, rank by score + group priority + bonus.
    pub fn refilter(&mut self) {
        let mut chars = self.query.chars();
        let (only, needle) = match chars.next().and_then(Group::from_prefix) {
            Some(g) if self.kind == PickKind::Palette => (Some(g), &self.query[1..]),
            _ => (None, &self.query[..]),
        };
        let mut v: Vec<(i32, usize)> = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            if only.is_some_and(|g| g != it.group) {
                continue;
            }
            let Some(s) = score(needle, &format!("{} {}", it.label, it.detail)) else { continue };
            let rank = if needle.trim().is_empty() { it.group.priority() * 100 + it.bonus - (i as i32).min(99) } else { s * 4 + it.group.priority() + it.bonus };
            v.push((rank, i));
        }
        if self.kind == PickKind::Palette {
            v.sort_by_key(|&(r, i)| (std::cmp::Reverse(r), i));
        }
        self.shown = v.into_iter().map(|(_, i)| i).collect();
        self.sel = self.sel.min(self.shown.len().saturating_sub(1));
    }

    #[cfg(test)]
    pub fn shown(&self) -> &[usize] {
        &self.shown
    }

    pub fn current(&self) -> Option<&PickItem> {
        self.shown.get(self.sel).map(|&i| &self.items[i])
    }

    pub fn move_sel(&mut self, d: i32) {
        let n = self.shown.len();
        self.sel = (self.sel as i64 + d as i64).clamp(0, n.saturating_sub(1) as i64) as usize;
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.sel = 0;
        self.scroll = 0;
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.sel = 0;
        self.scroll = 0;
        self.refilter();
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.sel = 0;
        self.scroll = 0;
        self.refilter();
    }
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, p: &mut Pick, bottom_reserved: i32) {
    cv.fill_rect(0, 0, cv.w, cv.h - bottom_reserved, panel_bg());
    let lh = t.line_h;
    let cw = t.width("M").max(1);
    let pad = cw * 2;
    let hint = if p.kind == PickKind::Palette { "  > cmds  # index  @ bookmarks  = previews  % recent" } else { "" };
    let title = format!("{}  ({} shown; Enter select, Esc close){}   filter: {}_", p.title, p.shown.len(), hint, p.query);
    t.draw(cv, pad, 6, &title, ACCENT, cv.w - 2 * pad);
    let top = lh + 14;
    let nrows = ((cv.h - bottom_reserved - top - 6) / lh).max(1) as usize;
    if p.sel < p.scroll {
        p.scroll = p.sel;
    } else if p.sel >= p.scroll + nrows {
        p.scroll = p.sel + 1 - nrows;
    }
    if p.shown.is_empty() {
        t.draw(cv, pad, top, "Nothing here", DIM, cv.w - 2 * pad);
    }
    let tagw = if p.kind == PickKind::Palette { cw * 6 } else { 0 };
    for (i, &id) in p.shown.iter().skip(p.scroll).take(nrows).enumerate() {
        let y = top + i as i32 * lh;
        let it = &p.items[id];
        if p.scroll + i == p.sel {
            cv.fill_rect(0, y, cv.w, lh, sel_bg());
        }
        if tagw > 0 {
            t.draw(cv, pad, y, it.group.tag(), DIM, tagw);
        }
        let x0 = pad + tagw;
        let col2 = x0 + (cw * 24).min(cv.w / 3);
        let rw = t.width(&it.right);
        t.draw(cv, x0, y, &it.label, FG, col2 - x0 - cw);
        t.draw(cv, col2, y, &it.detail, DIM, (cv.w - col2 - rw - 3 * pad).max(0));
        t.draw(cv, cv.w - rw - pad, y, &it.right, DIM, rw + 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(l: &str, d: &str, g: Group) -> PickItem {
        PickItem { label: l.into(), detail: d.into(), right: String::new(), act: Act::Bookmark(l.into()), group: g, bonus: 0 }
    }

    #[test]
    fn scoring_prefers_tight_matches() {
        assert!(score("zoom", "zoom-in").unwrap() > score("zoom", "z-o-o-m").unwrap());
        assert!(score("xyz", "zoom").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("fit w", "fit-width").unwrap() > 0);
    }

    #[test]
    fn filter_prefix_and_rank() {
        let items = vec![
            it("intro", "p.2", Group::Bookmark),
            it("zoom-in", "Zoom in", Group::Command),
            it("Zoom lemma", "p.9", Group::Preview),
            it("zoom.pdf", "/books", Group::Recent),
        ];
        let mut p = Pick::new(PickKind::Palette, "P", items);
        assert_eq!(p.shown().len(), 4);
        assert_eq!(p.items[p.shown()[0]].group, Group::Command, "commands first with an empty query");
        for c in "zoom".chars() {
            p.type_char(c);
        }
        assert_eq!(p.shown().len(), 3);
        assert_eq!(p.items[p.shown()[0]].label, "zoom-in");
        p.clear_query();
        p.type_char('@');
        assert_eq!(p.shown().len(), 1);
        p.type_char('i');
        assert_eq!(p.current().unwrap().label, "intro");
        p.clear_query();
        p.type_char('%');
        p.type_char('z');
        assert_eq!(p.current().unwrap().label, "zoom.pdf");
        p.clear_query();
        p.type_char('=');
        assert_eq!(p.shown().len(), 1);
    }

    #[test]
    fn recency_bonus_breaks_ties() {
        let mut a = it("paper.pdf", "/x", Group::Recent);
        let mut b = it("paper.pdf", "/y", Group::Recent);
        a.bonus = 1;
        b.bonus = 9;
        let mut p = Pick::new(PickKind::Palette, "P", vec![a, b]);
        p.type_char('p');
        assert_eq!(p.current().unwrap().detail, "/y");
    }

    #[test]
    fn plain_picker_keeps_order() {
        let mut p = Pick::new(PickKind::Bookmarks, "B", vec![it("intro", "p.2", Group::Plain), it("chapter3", "p.30", Group::Plain), it("proofs", "p.99", Group::Plain)]);
        p.type_char('p');
        p.type_char('r');
        assert_eq!(p.shown(), &[1, 2]);
        p.move_sel(5);
        assert_eq!(p.current().unwrap().label, "proofs");
        p.backspace();
        p.backspace();
        assert_eq!(p.current().unwrap().label, "intro", "selection resets on edit");
    }
}
