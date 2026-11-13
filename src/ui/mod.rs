//! Drawing primitives shared by the UI panels.

pub mod help;
pub mod index;
pub mod input;
pub mod pick;
pub mod pin;
pub mod plist;
pub mod popup;
pub mod select;
pub mod statusbar;

/// Premultiplied 0xAARRGGBB drawing surface over a softbuffer slice.
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: i32,
    pub h: i32,
}

#[inline]
pub fn over(s: u32, d: u32) -> u32 {
    let sa = s >> 24;
    match sa {
        255 => s,
        0 => d,
        _ => {
            let inv = 255 - sa;
            let ch = |sh: u32| {
                let dc = (d >> sh) & 0xff;
                let v = ((s >> sh) & 0xff) + (dc * inv + 127) / 255;
                v.min(255) << sh
            };
            ch(24) | ch(16) | ch(8) | ch(0)
        }
    }
}

/// Premultiplied colour from straight rgb and alpha.
pub fn pm(rgb: [u8; 3], a: u8) -> u32 {
    let m = |v: u8| (v as u32 * a as u32 + 127) / 255;
    ((a as u32) << 24) | (m(rgb[0]) << 16) | (m(rgb[1]) << 8) | m(rgb[2])
}

impl Canvas<'_> {
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let (x0, x1) = (x.max(0), (x + w).min(self.w));
        let (y0, y1) = (y.max(0), (y + h).min(self.h));
        if x0 >= x1 {
            return;
        }
        for yy in y0..y1 {
            let row = &mut self.buf[(yy * self.w + x0) as usize..(yy * self.w + x1) as usize];
            for d in row {
                *d = over(color, *d);
            }
        }
    }

    pub fn blend_px(&mut self, x: i32, y: i32, rgb: [u8; 3], a: u8) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) as usize;
        self.buf[i] = over(pm(rgb, a), self.buf[i]);
    }

    /// Source-over blit of premultiplied pixels (`tw` wide), clipped to the canvas and `clip_h` rows.
    pub fn blit(&mut self, tw: i64, th: i64, src: &[u32], dx: i64, dy: i64, clip_h: i64) {
        let (bw, bh) = (self.w as i64, (self.h as i64).min(clip_h));
        let (cx0, cx1) = (dx.max(0), (dx + tw).min(bw));
        let (cy0, cy1) = (dy.max(0), (dy + th).min(bh));
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        for y in cy0..cy1 {
            let s_row = &src[((y - dy) * tw + (cx0 - dx)) as usize..][..(cx1 - cx0) as usize];
            let d_row = &mut self.buf[(y * bw + cx0) as usize..][..(cx1 - cx0) as usize];
            for (d, &s) in d_row.iter_mut().zip(s_row) {
                *d = over(s, *d);
            }
        }
    }
}

/// Chrome colours: the built-in look, or the NERV palette when `nerv-chrome` is on and the file exists.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    pub fg: [u8; 3],
    pub dim: [u8; 3],
    pub err: [u8; 3],
    /// Titles and prompts (text).
    pub accent: [u8; 3],
    /// Structural lines: popup border, scrollbar thumb, pin focus, selection outline.
    pub line: [u8; 3],
    pub panel: [u8; 3],
    pub bar: [u8; 3],
    pub input: [u8; 3],
    pub popup: [u8; 4],
    pub hint_bg: [u8; 3],
    pub hint_fg: [u8; 3],
    /// Selection rows are outlined instead of filled.
    pub nerv: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            fg: [0xcd, 0xd6, 0xf4],
            dim: [0x93, 0x99, 0xb2],
            err: [0xf3, 0x8b, 0xa8],
            accent: [0x89, 0xb4, 0xfa],
            line: [0x89, 0xb4, 0xfa],
            panel: [0x1d, 0x1d, 0x2d],
            bar: [0x11, 0x11, 0x1b],
            input: [0x1e, 0x1e, 0x2e],
            popup: [0x1e, 0x1e, 0x2e, 0xf2],
            hint_bg: [0xf9, 0xe2, 0xaf],
            hint_fg: [0x11, 0x11, 0x1b],
            nerv: false,
        }
    }
}

impl Theme {
    /// Default look with the palette's keys laid over it; missing keys keep the NERV defaults.
    pub fn nerv(p: &crate::palette::Palette) -> Theme {
        let field = p.field.unwrap_or([0, 0, 0]);
        let signal = p.signal.unwrap_or(crate::palette::SIGNAL);
        let grid = p.grid.unwrap_or([0x4c, 0xe8, 0xa8]);
        Theme {
            fg: signal,
            dim: p.dim.unwrap_or([0xa8, 0x52, 0x1a]),
            err: p.alarm.unwrap_or([0xff, 0x20, 0x30]),
            accent: signal,
            line: grid,
            panel: field,
            bar: field,
            input: field,
            popup: [field[0], field[1], field[2], 0xf2],
            hint_bg: signal,
            hint_fg: field,
            nerv: true,
        }
    }

    /// `nerv-chrome` on and a palette file present gives the NERV look; otherwise exactly the built-in one.
    pub fn resolve(nerv_chrome: bool, pal: Option<&crate::palette::Palette>) -> Theme {
        match pal {
            Some(p) if nerv_chrome => Theme::nerv(p),
            _ => Theme::default(),
        }
    }
}

static THEME: std::sync::RwLock<Theme> = std::sync::RwLock::new(Theme {
    fg: [0xcd, 0xd6, 0xf4],
    dim: [0x93, 0x99, 0xb2],
    err: [0xf3, 0x8b, 0xa8],
    accent: [0x89, 0xb4, 0xfa],
    line: [0x89, 0xb4, 0xfa],
    panel: [0x1d, 0x1d, 0x2d],
    bar: [0x11, 0x11, 0x1b],
    input: [0x1e, 0x1e, 0x2e],
    popup: [0x1e, 0x1e, 0x2e, 0xf2],
    hint_bg: [0xf9, 0xe2, 0xaf],
    hint_fg: [0x11, 0x11, 0x1b],
    nerv: false,
});

pub fn theme() -> Theme {
    *THEME.read().unwrap_or_else(|e| e.into_inner())
}

pub fn set_theme(t: Theme) {
    *THEME.write().unwrap_or_else(|e| e.into_inner()) = t;
}

pub fn fg() -> [u8; 3] {
    theme().fg
}
pub fn dim() -> [u8; 3] {
    theme().dim
}
pub fn err() -> [u8; 3] {
    theme().err
}
pub fn accent() -> [u8; 3] {
    theme().accent
}

pub fn panel_bg() -> u32 {
    pm(theme().panel, 0xff)
}

/// Statusbar and message strip background (translucent in both looks).
pub fn bar_bg() -> u32 {
    pm(theme().bar, 0xcc)
}

pub fn input_bg() -> u32 {
    pm(theme().input, 0xff)
}

/// Selected list row: a filled band, or a one-pixel outline in the NERV look.
pub fn sel_band(cv: &mut Canvas, y: i32, lh: i32) {
    let t = theme();
    if t.nerv {
        outline(cv, 0, y, cv.w, lh, pm(t.line, 0xff));
    } else {
        cv.fill_rect(0, y, cv.w, lh, pm([0x45, 0x47, 0x5a], 0xe0));
    }
}

/// Section header row: a filled band, or a one-pixel rule under it in the NERV look.
pub fn band(cv: &mut Canvas, y: i32, lh: i32) {
    let t = theme();
    if t.nerv {
        cv.fill_rect(0, y + lh - 1, cv.w, 1, pm(t.line, 0xff));
    } else {
        cv.fill_rect(0, y, cv.w, lh, pm([0x45, 0x47, 0x5a], 0x90));
    }
}

pub fn outline(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, c: u32) {
    for (bx, by, bw, bh) in [(x, y, w, 1), (x, y + h - 1, w, 1), (x, y, 1, h), (x + w - 1, y, 1, h)] {
        cv.fill_rect(bx, by, bw, bh, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_blends_premultiplied() {
        assert_eq!(over(0, 0x1234_5678), 0x1234_5678);
        assert_eq!(over(0xff01_0203, 0x1234_5678), 0xff01_0203);
        assert_eq!(over(0x8080_8080, 0xff00_0000) >> 24, 0xff);
        assert_eq!(over(0x8080_8080, 0xff00_0000) & 0xff, 0x80);
    }

    #[test]
    fn blit_clips() {
        let mut buf = vec![0u32; 16];
        let mut cv = Canvas { buf: &mut buf, w: 4, h: 4 };
        let src = vec![0xffff_ffffu32; 9];
        cv.blit(3, 3, &src, -1, 2, 4);
        assert_eq!(buf.iter().filter(|&&p| p != 0).count(), 4);
        let mut cv = Canvas { buf: &mut buf, w: 4, h: 4 };
        cv.blit(3, 3, &src, 10, 10, 4);
    }

}
