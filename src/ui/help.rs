//! Help overlay generated from the keymap and command registry.

use super::{accent, band, desc, fg, panel_bg, Canvas};
use crate::config::OPTIONS;
use crate::keys::{Keymap, Mode, COMMANDS};
use crate::text::TextRenderer;

pub enum Line {
    Header(String),
    Item { left: String, right: String },
}

pub struct Help {
    lines: Vec<Line>,
    pub query: String,
    pub scroll: usize,
}

pub fn build(km: &Keymap) -> Vec<Line> {
    let mut v = Vec::new();
    for mode in Mode::ALL {
        v.push(Line::Header(format!("Keybindings: {} mode", mode.name())));
        for b in km.bindings.iter().filter(|b| b.mode == mode) {
            v.push(Line::Item { left: b.keys.clone(), right: format!("{}  ({})", b.description, b.text) });
        }
    }
    v.push(Line::Header("Commands (type : to enter)".into()));
    for (name, args, desc) in COMMANDS {
        v.push(Line::Item { left: format!("{name} {args}").trim_end().into(), right: desc.to_string() });
    }
    v.push(Line::Header("Options (:set NAME VALUE)".into()));
    for (name, desc) in OPTIONS {
        v.push(Line::Item { left: name.to_string(), right: desc.to_string() });
    }
    v
}

impl Help {
    pub fn new(km: &Keymap) -> Self {
        Help { lines: build(km), query: String::new(), scroll: 0 }
    }

    /// Lines passing the filter; headers are kept when any of their items match.
    pub fn visible(&self) -> Vec<&Line> {
        if self.query.is_empty() {
            return self.lines.iter().collect();
        }
        let mut out: Vec<&Line> = Vec::new();
        let mut header: Option<&Line> = None;
        for l in &self.lines {
            match l {
                Line::Header(_) => header = Some(l),
                Line::Item { left, right } if super::pick::score(&self.query, &format!("{left} {right}")).is_some() => {
                    if let Some(h) = header.take() {
                        out.push(h);
                    }
                    out.push(l);
                }
                _ => {}
            }
        }
        out
    }

    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.scroll = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.scroll = 0;
    }

    pub fn scroll_by(&mut self, d: i32, rows: usize) {
        let max = self.visible().len().saturating_sub(rows);
        self.scroll = (self.scroll as i32 + d).clamp(0, max as i32) as usize;
    }
}

pub fn draw(cv: &mut Canvas, t: &mut TextRenderer, h: &Help, bottom_reserved: i32) -> usize {
    cv.fill_rect(0, 0, cv.w, cv.h - bottom_reserved, panel_bg());
    let lh = t.line_h;
    let cw = t.width("M").max(1);
    let pad = cw * 2;
    let title = format!("Help  (type to filter, Esc to close)   filter: {}_", h.query);
    t.draw(cv, pad, 6, &title, accent(), cv.w - 2 * pad);
    let top = lh + 14;
    let rows = ((cv.h - bottom_reserved - top - 6) / lh).max(1) as usize;
    let col = (cw * 28).min(cv.w / 2);
    for (i, l) in h.visible().into_iter().skip(h.scroll).take(rows).enumerate() {
        let y = top + i as i32 * lh;
        match l {
            Line::Header(s) => {
                band(cv, y, lh);
                t.draw(cv, pad, y, s, accent(), cv.w - 2 * pad);
            }
            Line::Item { left, right } => {
                t.draw(cv, pad, y, left, fg(), col - pad);
                t.draw(cv, col, y, right, desc(), cv.w - col - pad);
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders the overlay in both looks to `target/help-*.ppm` for eyeballing.
    #[test]
    fn help_renders() {
        let Ok(mut t) = TextRenderer::load("monospace", 13.0) else { return };
        let (w, h) = (900, 520);
        for (name, th) in [("default", crate::ui::Theme::default()), ("nerv", crate::ui::Theme::nerv(&crate::palette::Palette::default()))] {
            let mut buf = vec![0xff40_4040u32; (w * h) as usize];
            crate::ui::set_theme(th);
            let mut hp = Help::new(&Keymap::with_defaults());
            hp.scroll = hp.visible().len().saturating_sub(40);
            draw(&mut Canvas { buf: &mut buf, w, h }, &mut t, &hp, 20);
            crate::ui::set_theme(crate::ui::Theme::default());
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            out.extend(buf.iter().flat_map(|px| [(px >> 16) as u8, (px >> 8) as u8, *px as u8]));
            let _ = std::fs::write(format!("target/help-{name}.ppm"), out);
        }
    }

    #[test]
    fn generated_and_filtered() {
        let mut km = Keymap::with_defaults();
        km.bind(Mode::Normal, "X", "goto 9").unwrap();
        let mut h = Help::new(&km);
        assert!(h.visible().iter().any(|l| matches!(l, Line::Item { left, .. } if left == "X")));
        for c in "cache".chars() {
            h.type_char(c);
        }
        let v = h.visible();
        assert!(v.len() < build(&km).len());
        assert!(v.iter().any(|l| matches!(l, Line::Item { left, .. } if left == "cache-mb")));
        h.query = "zzzzqq".into();
        assert!(h.visible().is_empty());
    }
}
