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

/// Score of one query word in `h` (lowercase chars): a contiguous match at a word start scores
/// far above one inside a word, which scores above a tight scattered subsequence (3+ letters only).
fn word_score(w: &[char], h: &[char]) -> Option<i32> {
    let n = w.len();
    let boundary = |i: usize| i == 0 || !h[i - 1].is_alphanumeric();
    let mut best: Option<i32> = None;
    if n <= h.len() {
        for i in 0..=h.len() - n {
            if h[i..i + n] != *w {
                continue;
            }
            let mut s = if boundary(i) { 100 } else if n >= 3 && w.iter().any(|c| c.is_alphabetic()) { 45 } else { continue };
            if i + n == h.len() || !h[i + n].is_alphanumeric() {
                s += 40; // a whole word / label
            }
            best = best.max(Some(s - (i as i32 / 8).min(10)));
        }
    }
    if best.is_some() {
        return best;
    }
    // scattered letters: only for longer words, starting at a word start and kept close together
    if n < 3 || !w.iter().all(|c| c.is_alphabetic()) {
        return None;
    }
    for start in (0..h.len()).filter(|&i| boundary(i) && h[i] == w[0]) {
        let (mut wi, mut last) = (0, start);
        for (k, &c) in h.iter().enumerate().skip(start) {
            if wi < n && c == w[wi] {
                wi += 1;
                last = k;
            }
        }
        if wi == n && last - start < 2 * n + 2 {
            return Some(20);
        }
    }
    None
}

/// Score of `needle` in `hay` (case-insensitive). Every whitespace-separated word of the needle must
/// match; `None` when one does not. Contiguous and word-start matches dominate loose subsequences.
pub fn score(needle: &str, hay: &str) -> Option<i32> {
    let h: Vec<char> = hay.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut total = 0;
    let mut any = false;
    for w in needle.split_whitespace() {
        any = true;
        let w: Vec<char> = w.chars().flat_map(|c| c.to_lowercase()).collect();
        total += word_score(&w, &h)?;
    }
    if !any {
        return Some(0);
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
            let Some(mut s) = score(needle, &format!("{} {} {}", it.label, it.detail, if it.group == Group::Command { it.right.as_str() } else { "" })) else { continue };
            let nl = needle.trim().to_lowercase();
            if !nl.is_empty() {
                if it.label.to_lowercase() == nl {
                    s += 500; // exact label
                }
                if it.group == Group::Command && !it.right.is_empty() && it.right.to_lowercase() == nl.replace(' ', "") {
                    s += 1000; // exact key binding
                }
            }
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
        assert!(score("zoom", "zoom-in").unwrap() > score("zoom", "z-o-o-m").map_or(0, |v| v));
        assert!(score("6.7", "Figure 16.7").is_none(), "a number must start a word");
        assert!(score("figure 6.7", "Figure 6.7 caption").unwrap() > 200);
        assert!(score("gf", "fit-page Fit the page").is_none());
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
        assert_eq!(p.shown(), &[2]);
        p.move_sel(5);
        assert_eq!(p.current().unwrap().label, "proofs");
        p.backspace();
        p.backspace();
        assert_eq!(p.current().unwrap().label, "intro", "selection resets on edit");
    }
}
