//! Alpha-preserving recolor. Pages are rendered on a transparent background, so the
//! "light" colour is simply transparency (the window background shows through) and
//! dark ink is mapped to `dark` with coverage derived from luminance and alpha.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recolor {
    pub enabled: bool,
    pub dark: [u8; 3],
    /// Keep the hue of strongly coloured ink instead of forcing it to `dark`.
    pub keephue: bool,
    /// Paper colour; alpha 0 leaves the page transparent so the window background shows.
    pub light: [u8; 4],
}

impl Default for Recolor {
    fn default() -> Self {
        Recolor { enabled: true, dark: [0xff, 0xff, 0xff], keephue: false, light: [0; 4] }
    }
}

const SAT_THRESHOLD: u32 = 48;

fn mul255(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

/// Pack straight RGBA channels into premultiplied 0xAARRGGBB.
fn pack(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | (mul255(r, a) << 16) | (mul255(g, a) << 8) | mul255(b, a)
}

impl Recolor {
    pub fn with_enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Convert one premultiplied RGBA pixel (as produced by MuPDF) to premultiplied 0xAARRGGBB.
    pub fn pixel(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        if a == 0 {
            return 0;
        }
        let (r, g, b, a) = (r as u32, g as u32, b as u32, a as u32);
        if !self.enabled {
            return (a << 24) | (r << 16) | (g << 8) | b;
        }
        let un = |c: u32| ((c * 255 + a / 2) / a).min(255);
        let (ur, ug, ub) = (un(r), un(g), un(b));
        let lum = (54 * ur + 183 * ug + 19 * ub) >> 8;
        let ink = mul255(a, 255 - lum);
        if self.keephue {
            let mx = ur.max(ug).max(ub);
            let sat = if mx == 0 { 0 } else { (mx - ur.min(ug).min(ub)) * 255 / mx };
            if sat > SAT_THRESHOLD {
                let alpha = mul255(a, (255 - lum).max(sat));
                return pack(ur, ug, ub, alpha);
            }
        }
        let [dr, dg, db] = self.dark.map(|c| c as u32);
        let la = self.light[3] as u32;
        if la == 0 {
            return pack(dr, dg, db, ink);
        }
        // blend ink over the (partly transparent) paper colour, premultiplied
        let paper = mul255(a - ink, la);
        let ch = |d: u32, l: u8| mul255(d, ink) + mul255(l as u32, paper);
        let alpha = ink + paper;
        (alpha << 24) | (ch(dr, self.light[0]).min(alpha) << 16) | (ch(dg, self.light[1]).min(alpha) << 8) | ch(db, self.light[2]).min(alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_stays_transparent() {
        assert_eq!(Recolor::default().pixel(0, 0, 0, 0), 0);
    }

    #[test]
    fn black_ink_becomes_opaque_dark_color() {
        assert_eq!(Recolor::default().pixel(0, 0, 0, 255), 0xffff_ffff);
        let rc = Recolor { dark: [0x10, 0x20, 0x30], ..Recolor::default() };
        assert_eq!(rc.pixel(0, 0, 0, 255), 0xff10_2030);
    }

    #[test]
    fn opaque_paper_color_fills_page() {
        let rc = Recolor { light: [0x1e, 0x1e, 0x2e, 0xff], ..Recolor::default() };
        assert_eq!(rc.pixel(255, 255, 255, 255), 0xff1e_1e2e);
        assert_eq!(rc.pixel(0, 0, 0, 255), 0xffff_ffff);
    }

    #[test]
    fn white_paper_vanishes() {
        assert_eq!(Recolor::default().pixel(255, 255, 255, 255), 0);
    }

    #[test]
    fn opaque_light_gray_is_nearly_transparent() {
        let a = Recolor::default().pixel(0xe0, 0xe0, 0xe0, 255) >> 24;
        assert!(a <= 32, "alpha {a}");
    }

    #[test]
    fn antialiasing_is_preserved() {
        // half-covered black pixel: premultiplied (0,0,0,128) -> half-alpha white
        let p = Recolor::default().pixel(0, 0, 0, 128);
        assert_eq!(p >> 24, 128);
        assert_eq!(p & 0xff, 128); // premultiplied white
        // 50% grey fully opaque -> about half ink
        let g = Recolor::default().pixel(128, 128, 128, 255) >> 24;
        assert!((126..=128).contains(&g));
    }

    #[test]
    fn keephue_keeps_colored_ink() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        let p = rc.pixel(200, 0, 0, 255); // strong red
        assert_eq!(p >> 24, 255);
        assert_eq!((p >> 16) & 0xff, 200);
        assert_eq!(p & 0xffff, 0);
        // neutral ink still uses the dark colour
        assert_eq!(rc.pixel(0, 0, 0, 255), 0xffff_ffff);
    }

    #[test]
    fn disabled_passes_through() {
        let rc = Recolor { enabled: false, ..Recolor::default() };
        assert_eq!(rc.pixel(10, 20, 30, 40), 0x280a_141e);
    }
}
