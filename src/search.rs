//! Search matching rules and the UI-side hit list. Matching itself runs on the renderer thread.

/// Hit rectangle in page points, relative to the page origin: `[x0, y0, x1, y1]`.
pub type Rect4 = [f32; 4];

/// Smart case: case-sensitive only when the query contains an uppercase letter.
pub fn case_sensitive(q: &str) -> bool {
    q.chars().any(|c| c.is_uppercase())
}

fn norm(c: char, cs: bool) -> char {
    if c.is_whitespace() {
        ' '
    } else if cs {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Find `query` in `text`; returns `(start, len)` char ranges.
pub fn find_matches(text: &[char], query: &str, cs: bool) -> Vec<(usize, usize)> {
    let q: Vec<char> = query.chars().map(|c| norm(c, cs)).collect();
    let mut out = Vec::new();
    if q.is_empty() || text.len() < q.len() {
        return out;
    }
    let t: Vec<char> = text.iter().map(|&c| norm(c, cs)).collect();
    let mut i = 0;
    while i + q.len() <= t.len() {
        if t[i..i + q.len()] == q[..] {
            out.push((i, q.len()));
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub page: u32,
    pub rect: Rect4,
}

fn key(h: &Hit) -> (u32, i32, i32) {
    (h.page, (h.rect[1] * 10.0) as i32, (h.rect[0] * 10.0) as i32)
}

pub struct Search {
    pub id: u64,
    pub query: String,
    pub forward: bool,
    pub hits: Vec<Hit>,
    pub done: bool,
    pub cur: Option<Hit>,
    /// Jump to the first hit that arrives.
    pub await_first: bool,
}

impl Search {
    pub fn new(id: u64, query: String, forward: bool) -> Self {
        Search { id, query, forward, hits: Vec::new(), done: false, cur: None, await_first: true }
    }

    pub fn add(&mut self, page: u32, rects: &[Rect4]) {
        for &rect in rects {
            let h = Hit { page, rect };
            let pos = self.hits.partition_point(|x| key(x) < key(&h));
            self.hits.insert(pos, h);
        }
    }

    pub fn index(&self) -> Option<usize> {
        let c = self.cur?;
        self.hits.iter().position(|h| *h == c)
    }

    /// Next hit in direction `fwd` (relative to the search direction), wrapping.
    pub fn step(&mut self, fwd: bool) -> Option<Hit> {
        if self.hits.is_empty() {
            return None;
        }
        let n = self.hits.len();
        let i = match (self.index(), fwd) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        self.cur = Some(self.hits[i]);
        self.cur
    }

    /// Hits on `page`.
    pub fn on_page(&self, page: u32) -> &[Hit] {
        let a = self.hits.partition_point(|h| h.page < page);
        let b = self.hits.partition_point(|h| h.page <= page);
        &self.hits[a..b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn smart_case() {
        assert!(!case_sensitive("hello"));
        assert!(case_sensitive("Hello"));
        assert!(!case_sensitive("hello world 123"));
    }

    #[test]
    fn matching_rules() {
        let t = chars("Hello hello HELLO");
        assert_eq!(find_matches(&t, "hello", false).len(), 3);
        assert_eq!(find_matches(&t, "Hello", true), vec![(0, 5)]);
        assert_eq!(find_matches(&chars("a\u{a0}b"), "a b", false).len(), 1);
        assert!(find_matches(&t, "", false).is_empty());
        assert!(find_matches(&t, "zzz", false).is_empty());
    }

    #[test]
    fn hits_sorted_and_stepping() {
        let mut s = Search::new(1, "x".into(), true);
        s.add(3, &[[0.0, 5.0, 1.0, 6.0]]);
        s.add(1, &[[0.0, 9.0, 1.0, 10.0], [0.0, 2.0, 1.0, 3.0]]);
        assert_eq!(s.hits.iter().map(|h| h.page).collect::<Vec<_>>(), [1, 1, 3]);
        assert_eq!(s.hits[0].rect[1], 2.0);
        assert_eq!(s.step(true).unwrap().page, 1);
        assert_eq!(s.index(), Some(0));
        s.step(true);
        assert_eq!(s.step(true).unwrap().page, 3);
        assert_eq!(s.step(true).unwrap().rect[1], 2.0); // wraps
        assert_eq!(s.step(false).unwrap().page, 3);
        assert_eq!(s.on_page(1).len(), 2);
        assert!(s.on_page(2).is_empty());
    }
}
