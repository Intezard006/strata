//! Glyph rasterisation (fontdue) with a byte-capped glyph cache.

use std::collections::HashMap;
use std::process::Command;

use crate::ui::Canvas;

const GLYPH_CACHE_BYTES: usize = 1024 * 1024;

struct Glyph {
    w: usize,
    h: usize,
    xmin: i32,
    ymin: i32,
    advance: i32,
    cov: Vec<u8>,
}

pub struct TextRenderer {
    font: fontdue::Font,
    px: f32,
    cache: HashMap<char, Glyph>,
    bytes: usize,
    pub ascent: i32,
    pub line_h: i32,
}

/// Resolve a font family to a file via fontconfig.
fn font_file(family: &str) -> Option<Vec<u8>> {
    let out = Command::new("fc-match").args(["-f", "%{file}", family]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    std::fs::read(path.trim()).ok()
}

impl TextRenderer {
    pub fn load(family: &str, px: f32) -> Result<Self, String> {
        let data = font_file(family).ok_or_else(|| format!("font not found: {family}"))?;
        let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).map_err(|e| e.to_string())?;
        let mut t = TextRenderer { font, px: 0.0, cache: HashMap::new(), bytes: 0, ascent: 0, line_h: 0 };
        t.set_px(px);
        Ok(t)
    }

    pub fn set_px(&mut self, px: f32) {
        let px = px.max(4.0);
        if (px - self.px).abs() < 0.01 {
            return;
        }
        self.px = px;
        self.cache.clear();
        self.bytes = 0;
        let m = self.font.horizontal_line_metrics(px);
        self.ascent = m.map_or(px as i32, |m| m.ascent.round() as i32);
        self.line_h = m.map_or(px as i32 + 4, |m| m.new_line_size.ceil() as i32);
    }

    fn glyph(&mut self, c: char) -> &Glyph {
        if !self.cache.contains_key(&c) {
            if self.bytes > GLYPH_CACHE_BYTES {
                self.cache.clear();
                self.bytes = 0;
            }
            let (m, cov) = self.font.rasterize(c, self.px);
            self.bytes += cov.len() + std::mem::size_of::<Glyph>();
            self.cache.insert(
                c,
                Glyph { w: m.width, h: m.height, xmin: m.xmin, ymin: m.ymin, advance: m.advance_width.round() as i32, cov },
            );
        }
        &self.cache[&c]
    }

    pub fn width(&mut self, s: &str) -> i32 {
        s.chars().map(|c| self.glyph(c).advance).sum()
    }

    /// Draw `s` with its top-left at (x, y), clipped to `max_w` pixels. Returns the end x.
    pub fn draw(&mut self, cv: &mut Canvas, x: i32, y: i32, s: &str, rgb: [u8; 3], max_w: i32) -> i32 {
        let base = y + self.ascent;
        let mut pen = x;
        for c in s.chars() {
            let g = self.glyph(c);
            if pen + g.advance > x + max_w {
                break;
            }
            let (gx, gy) = (pen + g.xmin, base - g.ymin - g.h as i32);
            for row in 0..g.h {
                for col in 0..g.w {
                    let a = g.cov[row * g.w + col];
                    if a != 0 {
                        cv.blend_px(gx + col as i32, gy + row as i32, rgb, a);
                    }
                }
            }
            pen += g.advance;
        }
        pen
    }
}
