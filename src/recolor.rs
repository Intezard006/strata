//! Alpha-preserving recolor. Pages are rendered on a transparent background, so the
//! "light" colour is simply transparency (the window background shows through) and
//! dark ink is mapped to `dark` with coverage derived from luminance and alpha.

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
/// Neutral: saturation (max-min)/max below this.
const NEUTRAL_SAT: f32 = 0.12;

/// Whether sampled RGB pixels (a thumbnail of an image placement) look like line art: a dominant
/// near-white background with sparse ink, hues allowed (coloured strokes), or an almost greyscale
/// image that is at least half white. Tolerant of JPEG noise through the thresholds.
pub fn is_line_art(px: impl Iterator<Item = [u8; 3]>) -> bool {
    let (mut n, mut white, mut neutral) = (0u32, 0u32, 0u32);
    let mut bins = [0u32; 512];
    for [r, g, b] in px {
        n += 1;
        bins[((r >> 5) as usize) << 6 | ((g >> 5) as usize) << 3 | (b >> 5) as usize] += 1;
        let (mx, mn) = (r.max(g).max(b) as f32, r.min(g).min(b) as f32);
        let sat = if mx == 0.0 { 0.0 } else { (mx - mn) / mx };
        let lum = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
        if sat < NEUTRAL_SAT {
            neutral += 1;
            if lum > WHITE_L {
                white += 1;
            }
        } else if mn / 255.0 > WHITE_L {
            white += 1; // faintly tinted paper
        }
    }
    if n == 0 {
        return false;
    }
    let (w, ne) = (white as f32 / n as f32, neutral as f32 / n as f32);
    // flat-colour graphics (logos): a mostly white image whose pixels sit in a handful of colours
    bins.sort_unstable_by(|a, b| b.cmp(a));
    let flat = bins[..6].iter().sum::<u32>() as f32 / n as f32;
    w >= 0.65 || (ne >= 0.8 && w >= 0.5) || (w >= 0.35 && flat >= 0.85)
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
}

impl Default for Recolor {
    fn default() -> Self {
        Recolor { enabled: true, dark: [0xff, 0xff, 0xff], keephue: false, light: [0; 4], images: ImageMode::Smart, image_dim: 0.85, invert_dark: false }
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
        for im in imgs {
            let r = im.rect;
            let area = (r[2] - r[0]) * (r[3] - r[1]);
            let scan = scan_page || area > SCAN_SHARE * page_area;
            let kind = if im.dark && self.invert_dark && scan {
                RegionKind::Invert
            } else if scan || self.images == ImageMode::Recolor {
                continue;
            } else if self.images == ImageMode::Smart && im.line {
                RegionKind::LineArt
            } else {
                RegionKind::Keep
            };
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
        Recolor { keephue: true, ..*self }.pixel(r, g, b, a)
    }

    pub fn inverted_pixel(&self, r: u8, g: u8, b: u8, a: u8) -> u32 {
        if a == 0 {
            return 0;
        }
        let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        self.pixel(255 - un(r), 255 - un(g), 255 - un(b), a)
    }
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

    #[test]
    fn regions_skip_scans_and_mask_pixels() {
        let rc = Recolor::default(); // Keep
        let page_area = 600.0 * 800.0;
        let photo = ImgRect { rect: [100.0, 100.0, 200.0, 150.0], dark: false, line: false };
        let big = ImgRect { rect: [0.0, 400.0, 600.0, 790.0], dark: false, line: false }; // under the scan share
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
        let scan = ImgRect { rect: [0.0, 0.0, 600.0, 790.0], dark: false, line: false };
        let crop = ImgRect { rect: [100.0, 100.0, 300.0, 130.0], dark: false, line: false };
        assert!(rc.regions(&[scan, crop], 600.0 * 800.0, 1.0, (0, 0), (600, 800)).is_empty());
    }

    #[test]
    fn regions_are_clipped_to_the_tile() {
        let rc = Recolor::default();
        let im = ImgRect { rect: [10.0, 100.0, 90.0, 300.0], dark: false, line: false };
        // tile covers page rows 200..260 of a 100px wide page
        let r = rc.regions(&[im], 1.0e6, 1.0, (0, 200), (100, 60));
        assert_eq!((r[0].y0, r[0].y1), (0, 60));
        assert!(rc.regions(&[im], 1.0e6, 1.0, (0, 400), (100, 60)).is_empty());
    }

    #[test]
    fn recolor_mode_and_off_ignore_images() {
        let im = ImgRect { rect: [0.0, 0.0, 10.0, 10.0], dark: false, line: false };
        let rc = Recolor { images: ImageMode::Recolor, ..Recolor::default() };
        assert!(rc.regions(&[im], 1.0e6, 1.0, (0, 0), (50, 50)).is_empty());
        assert!(!rc.wants_images());
        assert!(!Recolor { enabled: false, ..Recolor::default() }.wants_images());
        assert!(Recolor::default().wants_images());
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
        let dark_scan = ImgRect { rect: [0.0, 0.0, 600.0, 800.0], dark: true, line: false };
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
        assert!(((p >> 16) & 0xff) > 4 * ((p >> 8) & 0xff), "still red");
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
        let dark: Vec<[u8; 3]> = (0..4000).map(|i| [30, 40 + noise(i) as u8, 60]).collect();
        assert!(!is_line_art(dark.into_iter()));
    }

    #[test]
    fn smart_mode_recolors_line_art_regions_only() {
        let rc = Recolor::default();
        let art = ImgRect { rect: [100.0, 100.0, 200.0, 150.0], dark: false, line: true };
        let photo = ImgRect { rect: [300.0, 100.0, 400.0, 150.0], dark: false, line: false };
        let r = rc.regions(&[art, photo], 600.0 * 800.0, 1.0, (0, 0), (600, 800));
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].kind, r[1].kind), (RegionKind::LineArt, RegionKind::Keep));
    }
}
