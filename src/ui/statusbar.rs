//! Bottom bar: filename, page, zoom, mode, pending keys, messages.

use super::{bar_bg, dim, err as err_color, fg, Canvas};
use crate::text::TextRenderer;

pub struct Status<'a> {
    pub name: &'a str,
    pub message: Option<(&'a str, bool)>,
    pub page: usize,
    /// Printed page label when the book has a page map.
    pub printed: Option<&'a str>,
    /// Active look profile when it is not `normal`.
    pub profile: Option<&'a str>,
    pub total: usize,
    pub zoom_pct: u32,
    pub mode: &'a str,
    pub pending: &'a str,
    /// Search position `(i, n)`, 1-based.
    /// Background job text, e.g. `analysing 120/552`.
    pub job: Option<&'a str>,
    pub search: Option<(usize, usize, bool)>,
}

pub fn height(t: &TextRenderer) -> i32 {
    t.line_h + 6
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, s: &Status) {
    let h = height(t);
    let y = cv.h - h;
    cv.fill_rect(0, y, cv.w, h, bar_bg());
    let ty = y + 3;
    let cw = t.width("M").max(1);
    let mut extra = String::new();
    if !s.pending.is_empty() {
        extra.push_str(&format!("{}  ", s.pending));
    }
    if let Some((i, n, done)) = s.search {
        extra.push_str(&format!("[{i}/{n}{}]  ", if done { "" } else { "+" }));
    }
    if let Some(j) = s.job {
        extra.push_str(&format!("{j}  "));
    }
    if let Some(p) = s.profile {
        extra.push_str(&format!("[{p}]  "));
    }
    let mut essential = String::new();
    if let Some(p) = s.printed {
        essential.push_str(&format!("p.{p} "));
    }
    essential.push_str(&format!("[{}/{}]  {}%  {}", s.page, s.total, s.zoom_pct, s.mode));
    // the page/zoom/mode part is always kept; the extras go first when the bar is narrow
    let full = format!("{extra}{essential}");
    let right = if t.width(&full) * 10 <= cv.w * 6 { full } else { essential };
    let rw = t.width(&right);
    t.draw(cv, cv.w - rw - cw, ty, &right, dim(), rw + cw);
    let (left, color) = match s.message {
        Some((m, err)) => (m, if err { err_color() } else { fg() }),
        None => (s.name, fg()),
    };
    let room = (cv.w - rw - 3 * cw).max(0);
    let left = ellipsize_middle(t, left, room);
    t.draw(cv, cw, ty, &left, color, room);
}

/// `very-long-name.pdf` -> `very-lo...ame.pdf` so that it fits `room` pixels (the tail keeps the extension).
pub fn ellipsize_middle(t: &mut TextRenderer, s: &str, room: i32) -> String {
    if t.width(s) <= room {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut head, mut tail) = (chars.len() / 2, chars.len() / 2);
    // shrink alternately until head + "..." + tail fits
    while head + tail > 0 {
        let cand: String = chars[..head].iter().chain(['.', '.', '.'].iter()).chain(chars[chars.len() - tail..].iter()).collect();
        if t.width(&cand) <= room {
            return cand;
        }
        if head >= tail {
            head -= 1;
        } else {
            tail -= 1;
        }
    }
    "...".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Palette;
    use crate::ui::{set_theme, Theme};

    /// Renders the bar under the NERV theme (and an error message) onto a grey page strip; writes
    /// `target/nerv-statusbar.ppm` for eyeballing and checks the bar really is black with amber text.
    #[test]
    fn statusbar_with_nerv_palette() {
        let Ok(mut t) = TextRenderer::load("monospace", 13.0) else { return };
        let (w, h) = (640, 2 * height(&t) + 20);
        let mut buf = vec![0xff40_4040u32; (w * h) as usize];
        set_theme(Theme::nerv(&Palette::default()));
        let st = |message| Status { name: "report-final.pdf", message, page: 3, printed: Some("xii"), profile: Some("nerv"), total: 120, zoom_pct: 100, mode: "NORMAL", pending: "", job: None, search: Some((2, 9, true)) };
        {
            let mut cv = Canvas { buf: &mut buf, w, h };
            cv.h = h - height(&t) - 20;
            draw(&mut cv, &mut t, &st(None));
        }
        {
            let mut cv = Canvas { buf: &mut buf, w, h };
            draw(&mut cv, &mut t, &st(Some(("E: no such file", true))));
        }
        let bar_px = buf[(w * (h - 2) + 2) as usize];
        set_theme(Theme::default());
        // translucent black over 0x40 grey: dark, no hue
        let (r, g, b) = ((bar_px >> 16) & 0xff, (bar_px >> 8) & 0xff, bar_px & 0xff);
        assert!(r == g && g == b && r < 0x40);
        let amber = buf.iter().any(|&p| (p >> 16) & 0xff > 0xe0 && (p >> 8) & 0xff > 0x60 && p & 0xff < 0x50);
        assert!(amber);
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        out.extend(buf.iter().flat_map(|px| [(px >> 16) as u8, (px >> 8) as u8, *px as u8]));
        let _ = std::fs::write("target/nerv-statusbar.ppm", out);
    }
}
