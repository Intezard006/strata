//! Options and the zathurarc-style config parser. Reload is manual (no file watcher).

use std::path::{Path, PathBuf};

use crate::recolor::Recolor;

pub struct Config {
    pub cache_mb: usize,
    pub cache_max_mb: u64,
    pub default_bg: [u8; 4],
    pub recolor: Recolor,
    pub scroll_step: f32,
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
    ("scroll-step", "Logical pixels per j/k step"),
    ("zoom-step", "Zoom step in percent"),
    ("page-gap", "Gap between pages in points"),
    ("statusbar", "Show the statusbar (true/false)"),
    ("font", "UI font family (resolved with fc-match)"),
    ("font-size", "UI font size in logical pixels"),
    ("incremental-search", "Search while typing (true/false)"),
    ("auto-ocr", "OCR scanned documents in the background (true/false)"),
    ("auto-index", "Generated index: auto (only without outline), always, off"),
    ("auto-analyze", "Analyse documents in a background process (true/false)"),
];

impl Default for Config {
    fn default() -> Self {
        Config {
            cache_mb: 64,
            cache_max_mb: 2048,
            default_bg: [0x1e, 0x1e, 0x2e, 0x99],
            recolor: Recolor { light: [0x1e, 0x1e, 0x2e, 0xff], ..Recolor::default() },
            scroll_step: 40.0,
            zoom_step: 10.0,
            page_gap: 8.0,
            statusbar: true,
            font: "monospace".into(),
            font_size: 13.0,
            incremental_search: true,
            auto_ocr: true,
            auto_index: AutoIndex::Auto,
            auto_analyze: true,
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
            "scroll-step" => self.scroll_step = parse_num(v, 1.0, 2000.0)?,
            "zoom-step" => self.zoom_step = parse_num(v, 1.0, 100.0)?,
            "page-gap" => self.page_gap = parse_num(v, 0.0, 200.0)?,
            "statusbar" => self.statusbar = parse_bool(v)?,
            "font" => self.font = v.to_string(),
            "font-size" => self.font_size = parse_num(v, 6.0, 72.0)?,
            "incremental-search" => self.incremental_search = parse_bool(v)?,
            "auto-ocr" => self.auto_ocr = parse_bool(v)?,
            "auto-analyze" => self.auto_analyze = parse_bool(v)?,
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
            "scroll-step" => self.scroll_step.to_string(),
            "zoom-step" => self.zoom_step.to_string(),
            "page-gap" => self.page_gap.to_string(),
            "statusbar" => self.statusbar.to_string(),
            "font" => self.font.clone(),
            "font-size" => self.font_size.to_string(),
            "incremental-search" => self.incremental_search.to_string(),
            "auto-ocr" => self.auto_ocr.to_string(),
            "auto-analyze" => self.auto_analyze.to_string(),
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
}
