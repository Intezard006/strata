//! Text selection and highlights: character hit-testing, per-line rectangles, the rectangle BLOB
//! format stored in the database, the Markdown export and the snip rectangle math. All pure.

use crate::search::Rect4;

/// One character of a page's text layer: its box in page points (from the page origin) and line number.
#[derive(Clone, Debug, PartialEq)]
pub struct Ch {
    pub c: char,
    pub r: Rect4,
    pub line: u32,
}

/// Highlight colours selected with 1-4 (straight rgb); drawn with alpha.
pub const COLORS: [[u8; 3]; 4] = [[0xf9, 0xe2, 0xaf], [0xa6, 0xe3, 0xa1], [0x89, 0xb4, 0xfa], [0xf5, 0xc2, 0xe7]];

/// Index of the character nearest to the point (page points). Vertical distance counts triple so a
/// point between two lines picks the closer line before the closer column.
pub fn nearest(chars: &[Ch], x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, c) in chars.iter().enumerate() {
        if c.c.is_whitespace() {
            continue;
        }
        let dx = (c.r[0] - x).max(x - c.r[2]).max(0.0);
        let dy = (c.r[1] - y).max(y - c.r[3]).max(0.0);
        let d = dx + 3.0 * dy;
        if best.map_or(true, |(b, _)| d < b) {
            best = Some((d, i));
        }
    }
    best.map(|b| b.1)
}

/// Inclusive character range between two anchors, in reading order.
pub fn span(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

/// One rectangle per line for the characters `lo..=hi`.
pub fn rects(chars: &[Ch], lo: usize, hi: usize) -> Vec<Rect4> {
    let mut out: Vec<Rect4> = Vec::new();
    let mut line = None;
    for c in &chars[lo.min(chars.len())..(hi + 1).min(chars.len())] {
        if c.c.is_whitespace() && line != Some(c.line) {
            continue;
        }
        if line == Some(c.line) {
            let r = out.last_mut().unwrap();
            *r = [r[0].min(c.r[0]), r[1].min(c.r[1]), r[2].max(c.r[2]), r[3].max(c.r[3])];
        } else {
            line = Some(c.line);
            out.push(c.r);
        }
    }
    out
}

/// The selected text: lines joined with a space (a hyphen at a line end joins the word).
pub fn text(chars: &[Ch], lo: usize, hi: usize) -> String {
    let mut s = String::new();
    let mut line = None;
    for c in &chars[lo.min(chars.len())..(hi + 1).min(chars.len())] {
        if line.is_some() && line != Some(c.line) {
            if s.ends_with('-') && s.chars().rev().nth(1).is_some_and(|p| p.is_alphabetic()) {
                s.pop();
            } else if !s.ends_with(' ') {
                s.push(' ');
            }
        }
        line = Some(c.line);
        s.push(c.c);
    }
    s.trim().to_string()
}

pub struct Selection {
    pub page: u32,
    pub rects: Vec<Rect4>,
    pub text: String,
    /// The highlight made from this selection (so `1`-`4` recolour it and `:note` annotates it).
    pub hl: Option<i64>,
    pub color: u8,
}

/// A stored highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub id: i64,
    pub page: u32,
    pub rects: Vec<Rect4>,
    pub color: u8,
    pub text: String,
    pub note: String,
    pub created: i64,
}

pub fn encode_rects(rs: &[Rect4]) -> Vec<u8> {
    rs.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn decode_rects(b: &[u8]) -> Vec<Rect4> {
    b.chunks_exact(16)
        .map(|c| {
            let f = |i: usize| f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]]);
            [f(0), f(1), f(2), f(3)]
        })
        .collect()
}

/// The margin marker of a note: a square just inside the page's right edge, level with the first line.
pub fn marker_rect(h: &Highlight, page_w: f32) -> Option<Rect4> {
    if h.note.is_empty() {
        return None;
    }
    let r = h.rects.first()?;
    Some([page_w - 18.0, r[1], page_w - 6.0, r[1] + 12.0])
}

/// Markdown export grouped by page. `label` names a page (printed number, else the PDF number).
pub fn markdown(title: &str, hs: &[Highlight], label: impl Fn(u32) -> String) -> String {
    let mut sorted: Vec<&Highlight> = hs.iter().collect();
    sorted.sort_by(|a, b| (a.page, a.rects.first().map_or(0, |r| (r[1] * 10.0) as i32), a.id).cmp(&(b.page, b.rects.first().map_or(0, |r| (r[1] * 10.0) as i32), b.id)));
    let mut out = format!("# Highlights: {title}\n");
    let mut cur = None;
    for h in sorted {
        if cur != Some(h.page) {
            cur = Some(h.page);
            let l = label(h.page);
            let pdf = (h.page + 1).to_string();
            if l == pdf {
                out.push_str(&format!("\n## p. {l}\n\n"));
            } else {
                out.push_str(&format!("\n## p. {l} (PDF page {pdf})\n\n"));
            }
        }
        for line in h.text.lines() {
            out.push_str(&format!("> {line}\n"));
        }
        if !h.note.is_empty() {
            out.push_str(&format!("\n**Note:** {}\n", h.note));
        }
        out.push('\n');
    }
    out
}

/// A drag rectangle between two window points, normalised.
pub fn norm_rect(a: (f64, f64), b: (f64, f64)) -> [f64; 4] {
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

/// Intersection of two page-point rectangles; `None` when empty or smaller than `min` points.
pub fn clip(r: Rect4, to: Rect4, min: f32) -> Option<Rect4> {
    let o = [r[0].max(to[0]), r[1].max(to[1]), r[2].min(to[2]), r[3].min(to[3])];
    (o[2] - o[0] >= min && o[3] - o[1] >= min).then_some(o)
}

/// Does `p` lie inside `r` (with `pad` points of slack)?
pub fn hit(r: Rect4, p: (f32, f32), pad: f32) -> bool {
    p.0 >= r[0] - pad && p.0 <= r[2] + pad && p.1 >= r[1] - pad && p.1 <= r[3] + pad
}

/// Scale for a snip of `r` (points): at least `min_scale`, the current screen scale if larger,
/// but never more than `max_px` pixels in total.
pub fn snip_scale(r: Rect4, screen: f32, min_scale: f32, max_px: f32) -> f32 {
    let (w, h) = ((r[2] - r[0]).max(1.0), (r[3] - r[1]).max(1.0));
    screen.max(min_scale).min((max_px / (w * h)).sqrt()).max(0.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, n: u32, y: f32) -> Vec<Ch> {
        text.chars().enumerate().map(|(i, c)| Ch { c, r: [10.0 + i as f32 * 6.0, y, 16.0 + i as f32 * 6.0, y + 10.0], line: n }).collect()
    }

    fn page() -> Vec<Ch> {
        let mut v = line("hello world", 0, 100.0);
        v.extend(line("second-", 1, 114.0));
        v.extend(line("ary line", 2, 128.0));
        v
    }

    #[test]
    fn nearest_prefers_the_closer_line() {
        let p = page();
        assert_eq!(nearest(&p, 12.0, 105.0), Some(0));
        // between line 0 and line 1, nearer to line 1
        let i = nearest(&p, 40.0, 112.5).unwrap();
        assert_eq!(p[i].line, 1);
        // far right of line 0: the last char of line 0
        assert_eq!(nearest(&p, 500.0, 105.0), Some(10));
        assert_eq!(nearest(&[], 0.0, 0.0), None);
    }

    #[test]
    fn rects_are_one_per_line() {
        let p = page();
        let rs = rects(&p, 6, 16); // "world" + part of "second-"
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0], [10.0 + 36.0, 100.0, 16.0 + 60.0, 110.0]);
        assert_eq!(rs[1][1], 114.0);
        assert_eq!(rects(&p, 3, 3).len(), 1);
    }

    #[test]
    fn text_joins_lines_and_hyphens() {
        let p = page();
        assert_eq!(text(&p, 0, 10), "hello world");
        assert_eq!(text(&p, 6, 20), "world secondary");
        assert_eq!(text(&p, 0, 99), "hello world secondary line");
        assert_eq!(span(9, 2), (2, 9));
    }

    #[test]
    fn rect_blob_round_trip() {
        let rs = vec![[1.0, 2.5, 3.0, 4.0], [10.0, 20.0, 30.0, 40.5]];
        assert_eq!(decode_rects(&encode_rects(&rs)), rs);
        assert!(decode_rects(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn markdown_groups_by_page_with_printed_labels() {
        let h = |id, page, y, text: &str, note: &str| Highlight { id, page, rects: vec![[0.0, y, 1.0, y + 1.0]], color: 0, text: text.into(), note: note.into(), created: 0 };
        let hs = vec![h(3, 9, 50.0, "later", ""), h(1, 4, 90.0, "second on page", "mine"), h(2, 4, 10.0, "first on page", "")];
        let md = markdown("Book", &hs, |p| if p == 4 { "iv".into() } else { (p + 1).to_string() });
        let first = md.find("first on page").unwrap();
        let second = md.find("second on page").unwrap();
        assert!(first < second && second < md.find("later").unwrap());
        assert!(md.contains("## p. iv (PDF page 5)"));
        assert!(md.contains("## p. 10\n"));
        assert!(md.contains("**Note:** mine"));
        assert!(md.starts_with("# Highlights: Book"));
    }

    #[test]
    fn snip_math() {
        assert_eq!(norm_rect((10.0, 20.0), (5.0, 30.0)), [5.0, 20.0, 10.0, 30.0]);
        assert_eq!(clip([0.0, 0.0, 50.0, 50.0], [10.0, 10.0, 100.0, 100.0], 2.0), Some([10.0, 10.0, 50.0, 50.0]));
        assert_eq!(clip([0.0, 0.0, 5.0, 5.0], [10.0, 10.0, 100.0, 100.0], 2.0), None);
        assert_eq!(snip_scale([0.0, 0.0, 100.0, 100.0], 1.0, 2.0, 6.0e6), 2.0);
        assert_eq!(snip_scale([0.0, 0.0, 100.0, 100.0], 3.5, 2.0, 6.0e6), 3.5);
        assert!(snip_scale([0.0, 0.0, 5000.0, 5000.0], 1.0, 2.0, 6.0e6) < 2.0);
        assert!(hit([0.0, 0.0, 10.0, 10.0], (11.0, 5.0), 2.0));
        assert!(!hit([0.0, 0.0, 10.0, 10.0], (13.0, 5.0), 2.0));
    }

    #[test]
    fn marker_only_for_notes() {
        let mut h = Highlight { id: 1, page: 0, rects: vec![[10.0, 40.0, 50.0, 50.0]], color: 0, text: "x".into(), note: String::new(), created: 0 };
        assert_eq!(marker_rect(&h, 600.0), None);
        h.note = "n".into();
        assert_eq!(marker_rect(&h, 600.0), Some([582.0, 40.0, 594.0, 52.0]));
    }
}
