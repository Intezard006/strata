//! Alpha-preserving recolor. Pages are rendered on a transparent background, so the
//! "light" colour is simply transparency (the window background shows through) and
//! dark ink is mapped to `dark` with coverage derived from luminance and alpha.

use crate::imagemap::Ovr;

/// What happens to embedded images (photos, plots) when recoloring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMode {
    /// Original colours, untouched.
    Keep,
    /// Original colours at reduced brightness (`Recolor::image_dim`).
    Dim,
    /// Recolor them like everything else.
    Recolor,
    /// Line art and diagrams on a white background (ChemDraw schemes, logos) are recolored with their
    /// hues kept; photos and colourful renders keep their colours.
    Smart,
}

/// Near-white: luminance above this (0..1) in a neutral pixel.
const WHITE_L: f32 = 0.88;
/// Neutral: chroma (max-min, 0..255) below this. An absolute measure: HSV-style saturation
/// (max-min)/max explodes for near-black pixels, where JPEG noise would look "coloured".
const NEUTRAL_CHROMA: f32 = 30.0;

/// Whether sampled RGB pixels (a thumbnail of an image placement) look like line art: a dominant
/// near-white background with sparse ink, hues allowed (coloured strokes), or an almost greyscale
/// image that is at least half white. Tolerant of JPEG noise through the thresholds.
#[cfg(test)]
pub fn is_line_art(px: impl Iterator<Item = [u8; 3]>) -> bool {
    line_stats(px).is_some_and(|s| s.is_line())
}

/// Summary of a thumbnail: shares of near-white and neutral pixels, of the 6 busiest colour bins, and the
/// share of pixels that are neither white nor in those bins.
#[derive(Clone, Copy, Debug)]
pub struct LineStats {
    pub white: f32,
    pub neutral: f32,
    pub flat: f32,
    /// Share of the single most common colour bin, when that bin is bright (a light, off-white background).
    pub light_bg: f32,
}

impl LineStats {
    pub fn is_line(&self) -> bool {
        let (w, ne, flat) = (self.white, self.neutral, self.flat);
        w >= 0.65 || (ne >= 0.8 && w >= 0.5) || (w >= 0.35 && flat >= 0.85) || (self.light_bg >= 0.35 && flat >= 0.9)
    }
}

pub fn line_stats(px: impl Iterator<Item = [u8; 3]>) -> Option<LineStats> {
    let (mut n, mut white, mut neutral) = (0u32, 0u32, 0u32);
    let mut bins = [0u32; 512];
    for [r, g, b] in px {
        n += 1;
        bins[((r >> 5) as usize) << 6 | ((g >> 5) as usize) << 3 | (b >> 5) as usize] += 1;
        let (mx, mn) = (r.max(g).max(b) as f32, r.min(g).min(b) as f32);
        let chroma = mx - mn;
        let lum = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
        if chroma < NEUTRAL_CHROMA {
            neutral += 1;
            if lum > WHITE_L {
                white += 1;
            }
        } else if mn / 255.0 > WHITE_L {
            white += 1; // faintly tinted paper
        }
    }
    if n == 0 {
        return None;
    }
    let (w, ne) = (white as f32 / n as f32, neutral as f32 / n as f32);
    // flat-colour graphics (logos): a mostly white image whose pixels sit in a handful of colours
    // the busiest bin is a light background when all three channels are in the top three eighths (>= 192)
    let (top_i, &top_n) = bins.iter().enumerate().max_by_key(|(_, &c)| c).unwrap();
    let light_bg = if (top_i >> 6) >= 6 && ((top_i >> 3) & 7) >= 6 && (top_i & 7) >= 6 { top_n as f32 / n as f32 } else { 0.0 };
    bins.sort_unstable_by(|a, b| b.cmp(a));
    let flat = bins[..6].iter().sum::<u32>() as f32 / n as f32;
    Some(LineStats { white: w, neutral: ne, flat, light_bg })
}

/// Images covering more than this share of the page are scans and are always recolored.
pub const SCAN_SHARE: f32 = 0.7;
/// Mean luminance (0..1) below which an image counts as dark.
pub const DARK_MEAN: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegionKind {
    /// Pixels keep (or dim) their original colours.
    Keep,
    /// Line art: recolored like vector content, with the hue of coloured strokes kept.
    LineArt,
    /// Recolor the inverted pixels (a dark image: dark stays background, light becomes ink).
    Invert,
    /// A scan (or an image recolored like text): normal recolor plus the contrast curve.
    Ink,
}

/// A pixel rectangle inside a tile with its own treatment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub kind: RegionKind,
}

/// An image placement on a page, in points from the page origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImgRect {
    pub rect: [f32; 4],
    /// Mean luminance below `DARK_MEAN` (only measured when the invert option is on).
    pub dark: bool,
    /// Classified as line art / a diagram (only measured in `ImageMode::Smart`).
    pub line: bool,
    /// Stable image id (see `imagemap`), for overrides.
    pub id: u64,
    /// The user's decision for this image.
    pub ovr: crate::imagemap::Ovr,
    /// Per-cell classes when the crop is mixed (then `line` is false).
    pub grid: crate::imagemap::Grid,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recolor {
    pub images: ImageMode,
    /// Brightness factor of `ImageMode::Dim`.
    pub image_dim: f32,
    pub invert_dark: bool,
    pub enabled: bool,
    pub dark: [u8; 3],
    /// Keep the hue of strongly coloured ink instead of forcing it to `dark`.
    pub keephue: bool,
    /// Paper colour; alpha 0 leaves the page transparent so the window background shows.
    pub light: [u8; 4],
    /// Ink contrast curve for scans and line art (1 = off): partial ink is pushed towards solid.
    pub contrast: f32,
    /// Apply the contrast curve to vector content (text, drawings) too.
    pub contrast_vector: bool,
}

impl Default for Recolor {
    fn default() -> Self {
        Recolor { enabled: true, dark: [0xff, 0xff, 0xff], keephue: false, light: [0; 4], images: ImageMode::Smart, image_dim: 0.85, invert_dark: false, contrast: 1.0, contrast_vector: false }
    }
}

/// Chroma (max-min) where a pixel starts / finishes counting as coloured ink; between the two the
/// neutral and the hue-keeping looks are blended, so anti-aliased edges leave no fringe and JPEG chroma
/// noise (up to ~30 on dark pixels) is ignored.
const CHROMA_LO: u32 = 45;
const CHROMA_HI: u32 = 105;
/// Lightness (0..255) a kept hue is raised to at least, so it stays readable on a dark page.
const HUE_MIN_L: f32 = 0.55;

/// Invert HSL lightness keeping hue and saturation: dark blue becomes light blue.
fn invert_lightness(r: u32, g: u32, b: u32) -> [u32; 3] {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (mx, mn) = (rf.max(gf).max(bf), rf.min(gf).min(bf));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d <= 0.0 {
        let v = ((1.0 - l).max(HUE_MIN_L) * 255.0).round() as u32;
        return [v; 3];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-4);
    let h = if mx == rf { ((gf - bf) / d).rem_euclid(6.0) } else if mx == gf { (bf - rf) / d + 2.0 } else { (rf - gf) / d + 4.0 };
    let l2 = (1.0 - l).max(HUE_MIN_L);
    let c = (1.0 - (2.0 * l2 - 1.0).abs()) * s.min(1.0);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l2 - c / 2.0;
    let rgb = [r1, g1, b1].map(|v| ((v + m) * 255.0).clamp(0.0, 255.0));
    // perceived brightness (blue is dark at equal HSL lightness): lift it to a readable floor
    let lum = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let floor = 100.0;
    let t = if lum < floor { (floor - lum) / (255.0 - lum) } else { 0.0 };
    rgb.map(|v| (v + (255.0 - v) * t).round() as u32)
}

fn mul255(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

/// Pack straight RGBA channels into premultiplied 0xAARRGGBB.
fn pack(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | (mul255(r, a) << 16) | (mul255(g, a) << 8) | mul255(b, a)
}

/// Ink contrast curve on 0..255 coverage: `x(1+k)/(1+kx)` with `k = contrast - 1`. It fixes 0 and 255 and is
/// monotonic, so anti-aliasing ramps stay ramps, but mid greys of blurry scans are lifted towards solid.
pub fn ink_curve(x: u32, contrast: f32) -> u32 {
    let kq = ((contrast - 1.0).max(0.0) * 256.0) as u32;
    if kq == 0 || x == 0 || x >= 255 {
        return x.min(255);
    }
    (x * (256 + kq) / (256 + kq * x / 255)).min(255)
}

impl Recolor {
    /// Whether image placements matter (and so must be collected) for this recolor.
    pub fn wants_images(&self) -> bool {
        self.enabled && (self.images != ImageMode::Recolor || self.invert_dark)
    }

    /// Whether image placements must be classified (line art vs photo).
    pub fn wants_class(&self) -> bool {
        self.enabled && self.images == ImageMode::Smart
    }

    /// Regions (tile pixel coordinates) that are not recolored normally. `origin` is the tile's top-left
    /// in page pixels, `scale` pixels per point, `page_area` in square points.
    pub fn regions(&self, imgs: &[ImgRect], page_area: f32, scale: f32, origin: (i32, i32), size: (i32, i32)) -> Vec<Region> {
        let mut out = Vec::new();
        // a page-sized image makes this a scanned page: its small crops (formulas, rules) are text too
        let scan_page = imgs.iter().any(|im| (im.rect[2] - im.rect[0]) * (im.rect[3] - im.rect[1]) > SCAN_SHARE * page_area);
        let region = |out: &mut Vec<Region>, r: [f32; 4], kind| {
            let reg = Region {
                x0: ((r[0] * scale).floor() as i32 - origin.0).max(0),
                y0: ((r[1] * scale).floor() as i32 - origin.1).max(0),
                x1: ((r[2] * scale).ceil() as i32 - origin.0).min(size.0),
                y1: ((r[3] * scale).ceil() as i32 - origin.1).min(size.1),
                kind,
            };
            if reg.x0 < reg.x1 && reg.y0 < reg.y1 {
                out.push(reg);
            }
        };
        for im in imgs {
            let r = im.rect;
            let area = (r[2] - r[0]) * (r[3] - r[1]);
            let scan = scan_page || area > SCAN_SHARE * page_area;
            let g = im.grid;
            if g.is_some() && im.ovr == Ovr::Auto && !scan && self.images == ImageMode::Smart {
                // a mixed crop: each cell of the grid is line art or keeps its colours
                let (cw, ch) = ((r[2] - r[0]) / g.nx as f32, (r[3] - r[1]) / g.ny as f32);
                for cy in 0..g.ny {
                    for cx in 0..g.nx {
                        let c = [r[0] + cw * cx as f32, r[1] + ch * cy as f32, r[0] + cw * (cx + 1) as f32, r[1] + ch * (cy + 1) as f32];
                        region(&mut out, c, if g.line(cx, cy) { RegionKind::LineArt } else { RegionKind::Keep });
                    }
                }
                continue;
            }
            let kind = if im.ovr == Ovr::Keep {
                RegionKind::Keep
            } else if im.ovr == Ovr::Recolor {
                RegionKind::LineArt
            } else if im.dark && self.invert_dark && scan {
                RegionKind::Invert
            } else if scan || self.images == ImageMode::Recolor {
                if self.contrast > 1.0 && !self.contrast_vector { RegionKind::Ink } else { continue }
            } else if self.images == ImageMode::Smart && im.line {
                RegionKind::LineArt
            } else {
                RegionKind::Keep
            };
            region(&mut out, r, kind);
        }
        out
    }

    /// Convert one row of premultiplied RGBA bytes, applying `regions` (tile pixel coordinates) at row `y`.
    pub fn convert_row(&self, line: &[u8], y: i32, regions: &[Region], out: &mut Vec<u32>) {
        if !regions.iter().any(|r| y >= r.y0 && y < r.y1) {
            out.extend(line.chunks_exact(4).map(|p| self.pixel(p[0], p[1], p[2], p[3])));
            return;
        }
        for (x, p) in line.chunks_exact(4).enumerate() {
            let x = x as i32;
            let reg = regions.iter().find(|r| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1);
            out.push(match reg.map(|r| r.kind) {
                None => self.pixel(p[0], p[1], p[2], p[3]),
                Some(RegionKind::Keep) => self.keep_pixel(p[0], p[1], p[2], p[3]),
                Some(RegionKind::LineArt) => self.pixel_hue(p[0], p[1], p[2], p[3]),
                Some(RegionKind::Invert) => self.inverted_pixel(p[0], p[1], p[2], p[3]),
                Some(RegionKind::Ink) => self.pixel_scan(p[0], p[1], p[2], p[3]),
            });
        }
    }

    /// Original colours (premultiplied in, premultiplied 0xAARRGGBB out), dimmed in `Dim` mode.
    pub fn keep_pixel(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        let f = if self.images == ImageMode::Dim { (self.image_dim.clamp(0.0, 1.0) * 256.0) as u32 } else { 256 };
        let s = |c: u8| (c as u32 * f) >> 8;
        ((a as u32) << 24) | (s(r) << 16) | (s(g) << 8) | s(b)
    }

    /// Like `pixel`, but strongly coloured ink keeps its hue whatever the `keephue` option says.
    pub fn pixel_hue(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        Recolor { keephue: true, ..*self }.pixel_c(r, g, b, a, self.contrast)
    }

    pub fn inverted_pixel(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        if a == 0 {
            return 0;
        }
        let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        self.pixel_c(255 - un(r), 255 - un(g), 255 - un(b), a, self.contrast)
    }
    #[cfg(test)]
    fn pixel_neutral_for_test(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        Recolor { keephue: false, ..*self }.pixel(r, g, b, a)
    }

    pub fn with_enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Convert one premultiplied RGBA pixel (as produced by MuPDF) to premultiplied 0xAARRGGBB.
    pub fn pixel(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        self.pixel_c(r, g, b, a, if self.contrast_vector { self.contrast } else { 1.0 })
    }

    /// Scans and image regions: `pixel` with the contrast curve always applied.
    pub fn pixel_scan(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        self.pixel_c(r, g, b, a, self.contrast)
    }

    fn pixel_c(&self, r: u8, g: u8, b: u8, a: u8, contrast: f32) -> u32 {
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
        let inkl = ink_curve(255 - lum, contrast);
        let ink = mul255(a, inkl);
        let hue = if self.keephue {
            let chroma = ur.max(ug).max(ub) - ur.min(ug).min(ub);
            let w = (chroma.saturating_sub(CHROMA_LO) * 256 / (CHROMA_HI - CHROMA_LO)).min(256);
            (w > 0).then(|| (w, chroma))
        } else {
            None
        };
        let [dr, dg, db] = self.dark.map(|c| c as u32);
        let la = self.light[3] as u32;
        let neutral = if la == 0 {
            pack(dr, dg, db, ink)
        } else {
        // blend ink over the (partly transparent) paper colour, premultiplied
        let paper = mul255(a - ink, la);
        let ch = |d: u32, l: u8| mul255(d, ink) + mul255(l as u32, paper);
        let alpha = ink + paper;
        (alpha << 24) | (ch(dr, self.light[0]).min(alpha) << 16) | (ch(dg, self.light[1]).min(alpha) << 8) | ch(db, self.light[2]).min(alpha)
        };
        let Some((w, chroma)) = hue else { return neutral };
        // coloured ink: same hue, lightness inverted; blended with the neutral look by chroma
        let [hr, hg, hb] = invert_lightness(ur, ug, ub);
        let coloured = pack(hr, hg, hb, mul255(a, inkl.max((chroma * 255 / CHROMA_HI).min(255))));
        let mix = |sh: u32| {
            let (n, c) = ((neutral >> sh) & 0xff, (coloured >> sh) & 0xff);
            (n * (256 - w) + c * w) >> 8
        };
        (mix(24) << 24) | (mix(16).min(mix(24)) << 16) | (mix(8).min(mix(24)) << 8) | mix(0).min(mix(24))
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::imagemap::Grid;

    #[test]
    fn contrast_curve() {
        for c in [1.0, 2.0, 3.5] {
            assert_eq!((ink_curve(0, c), ink_curve(255, c)), (0, 255));
            let mut prev = 0;
            for x in 0..=255 {
                let y = ink_curve(x, c);
                assert!(y >= prev, "monotonic at {x} for {c}");
                prev = y;
            }
        }
        assert_eq!(ink_curve(100, 1.0), 100); // off
        assert!(ink_curve(100, 2.5) > 150); // mid grey lifted
        assert!(ink_curve(10, 2.5) < 30); // faint noise stays faint
        assert!(ink_curve(128, 3.0) > ink_curve(128, 2.0));
    }

    #[test]
    fn contrast_applies_to_scans_not_vector_by_default() {
        let rc = Recolor { contrast: 2.5, ..Recolor::default() };
        let grey = (0x80, 0x80, 0x80, 255);
        let plain = rc.pixel(grey.0, grey.1, grey.2, grey.3) >> 24;
        let scan = rc.pixel_scan(grey.0, grey.1, grey.2, grey.3) >> 24;
        assert!(scan > plain + 40, "{scan} vs {plain}");
        let rv = Recolor { contrast_vector: true, ..rc };
        assert_eq!(rv.pixel(grey.0, grey.1, grey.2, grey.3) >> 24, scan);
        // scan pages get an Ink region only when the curve is on and not already global
        let img = [ImgRect { rect: [0.0, 0.0, 100.0, 100.0], dark: false, line: false, id: 1, ovr: crate::imagemap::Ovr::Auto, grid: crate::imagemap::Grid::NONE }];
        assert_eq!(rc.regions(&img, 10000.0, 1.0, (0, 0), (100, 100))[0].kind, RegionKind::Ink);
        assert!(rv.regions(&img, 10000.0, 1.0, (0, 0), (100, 100)).is_empty());
        assert!(Recolor::default().regions(&img, 10000.0, 1.0, (0, 0), (100, 100)).is_empty());
    }

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

    fn parts(p: u32) -> (u32, u32, u32, u32) {
        (p >> 24, (p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff)
    }

    #[test]
    fn keephue_keeps_hue_of_colored_ink_and_lightens_it() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        let (a, r, g, b) = parts(rc.pixel(220, 20, 20, 255)); // strong red stays a readable red
        assert!(a >= 250 && r > 200 && r > 2 * g && r > 2 * b, "{a} {r} {g} {b}");
        // neutral ink still uses the dark colour
        assert_eq!(rc.pixel(0, 0, 0, 255), 0xffff_ffff);
    }

    #[test]
    fn dark_blue_becomes_light_blue() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        let (a, r, g, b) = parts(rc.pixel(20, 40, 140, 255));
        assert_eq!(a, 255);
        let lum = (54 * r + 183 * g + 19 * b) >> 8;
        assert!(lum >= 135, "light enough for a dark page: lum {lum}");
        assert!(b > r + 60 && b > g, "still blue: {r} {g} {b}");
    }

    #[test]
    fn dark_tinted_jpeg_text_is_neutral_ink() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        // near-black with a bluish cast has high HSV saturation but tiny chroma: plain white ink
        assert_eq!(rc.pixel(20, 25, 35, 255), rc.pixel_neutral_for_test(20, 25, 35, 255));
        let (a, r, g, b) = parts(rc.pixel(20, 25, 35, 255));
        assert!(a > 200 && r > 200 && g > 200 && b > 200, "{a} {r} {g} {b}");
    }

    #[test]
    fn noisy_black_glyph_row_has_no_hollow_core() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        // luminance ramps from white paper into a black stem with JPEG chroma noise on the dark part
        let row: Vec<u32> = (0..=10)
            .map(|i| {
                let v = 255 - i * 25; // 255 .. 5
                let n = if v < 120 { (i as i32 % 3 - 1) * 12 } else { 0 };
                let c = |d: i32| (v as i32 + d).clamp(0, 255) as u8;
                parts(rc.pixel(c(n), c(0), c(-n), 255)).0
            })
            .collect();
        assert!(row.windows(2).all(|w| w[1] + 3 >= w[0]), "alpha must rise into the stem: {row:?}");
        assert!(row[10] > 230);
    }

    #[test]
    fn white_stays_transparent_with_keephue() {
        let rc = Recolor { keephue: true, ..Recolor::default() };
        assert_eq!(rc.pixel(255, 255, 255, 255), 0);
    }

    #[test]
    fn disabled_passes_through() {
        let rc = Recolor { enabled: false, ..Recolor::default() };
        assert_eq!(rc.pixel(10, 20, 30, 40), 0x280a_141e);
    }

    #[test]
    fn regions_skip_scans_and_mask_pixels() {
        let rc = Recolor::default(); // Keep
        let page_area = 600.0 * 800.0;
        let photo = ImgRect { rect: [100.0, 100.0, 200.0, 150.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let big = ImgRect { rect: [0.0, 400.0, 600.0, 790.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE }; // under the scan share
        let r = rc.regions(&[photo, big], page_area, 1.0, (0, 0), (600, 800));
        assert_eq!(r.len(), 2);
        let r = rc.regions(&[photo], page_area, 1.0, (0, 0), (600, 800));
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].x0, r[0].y0, r[0].x1, r[0].y1), (100, 100, 200, 150));
        // a row inside the photo keeps white as opaque white; outside, white paper vanishes
        let line = [255u8; 4 * 300];
        let mut out = Vec::new();
        rc.convert_row(&line, 120, &r, &mut out);
        assert_eq!(out[150], 0xffff_ffff);
        assert_eq!(out[250], 0);
        assert_eq!(out[50], 0);
        assert_eq!(out[100], 0xffff_ffff);
        assert_eq!(out[99], 0);
        // a row above the photo is entirely recolored
        let mut out = Vec::new();
        rc.convert_row(&line, 10, &r, &mut out);
        assert!(out.iter().all(|&p| p == 0));
    }

    #[test]
    fn scanned_page_crops_are_recolored() {
        let rc = Recolor::default();
        let scan = ImgRect { rect: [0.0, 0.0, 600.0, 790.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let crop = ImgRect { rect: [100.0, 100.0, 300.0, 130.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        assert!(rc.regions(&[scan, crop], 600.0 * 800.0, 1.0, (0, 0), (600, 800)).is_empty());
    }

    #[test]
    fn regions_are_clipped_to_the_tile() {
        let rc = Recolor::default();
        let im = ImgRect { rect: [10.0, 100.0, 90.0, 300.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        // tile covers page rows 200..260 of a 100px wide page
        let r = rc.regions(&[im], 1.0e6, 1.0, (0, 200), (100, 60));
        assert_eq!((r[0].y0, r[0].y1), (0, 60));
        assert!(rc.regions(&[im], 1.0e6, 1.0, (0, 400), (100, 60)).is_empty());
    }

    #[test]
    fn recolor_mode_and_off_ignore_images() {
        let im = ImgRect { rect: [0.0, 0.0, 10.0, 10.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let rc = Recolor { images: ImageMode::Recolor, ..Recolor::default() };
        assert!(rc.regions(&[im], 1.0e6, 1.0, (0, 0), (50, 50)).is_empty());
        assert!(!rc.wants_images());
        assert!(!Recolor { enabled: false, ..Recolor::default() }.wants_images());
        assert!(Recolor::default().wants_images());
    }

    #[test]
    fn overrides_beat_class_and_scan() {
        let rc = Recolor::default();
        let mk = |line, ovr| ImgRect { rect: [100.0, 100.0, 200.0, 150.0], dark: false, line, id: 7, ovr, grid: Grid::NONE };
        let kind = |im: ImgRect| rc.regions(&[im], 600.0 * 800.0, 1.0, (0, 0), (600, 800)).first().map(|r| r.kind);
        assert_eq!(kind(mk(true, Ovr::Auto)), Some(RegionKind::LineArt));
        assert_eq!(kind(mk(true, Ovr::Keep)), Some(RegionKind::Keep));
        assert_eq!(kind(mk(false, Ovr::Recolor)), Some(RegionKind::LineArt));
        assert_eq!(kind(mk(false, Ovr::Auto)), Some(RegionKind::Keep));
        let scan = ImgRect { rect: [0.0, 0.0, 600.0, 790.0], dark: false, line: false, id: 8, ovr: Ovr::Keep, grid: Grid::NONE };
        assert_eq!(rc.regions(&[scan], 600.0 * 800.0, 1.0, (0, 0), (600, 800))[0].kind, RegionKind::Keep);
    }

    #[test]
    fn dim_scales_brightness() {
        let rc = Recolor { images: ImageMode::Dim, image_dim: 0.5, ..Recolor::default() };
        let p = rc.keep_pixel(200, 100, 50, 255);
        assert_eq!(p >> 24, 255);
        assert_eq!((p >> 16) & 0xff, 100);
        assert_eq!((p >> 8) & 0xff, 50);
        assert_eq!(p & 0xff, 25);
        assert_eq!(Recolor::default().keep_pixel(200, 100, 50, 255), 0xffc8_6432);
    }

    #[test]
    fn dark_scans_are_inverted_only_when_asked() {
        let dark_scan = ImgRect { rect: [0.0, 0.0, 600.0, 800.0], dark: true, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let mut rc = Recolor::default();
        assert!(rc.regions(&[dark_scan], 600.0 * 800.0, 1.0, (0, 0), (600, 800)).is_empty());
        rc.invert_dark = true;
        let r = rc.regions(&[dark_scan], 600.0 * 800.0, 1.0, (0, 0), (600, 800));
        assert_eq!(r[0].kind, RegionKind::Invert);
        // near-black stays background (transparent), white becomes ink
        assert_eq!(rc.inverted_pixel(0, 0, 0, 255), 0);
        assert_eq!(rc.inverted_pixel(255, 255, 255, 255), 0xffff_ffff);
    }

    fn noise(i: usize) -> i32 {
        ((i * 2654435761) >> 7) as i32 % 9 - 4 // +-4, JPEG-like
    }

    #[test]
    fn white_background_line_art_is_detected_despite_noise() {
        let px: Vec<[u8; 3]> = (0..4000)
            .map(|i| {
                if i % 14 == 0 { [(40 + noise(i)) as u8; 3] } else { [(251 + noise(i).min(4)) as u8; 3] }
            })
            .collect();
        assert!(is_line_art(px.into_iter()));
    }

    #[test]
    fn coloured_strokes_on_white_are_line_art_and_keep_their_hue() {
        let px: Vec<[u8; 3]> = (0..4000).map(|i| match i % 12 { 0 => [220, 20, 20], 1 => [20, 40, 200], _ => [255, 255, 255] }).collect();
        assert!(is_line_art(px.into_iter()));
        let rc = Recolor::default();
        let p = rc.pixel_hue(220, 20, 20, 255);
        assert!(p >> 24 >= 200, "coloured ink stays strongly opaque");
        assert!(((p >> 16) & 0xff) > 2 * ((p >> 8) & 0xff), "still red");
        assert_eq!(rc.pixel_hue(255, 255, 255, 255), 0, "white background becomes transparent");
    }

    #[test]
    fn photos_and_gradients_are_not_line_art() {
        let grad: Vec<[u8; 3]> = (0..4000).map(|i| [(i % 256) as u8, ((i * 3) % 256) as u8, 120]).collect();
        assert!(!is_line_art(grad.into_iter()));
        let grey: Vec<[u8; 3]> = (0..4000).map(|i| [(i % 200) as u8 + 20; 3]).collect(); // greyscale photo
        assert!(!is_line_art(grey.into_iter()));
        // a photo cut out on white: plenty of white but a continuous range of colours
        let cut: Vec<[u8; 3]> = (0..4000).map(|i| if i % 2 == 0 { [255, 255, 255] } else { [(i % 251) as u8, ((i * 7) % 251) as u8, ((i * 13) % 251) as u8] }).collect();
        assert!(!is_line_art(cut.into_iter()));
        let logo: Vec<[u8; 3]> = (0..4000).map(|i| match i % 10 { 0..=3 => [255, 255, 255], 4..=6 => [200, 30, 40], 7 => [20, 20, 20], 8 => [120, 20, 30], _ => [255, 255, 255] }).collect();
        assert!(is_line_art(logo.into_iter()));
        let dark: Vec<[u8; 3]> = (0..4000).map(|i| [30, (40 + noise(i)) as u8, 60]).collect();
        assert!(!is_line_art(dark.into_iter()));
    }

    #[test]
    fn logos_on_an_off_white_background_are_line_art() {
        // IISER-style logo: red emblem, black ring and text on a light grey-white (not above the white threshold)
        let logo: Vec<[u8; 3]> = (0..4000)
            .map(|i| match i % 20 {
                0..=9 => [(214 + noise(i)) as u8; 3],
                10..=13 => [215, 215, 218],
                14..=16 => [200, 25, 30],
                17 => [15, 15, 15],
                18 => [140, 20, 25],
                _ => [214, 214, 214],
            })
            .collect();
        assert!(is_line_art(logo.into_iter()));
        // a soft-masked logo composited on white: mostly white, three flat colours
        let masked: Vec<[u8; 3]> = (0..4000).map(|i| match i % 20 { 0..=9 => [255; 3], 10..=14 => [255; 3], 15..=17 => [210, 10, 20], _ => [0; 3] }).collect();
        assert!(is_line_art(masked.into_iter()));
        // a bright but continuous-tone photo (e.g. a sky and a vial) is still a photo
        let photo: Vec<[u8; 3]> = (0..4000).map(|i| if i % 3 == 0 { [(200 + i % 50) as u8, (205 + (i * 3) % 50) as u8, (210 + (i * 7) % 45) as u8] } else { [(i % 256) as u8, ((i * 5) % 256) as u8, ((i * 11) % 200) as u8] }).collect();
        assert!(!is_line_art(photo.into_iter()));
        let vial: Vec<[u8; 3]> = (0..4000).map(|i| match i % 8 { 0..=2 => [245, 240, 235], 3 | 4 => [(220 + i % 30) as u8, (190 + i % 40) as u8, (30 + i % 60) as u8], _ => [(100 + i % 120) as u8, (80 + (i * 3) % 120) as u8, (40 + (i * 7) % 150) as u8] }).collect();
        assert!(!is_line_art(vial.into_iter()));
    }

    #[test]
    fn smart_mode_recolors_line_art_regions_only() {
        let rc = Recolor::default();
        let art = ImgRect { rect: [100.0, 100.0, 200.0, 150.0], dark: false, line: true, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let photo = ImgRect { rect: [300.0, 100.0, 400.0, 150.0], dark: false, line: false, id: 0, ovr: Ovr::Auto, grid: Grid::NONE };
        let r = rc.regions(&[art, photo], 600.0 * 800.0, 1.0, (0, 0), (600, 800));
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].kind, r[1].kind), (RegionKind::LineArt, RegionKind::Keep));
    }

    #[test]
    fn regions_use_the_visible_rect_not_the_unclipped_one() {
        let rc = Recolor::default();
        // the image extends past the page, but only [0,100,400,450] shows
        let im = ImgRect { rect: [0.0, 100.0, 400.0, 450.0], dark: true, line: false, id: 1, ovr: Ovr::Auto, grid: Grid::NONE };
        let r = rc.regions(&[im], 720.0 * 540.0, 1.0, (0, 0), (720, 540));
        assert_eq!((r[0].x0, r[0].y0, r[0].x1, r[0].y1), (0, 100, 400, 450));
        // a title bar above the crop is outside every region, so it is recolored like text
        assert!(!r.iter().any(|g| g.y0 < 100));
    }

    #[test]
    fn mixed_crops_get_a_region_per_cell() {
        let rc = Recolor::default();
        // 2x2 cells of 100x100: top row line art, bottom row photo
        let g = Grid { nx: 2, ny: 2, bits: 0b0011 };
        let im = ImgRect { rect: [100.0, 100.0, 300.0, 300.0], dark: false, line: false, id: 1, ovr: Ovr::Auto, grid: g };
        let r = rc.regions(&[im], 1.0e6, 1.0, (0, 0), (400, 400));
        assert_eq!(r.len(), 4);
        let kind = |x, y| r.iter().find(|g| x >= g.x0 && x < g.x1 && y >= g.y0 && y < g.y1).map(|g| g.kind);
        assert_eq!(kind(150, 150), Some(RegionKind::LineArt));
        assert_eq!(kind(250, 150), Some(RegionKind::LineArt));
        assert_eq!(kind(150, 250), Some(RegionKind::Keep));
        assert_eq!(kind(250, 250), Some(RegionKind::Keep));
        // an override applies to the whole placement
        let k = ImgRect { ovr: Ovr::Keep, ..im };
        let r = rc.regions(&[k], 1.0e6, 1.0, (0, 0), (400, 400));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RegionKind::Keep);
    }
}
