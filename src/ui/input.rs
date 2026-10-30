//! One-line input bar (`:` commands and `/` `?` search) with basic editing and completion.

use crate::config::OPTIONS;
use crate::keys::COMMANDS;

pub struct InputBar {
    pub prefix: char,
    pub text: Vec<char>,
    pub cursor: usize,
    hist_pos: Option<usize>,
    draft: Vec<char>,
    comp: Option<Comp>,
}

struct Comp {
    start: usize,
    cands: Vec<String>,
    idx: usize,
}

/// In-memory history, one list per prefix kind.
#[derive(Default)]
pub struct History {
    cmd: Vec<String>,
    search: Vec<String>,
}

impl History {
    fn list(&mut self, prefix: char) -> &mut Vec<String> {
        if prefix == ':' { &mut self.cmd } else { &mut self.search }
    }

    pub fn push(&mut self, prefix: char, s: &str) {
        let l = self.list(prefix);
        if !s.is_empty() && l.last().map(|x| x.as_str()) != Some(s) {
            l.push(s.to_string());
        }
    }
}

impl InputBar {
    pub fn new(prefix: char) -> Self {
        InputBar { prefix, text: Vec::new(), cursor: 0, hist_pos: None, draft: Vec::new(), comp: None }
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    pub fn insert(&mut self, c: char) {
        self.comp = None;
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) -> bool {
        self.comp = None;
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.text.remove(self.cursor);
        true
    }

    pub fn delete(&mut self) {
        self.comp = None;
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Delete the word before the cursor.
    pub fn ctrl_w(&mut self) {
        self.comp = None;
        let mut i = self.cursor;
        while i > 0 && self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        self.text.drain(i..self.cursor);
        self.cursor = i;
    }

    /// Delete from the cursor back to the start of the line.
    pub fn ctrl_u(&mut self) {
        self.comp = None;
        self.text.drain(..self.cursor);
        self.cursor = 0;
    }

    pub fn history(&mut self, hist: &mut History, up: bool) {
        self.comp = None;
        let list = hist.list(self.prefix);
        let next = match (self.hist_pos, up) {
            (None, true) if !list.is_empty() => {
                self.draft = self.text.clone();
                Some(list.len() - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < list.len() => Some(i + 1),
            (Some(_), false) => None,
            _ => return,
        };
        self.hist_pos = next;
        self.text = match next {
            Some(i) => list[i].chars().collect(),
            None => std::mem::take(&mut self.draft),
        };
        self.cursor = self.text.len();
    }

    pub fn tab(&mut self) {
        if self.prefix != ':' {
            return;
        }
        if self.comp.is_none() {
            let s: String = self.text[..self.cursor].iter().collect();
            let (start, cands) = completions(&s);
            if cands.is_empty() {
                return;
            }
            let typed = &s[start..];
            let common = common_prefix(&cands);
            let single = cands.len() == 1;
            let word = if single { format!("{} ", cands[0]) } else if common.len() > typed.len() { common } else { cands[0].clone() };
            self.replace_from(start, &word);
            if !single {
                self.comp = Some(Comp { start, cands, idx: 0 });
            }
        } else if let Some(c) = self.comp.take() {
            let idx = (c.idx + 1) % c.cands.len();
            let word = c.cands[idx].clone();
            self.replace_from(c.start, &word);
            self.comp = Some(Comp { idx, ..c });
        }
    }

    fn replace_from(&mut self, start: usize, word: &str) {
        let tail: Vec<char> = self.text[self.cursor..].to_vec();
        self.text.truncate(start);
        self.text.extend(word.chars());
        self.cursor = self.text.len();
        self.text.extend(tail);
    }
}

fn common_prefix(c: &[String]) -> String {
    let mut p = c[0].clone();
    for s in &c[1..] {
        while !s.starts_with(&p) {
            p.pop();
        }
    }
    p
}

/// Candidates for the word being typed in `line` (text before the cursor): (start byte, matches).
pub fn completions(line: &str) -> (usize, Vec<String>) {
    let words: Vec<&str> = line.split(' ').collect();
    let start = line.len() - words.last().map_or(0, |w| w.len());
    let cur = words.last().copied().unwrap_or("");
    let names: Vec<&str> = match words.as_slice() {
        [_] => COMMANDS.iter().map(|c| c.0).collect(),
        ["set", _] => OPTIONS.iter().map(|o| o.0).collect(),
        _ => Vec::new(),
    };
    (start, names.into_iter().filter(|n| n.starts_with(cur)).map(String::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> InputBar {
        let mut b = InputBar::new(':');
        s.chars().for_each(|c| b.insert(c));
        b
    }

    #[test]
    fn editing() {
        let mut b = typed("set cache-mb 32");
        b.ctrl_w();
        assert_eq!(b.string(), "set cache-mb ");
        b.left();
        b.backspace();
        assert_eq!(b.string(), "set cache-m ");
        b.ctrl_u();
        assert_eq!(b.string(), " ");
        assert_eq!(b.cursor, 0);
    }

    #[test]
    fn history_walk() {
        let mut h = History::default();
        h.push(':', "goto 1");
        h.push(':', "goto 2");
        let mut b = typed("dra");
        b.history(&mut h, true);
        assert_eq!(b.string(), "goto 2");
        b.history(&mut h, true);
        assert_eq!(b.string(), "goto 1");
        b.history(&mut h, false);
        b.history(&mut h, false);
        assert_eq!(b.string(), "dra");
    }

    #[test]
    fn completion() {
        let (s, c) = completions("set cac");
        assert_eq!((s, c), (4, vec!["cache-mb".to_string()]));
        let (_, c) = completions("sear");
        assert!(c.contains(&"search".to_string()) && c.contains(&"search-next".to_string()));
        let mut b = typed("set cac");
        b.tab();
        assert_eq!(b.string(), "set cache-mb ");
        let mut b = typed("search-");
        b.tab();
        let first = b.string();
        b.tab();
        assert_ne!(first, b.string());
        assert!(completions("goto 5").1.is_empty());
    }
}
