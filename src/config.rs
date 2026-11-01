//! Options and the zathurarc-style config parser. Reload is manual (no file watcher).

use std::path::{Path, PathBuf};

use crate::recolor::{ImageMode, Recolor};

#[derive(Clone)]
pub struct Config {
    pub cache_mb: usize,
    pub cache_max_mb: u64,
    pub default_bg: [u8; 4],
    pub recolor: Recolor,
    pub scroll_step: f32,
    /// Logical pixels per mouse-wheel notch.
    pub wheel_step: f32,
    /// Multiplier for touchpad (pixel-delta) scrolling.
    pub touchpad_scroll_factor: f32,
    /// Percent per Ctrl+wheel notch; 0 follows `zoom-step`.
    pub wheel_zoom_step: f32,
    /// Percent per zoom step.
    pub zoom_step: f32,
    pub page_gap: f32,
    pub statusbar: bool,
    pub font: String,
    pub font_size: f32,
    pub incremental_search: bool,
    pub auto_ocr: bool,
    pub auto_index: AutoIndex,
    pub auto_analyze: bool,
    pub previews: bool,
    pub preview_hover_ms: u64,
    pub preview_max_scale: f32,
    pub preview_bg: [u8; 4],
    pub restore_position: bool,
    pub goto_printed: bool,
    pub float_width: u32,
    pub float_height: u32,
    pub float_position: String,
    pub synctex: bool,
    pub synctex_editor_command: String,
    pub auto_reload: bool,
    pub first_page_single: bool,
    pub book_rtl: bool,
    pub presentation_bg: [u8; 4],
}

/// A look profile: option overrides applied on top of the base config.
pub struct Profile {
    pub name: String,
    pub opts: Vec<(String, String)>,
}

/// The profiles used when the config defines none: the old zathura `normal` and `textbook` looks.
pub fn builtin_profiles() -> Vec<Profile> {
    let p = |name: &str, o: &[(&str, &str)]| Profile { name: name.into(), opts: o.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() };
    vec![
        p("normal", &[("recolor-lightcolor", "#1e1e2e"), ("recolor-darkcolor", "#ffffff"), ("default-bg", "#1e1e2e99"), ("recolor-keephue", "true")]),
        p("textbook", &[("recolor-lightcolor", "rgba(0,0,0,0)"), ("recolor-darkcolor", "rgba(255,255,255,1)"), ("default-bg", "rgba(0,0,0,0)"), ("recolor-keephue", "false")]),
    ]
}

/// Apply profile `name`: every option some profile touches first returns to its `base` value, then the
/// profile's own values are set. `None` when there is no such profile; otherwise the errors met.
pub fn apply_profile(profiles: &[Profile], name: &str, base: &Config, cfg: &mut Config) -> Option<Vec<String>> {
    let p = profiles.iter().find(|p| p.name == name)?;
    let mut touched: Vec<&str> = profiles.iter().flat_map(|q| q.opts.iter().map(|o| o.0.as_str())).collect();
    touched.sort_unstable();
    touched.dedup();
    for o in touched {
        if let Some(v) = base.get(o) {
            let _ = cfg.set(o, &v);
        }
    }
    Some(p.opts.iter().filter_map(|(o, v)| cfg.set(o, v).err()).collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoIndex {
    Auto,
    Always,
    Off,
}

/// (name, description) for `:set` completion and the help overlay.
pub const OPTIONS: &[(&str, &str)] = &[
    ("cache-mb", "Rendered page cache size in MB"),
    ("cache-max-mb", "Cap for the on-disk analysis/OCR cache in MB (LRU eviction)"),
    ("default-bg", "Window background colour (#rrggbb, #rrggbbaa, rgba(r,g,b,a))"),
    ("recolor", "Recolor pages (true/false)"),
    ("recolor-lightcolor", "Paper colour; same RGB as default-bg keeps pages transparent"),
    ("recolor-darkcolor", "Ink colour"),
    ("recolor-keephue", "Keep the hue of coloured ink (true/false)"),
    ("recolor-images", "Images when recoloring: smart (line art recolored, photos kept), keep, dim, recolor"),
    ("recolor-contrast", "Ink contrast curve for scans and line art (1 = off; higher makes grey ink solid)"),
    ("recolor-contrast-vector", "Apply recolor-contrast to vector text and drawings too (true/false)"),
    ("recolor-image-dim", "Brightness of dimmed images (0.2-1)"),
    ("recolor-invert-dark-images", "Recolor dark page-sized images (covers) with inverted mapping (true/false)"),
    ("scroll-step", "Logical pixels per j/k step"),
    ("zoom-step", "Zoom step in percent"),
    ("wheel-step", "Logical pixels per mouse-wheel notch"),
    ("touchpad-scroll-factor", "Multiplier for touchpad scrolling (0.05-20)"),
    ("wheel-zoom-step", "Ctrl+wheel zoom step in percent per notch (0 = same as zoom-step)"),
    ("page-gap", "Gap between pages in points"),
    ("statusbar", "Show the statusbar (true/false)"),
    ("font", "UI font family (resolved with fc-match)"),
    ("font-size", "UI font size in logical pixels"),
    ("incremental-search", "Search while typing (true/false)"),
    ("auto-ocr", "OCR scanned documents in the background (true/false)"),
    ("auto-index", "Generated index: auto (only without outline), always, off"),
    ("auto-analyze", "Analyse documents in a background process (true/false)"),
    ("previews", "Hover/keyboard previews of equations, theorems, figures (true/false)"),
    ("preview-hover-ms", "Hover time before a preview popup opens, in ms"),
    ("preview-bg", "Preview popup background (#rrggbbaa)"),
    ("preview-max-scale", "Maximum render scale of the popup (1 = 100%)"),
    ("restore-position", "Reopen documents at the last position, zoom and recolor state (true/false)"),
    ("goto-printed", "`:goto N` and `NG` use printed page numbers when the book has them (true/false)"),
    ("float-width", "Float window width in logical pixels"),
    ("float-height", "Float window height in logical pixels"),
    ("float-position", "Float window position: top-right, top-left, bottom-right, bottom-left, center"),
    ("synctex", "SyncTeX forward/backward search (true/false)"),
    ("synctex-editor-command", "Run on Ctrl+click; %{input} %{line} %{column} are substituted (quote the value)"),
    ("auto-reload", "Reload the file when it changes on disk, keeping page, offset and zoom (true/false)"),
    ("first-page-single", "Book mode: show the first page alone, as a cover (true/false)"),
    ("book-rtl", "Book mode: right-to-left spreads (true/false)"),
    ("presentation-bg", "Presentation mode background (#rrggbbaa or rgba(r,g,b,a); transparent allowed)"),
];

impl Default for Config {
    fn default() -> Self {
        Config {
            cache_mb: 40,
            cache_max_mb: 2048,
            default_bg: [0x1e, 0x1e, 0x2e, 0x99],
            recolor: Recolor { light: [0x1e, 0x1e, 0x2e, 0xff], contrast: 2.5, ..Recolor::default() },
            scroll_step: 40.0,
            zoom_step: 10.0,
            wheel_step: 50.0,
            touchpad_scroll_factor: 1.0,
            wheel_zoom_step: 0.0,
            page_gap: 8.0,
            statusbar: true,
            font: "monospace".into(),
            font_size: 13.0,
            incremental_search: true,
            auto_ocr: true,
            auto_index: AutoIndex::Auto,
            auto_analyze: true,
            previews: true,
            preview_hover_ms: 300,
            preview_max_scale: 3.0,
            preview_bg: [0x1e, 0x1e, 0x2e, 0xf2],
            restore_position: true,
            goto_printed: true,
            float_width: 520,
            float_height: 700,
            float_position: "top-right".into(),
            synctex: true,
            synctex_editor_command: String::new(),
            auto_reload: true,
            first_page_single: true,
            book_rtl: false,
            presentation_bg: [0, 0, 0, 255],
        }
    }
}

pub fn parse_bool(v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("not a boolean: {v}")),
    }
}

fn parse_num(v: &str, min: f32, max: f32) -> Result<f32, String> {
    let n: f32 = v.parse().map_err(|_| format!("not a number: {v}"))?;
    if !(min..=max).contains(&n) {
        return Err(format!("{v} out of range {min}..{max}"));
    }
    Ok(n)
}

/// `#rrggbb`, `#rrggbbaa` or `rgba(r,g,b,a)` with a in 0..1 (rgb(r,g,b) also accepted).
pub fn parse_color(s: &str) -> Result<[u8; 4], String> {
    let s = s.trim();
    let bad = || format!("bad colour: {s}");
    if let Some(h) = s.strip_prefix('#') {
        if (h.len() != 6 && h.len() != 8) || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        return Ok([b(0), b(2), b(4), if h.len() == 8 { b(6) } else { 255 }]);
    }
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
        .ok_or_else(bad)?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(bad());
    }
    let mut out = [0u8, 0, 0, 255];
    for i in 0..3 {
        out[i] = parts[i].parse::<u8>().map_err(|_| bad())?;
    }
    if let Some(a) = parts.get(3) {
        let a: f32 = a.parse().map_err(|_| bad())?;
        if !(0.0..=1.0).contains(&a) {
            return Err(bad());
        }
        out[3] = (a * 255.0).round() as u8;
    }
    Ok(out)
}

fn fmt_color(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
}

impl Config {
    pub fn set(&mut self, name: &str, v: &str) -> Result<(), String> {
        match name {
            "cache-mb" => self.cache_mb = parse_num(v, 1.0, 4096.0)? as usize,
            "cache-max-mb" => self.cache_max_mb = parse_num(v, 16.0, 1_000_000.0)? as u64,
            "default-bg" => self.default_bg = parse_color(v)?,
            "recolor" => self.recolor.enabled = parse_bool(v)?,
            "recolor-lightcolor" => self.recolor.light = parse_color(v)?,
            "recolor-darkcolor" => {
                let c = parse_color(v)?;
                self.recolor.dark = [c[0], c[1], c[2]];
            }
            "recolor-keephue" => self.recolor.keephue = parse_bool(v)?,
            "recolor-images" => {
                self.recolor.images = match v {
                    "keep" => ImageMode::Keep,
                    "dim" => ImageMode::Dim,
                    "smart" => ImageMode::Smart,
                    "recolor" => ImageMode::Recolor,
                    _ => return Err(format!("recolor-images: expected smart|keep|dim|recolor, got {v}")),
                }
            }
            "recolor-contrast" => self.recolor.contrast = parse_num(v, 1.0, 6.0)?,
            "recolor-contrast-vector" => self.recolor.contrast_vector = parse_bool(v)?,
            "recolor-image-dim" => self.recolor.image_dim = parse_num(v, 0.2, 1.0)?,
            "recolor-invert-dark-images" => self.recolor.invert_dark = parse_bool(v)?,
            "scroll-step" => self.scroll_step = parse_num(v, 1.0, 2000.0)?,
            "zoom-step" => self.zoom_step = parse_num(v, 1.0, 100.0)?,
            "wheel-step" => self.wheel_step = parse_num(v, 1.0, 2000.0)?,
            "touchpad-scroll-factor" => self.touchpad_scroll_factor = parse_num(v, 0.05, 20.0)?,
            "wheel-zoom-step" => self.wheel_zoom_step = parse_num(v, 0.0, 100.0)?,
            "page-gap" => self.page_gap = parse_num(v, 0.0, 200.0)?,
            "statusbar" => self.statusbar = parse_bool(v)?,
            "font" => self.font = v.to_string(),
            "font-size" => self.font_size = parse_num(v, 6.0, 72.0)?,
            "incremental-search" => self.incremental_search = parse_bool(v)?,
            "auto-ocr" => self.auto_ocr = parse_bool(v)?,
            "auto-analyze" => self.auto_analyze = parse_bool(v)?,
            "previews" => self.previews = parse_bool(v)?,
            "preview-hover-ms" => self.preview_hover_ms = parse_num(v, 0.0, 5000.0)? as u64,
            "preview-max-scale" => self.preview_max_scale = parse_num(v, 0.5, 8.0)?,
            "preview-bg" => self.preview_bg = parse_color(v)?,
            "restore-position" => self.restore_position = parse_bool(v)?,
            "goto-printed" => self.goto_printed = parse_bool(v)?,
            "float-width" => self.float_width = parse_num(v, 200.0, 4000.0)? as u32,
            "float-height" => self.float_height = parse_num(v, 150.0, 4000.0)? as u32,
            "float-position" => {
                if !["top-right", "top-left", "bottom-right", "bottom-left", "center"].contains(&v) {
                    return Err(format!("float-position: expected top-right|top-left|bottom-right|bottom-left|center, got {v}"));
                }
                self.float_position = v.to_string();
            }
            "synctex" => self.synctex = parse_bool(v)?,
            "synctex-editor-command" => self.synctex_editor_command = v.to_string(),
            "auto-reload" => self.auto_reload = parse_bool(v)?,
            "first-page-single" => self.first_page_single = parse_bool(v)?,
            "book-rtl" => self.book_rtl = parse_bool(v)?,
            "presentation-bg" => self.presentation_bg = parse_color(v)?,
            "auto-index" => {
                self.auto_index = match v {
                    "auto" => AutoIndex::Auto,
                    "always" => AutoIndex::Always,
                    "off" => AutoIndex::Off,
                    _ => return Err(format!("auto-index: expected auto|always|off, got {v}")),
                }
            }
            _ => return Err(format!("unknown option: {name}")),
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<String> {
        Some(match name {
            "cache-mb" => self.cache_mb.to_string(),
            "cache-max-mb" => self.cache_max_mb.to_string(),
            "default-bg" => fmt_color(self.default_bg),
            "recolor" => self.recolor.enabled.to_string(),
            "recolor-lightcolor" => fmt_color(self.recolor.light),
            "recolor-darkcolor" => fmt_color([self.recolor.dark[0], self.recolor.dark[1], self.recolor.dark[2], 255]),
            "recolor-keephue" => self.recolor.keephue.to_string(),
            "recolor-images" => format!("{:?}", self.recolor.images).to_lowercase(),
            "recolor-contrast" => self.recolor.contrast.to_string(),
            "recolor-contrast-vector" => self.recolor.contrast_vector.to_string(),
            "recolor-image-dim" => self.recolor.image_dim.to_string(),
            "recolor-invert-dark-images" => self.recolor.invert_dark.to_string(),
            "scroll-step" => self.scroll_step.to_string(),
            "zoom-step" => self.zoom_step.to_string(),
            "wheel-step" => self.wheel_step.to_string(),
            "touchpad-scroll-factor" => self.touchpad_scroll_factor.to_string(),
            "wheel-zoom-step" => self.wheel_zoom_step.to_string(),
            "page-gap" => self.page_gap.to_string(),
            "statusbar" => self.statusbar.to_string(),
            "font" => self.font.clone(),
            "font-size" => self.font_size.to_string(),
            "incremental-search" => self.incremental_search.to_string(),
            "auto-ocr" => self.auto_ocr.to_string(),
            "auto-analyze" => self.auto_analyze.to_string(),
            "previews" => self.previews.to_string(),
            "preview-hover-ms" => self.preview_hover_ms.to_string(),
            "preview-max-scale" => self.preview_max_scale.to_string(),
            "preview-bg" => fmt_color(self.preview_bg),
            "restore-position" => self.restore_position.to_string(),
            "goto-printed" => self.goto_printed.to_string(),
            "float-width" => self.float_width.to_string(),
            "float-height" => self.float_height.to_string(),
            "float-position" => self.float_position.clone(),
            "synctex" => self.synctex.to_string(),
            "synctex-editor-command" => self.synctex_editor_command.clone(),
            "auto-reload" => self.auto_reload.to_string(),
            "first-page-single" => self.first_page_single.to_string(),
            "book-rtl" => self.book_rtl.to_string(),
            "presentation-bg" => fmt_color(self.presentation_bg),
            "auto-index" => format!("{:?}", self.auto_index).to_lowercase(),
            _ => return None,
        })
    }

    /// Effective recolor settings: paper with the same RGB as the window background stays transparent.
    pub fn effective_recolor(&self) -> Recolor {
        let mut r = self.recolor;
        if r.light[..3] == self.default_bg[..3] {
            r.light[3] = 0;
        }
        r
    }
}

/// Window background as premultiplied 0xAARRGGBB.
pub fn premultiply(c: [u8; 4]) -> u32 {
    let a = c[3] as u32;
    let m = |v: u8| (v as u32 * a + 127) / 255;
    (a << 24) | (m(c[0]) << 16) | (m(c[1]) << 8) | m(c[2])
}

/// Split a line into words honouring quotes and backslash escapes; `#` starts a comment.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_tok = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') | (None, '\\') => {
                in_tok = true;
                match chars.next() {
                    Some(n) => cur.push(n),
                    None => return Err("trailing backslash".into()),
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                in_tok = true;
            }
            (None, '#') if !in_tok && (out.is_empty() || chars.clone().next().map_or(true, |n| n.is_whitespace())) => break,
            (None, c) if c.is_whitespace() => {
                if in_tok {
                    out.push(std::mem::take(&mut cur));
                    in_tok = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_tok = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if in_tok {
        out.push(cur);
    }
    Ok(out)
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default();
    base.join("strata").join("stratarc")
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => PathBuf::from(p),
    }
}

/// One parsed config line: `(file, line number, words)`.
pub type Item = (String, usize, Vec<String>);

/// Read a config file, following `include` (relative to the including file).
pub fn read(path: &Path, depth: u32, items: &mut Vec<Item>, errors: &mut Vec<String>) {
    let name = path.display().to_string();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("{name}: {e}"));
            return;
        }
    };
    for (i, line) in text.lines().enumerate() {
        match tokenize(line) {
            Err(e) => errors.push(format!("{name}:{}: {e}", i + 1)),
            Ok(w) if w.is_empty() => {}
            Ok(w) if w[0] == "include" => match w.get(1) {
                Some(p) if depth < 8 => {
                    let mut inc = expand_home(p);
                    if inc.is_relative() {
                        inc = path.parent().unwrap_or(Path::new(".")).join(inc);
                    }
                    read(&inc, depth + 1, items, errors);
                }
                Some(_) => errors.push(format!("{name}:{}: include nested too deeply", i + 1)),
                None => errors.push(format!("{name}:{}: include needs a path", i + 1)),
            },
            Ok(w) => items.push((name.clone(), i + 1, w)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_options() {
        let mut c = Config::default();
        assert_eq!((c.recolor.contrast, c.recolor.contrast_vector), (2.5, false));
        c.set("recolor-contrast", "3").unwrap();
        c.set("recolor-contrast-vector", "true").unwrap();
        assert_eq!((c.recolor.contrast, c.recolor.contrast_vector), (3.0, true));
        assert_eq!(c.get("recolor-contrast").as_deref(), Some("3"));
        assert!(c.set("recolor-contrast", "0.5").is_err());
        assert!(c.set("recolor-contrast", "x").is_err());
    }

    #[test]
    fn wheel_options() {
        let mut c = Config::default();
        assert_eq!((c.wheel_step, c.touchpad_scroll_factor, c.wheel_zoom_step), (50.0, 1.0, 0.0));
        c.set("wheel-step", "80").unwrap();
        c.set("touchpad-scroll-factor", "0.5").unwrap();
        c.set("wheel-zoom-step", "5").unwrap();
        assert_eq!((c.wheel_step, c.touchpad_scroll_factor, c.wheel_zoom_step), (80.0, 0.5, 5.0));
        assert!(c.set("wheel-step", "0").is_err());
        assert!(c.set("touchpad-scroll-factor", "abc").is_err());
        assert!(c.set("wheel-zoom-step", "101").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#1e1e2e").unwrap(), [0x1e, 0x1e, 0x2e, 255]);
        assert_eq!(parse_color("#1e1e2e99").unwrap(), [0x1e, 0x1e, 0x2e, 0x99]);
        assert_eq!(parse_color("rgba(255, 0, 10, 0.5)").unwrap(), [255, 0, 10, 128]);
        assert_eq!(parse_color("rgb(1,2,3)").unwrap(), [1, 2, 3, 255]);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("rgba(1,2,3,2)").is_err());
        assert!(parse_color("red").is_err());
    }

    #[test]
    fn tokens_and_quotes() {
        assert_eq!(tokenize("set font \"Fira Code\"  # c").unwrap(), ["set", "font", "Fira Code"]);
        assert_eq!(tokenize("map x 'goto 5'").unwrap(), ["map", "x", "goto 5"]);
        assert_eq!(tokenize("  # only comment").unwrap(), Vec::<String>::new());
        assert_eq!(tokenize("set a#b").unwrap(), ["set", "a#b"]);
        assert_eq!(tokenize("set default-bg #1e1e2e99").unwrap(), ["set", "default-bg", "#1e1e2e99"]);
        assert_eq!(tokenize("a \\ b").unwrap(), ["a", " b"]);
        assert!(tokenize("set a \"oops").is_err());
    }

    #[test]
    fn set_options() {
        let mut c = Config::default();
        c.set("cache-mb", "32").unwrap();
        c.set("recolor", "false").unwrap();
        c.set("default-bg", "rgba(0,0,0,0.5)").unwrap();
        assert_eq!((c.cache_mb, c.recolor.enabled, c.default_bg), (32, false, [0, 0, 0, 128]));
        assert!(c.set("cache-mb", "abc").is_err());
        assert!(c.set("nope", "1").is_err());
        assert_eq!(c.get("statusbar").unwrap(), "true");
        for (n, _) in OPTIONS {
            assert!(c.get(n).is_some(), "{n}");
        }
    }

    #[test]
    fn paper_matching_bg_is_transparent() {
        let c = Config::default();
        assert_eq!(c.effective_recolor().light[3], 0);
        let mut c = c;
        c.set("recolor-lightcolor", "#ffffff").unwrap();
        assert_eq!(c.effective_recolor().light[3], 255);
    }

    #[test]
    fn profiles_restore_base_for_unset_options() {
        let base = Config::default();
        let profs = vec![
            Profile { name: "a".into(), opts: vec![("recolor-keephue".into(), "true".into()), ("default-bg".into(), "#112233".into())] },
            Profile { name: "b".into(), opts: vec![("recolor-darkcolor".into(), "#010203".into())] },
        ];
        let mut c = base.clone();
        assert_eq!(apply_profile(&profs, "a", &base, &mut c), Some(vec![]));
        assert!(c.recolor.keephue);
        assert_eq!(c.default_bg[..3], [0x11, 0x22, 0x33]);
        assert_eq!(apply_profile(&profs, "b", &base, &mut c), Some(vec![]));
        assert!(!c.recolor.keephue, "back to the base value");
        assert_eq!(c.default_bg, base.default_bg);
        assert_eq!(c.recolor.dark, [1, 2, 3]);
        assert!(apply_profile(&profs, "zzz", &base, &mut c).is_none());
        let bad = vec![Profile { name: "x".into(), opts: vec![("nope".into(), "1".into())] }];
        assert_eq!(apply_profile(&bad, "x", &base, &mut c).unwrap().len(), 1);
    }

    #[test]
    fn builtin_profiles_apply() {
        let base = Config::default();
        let profs = builtin_profiles();
        let mut c = base.clone();
        assert_eq!(apply_profile(&profs, "textbook", &base, &mut c), Some(vec![]));
        assert_eq!(c.default_bg, [0, 0, 0, 0]);
        assert_eq!(c.recolor.dark, [255, 255, 255]);
        assert!(!c.recolor.keephue);
        assert_eq!(apply_profile(&profs, "normal", &base, &mut c), Some(vec![]));
        assert!(c.recolor.keephue);
        assert_eq!(c.default_bg, [0x1e, 0x1e, 0x2e, 0x99]);
    }
}
