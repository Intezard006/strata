//! Drawing primitives shared by the UI panels.

pub mod help;
pub mod index;
pub mod input;
pub mod pick;
pub mod plist;
pub mod popup;
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
    pub fn blit(&mut self, tw: i64, th: i64, src: &[u32], dx: i64, dy: i64, clip_h: i64, under: bool) {
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
                *d = over(s, if under { 0xffff_ffff } else { *d });
            }
        }
    }
}

pub const FG: [u8; 3] = [0xcd, 0xd6, 0xf4];
pub const DIM: [u8; 3] = [0x93, 0x99, 0xb2];
pub const ERR: [u8; 3] = [0xf3, 0x8b, 0xa8];
pub const ACCENT: [u8; 3] = [0x89, 0xb4, 0xfa];

pub fn panel_bg() -> u32 {
    pm([0x11, 0x11, 0x1b], 0xf0)
}

pub fn sel_bg() -> u32 {
    pm([0x45, 0x47, 0x5a], 0xe0)
}

/// Fuzzy subsequence match, case-insensitive; empty needle matches everything.
pub fn fuzzy(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars().flat_map(|c| c.to_lowercase());
    needle.chars().flat_map(|c| c.to_lowercase()).filter(|c| !c.is_whitespace()).all(|n| it.any(|h| h == n))
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
        cv.blit(3, 3, &src, -1, 2, 4, false);
        assert_eq!(buf.iter().filter(|&&p| p != 0).count(), 4);
        let mut cv = Canvas { buf: &mut buf, w: 4, h: 4 };
        cv.blit(3, 3, &src, 10, 10, 4, false);
    }

    #[test]
    fn fuzzy_filter() {
        assert!(fuzzy("", "anything"));
        assert!(fuzzy("scdn", "scroll-down"));
        assert!(fuzzy("ZOOM", "Zoom in"));
        assert!(fuzzy("zo in", "zoom in"));
        assert!(!fuzzy("zx", "zoom"));
        assert!(!fuzzy("nz", "zn"));
    }
}
