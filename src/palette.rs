//! The shared NERV palette (`$XDG_CONFIG_HOME/nerv/palette.toml`): optional `#RRGGBB` strings, read once.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub field: Option<[u8; 3]>,
    pub signal: Option<[u8; 3]>,
    pub grid: Option<[u8; 3]>,
    pub alarm: Option<[u8; 3]>,
    pub dim: Option<[u8; 3]>,
}

pub const SIGNAL: [u8; 3] = [0xff, 0x83, 0x24];

pub fn palette_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default();
    base.join("nerv").join("palette.toml")
}

/// `None` when the file is absent or unreadable.
pub fn load() -> Option<Palette> {
    load_from(&palette_path())
}

fn load_from(path: &std::path::Path) -> Option<Palette> {
    std::fs::read_to_string(path).ok().map(|s| parse(&s))
}

fn hex(v: &str) -> Option<[u8; 3]> {
    let h = v.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

/// Flat `key = "#RRGGBB"` lines; unknown keys, tables, comments and invalid values are ignored.
pub fn parse(src: &str) -> Palette {
    let mut p = Palette::default();
    for line in src.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        // the value is quoted; anything after the closing quote (a trailing comment) is dropped
        let Some(rest) = v.strip_prefix('"').or_else(|| v.strip_prefix('\'')) else { continue };
        let Some(end) = rest.find(['"', '\'']) else { continue };
        let Some(c) = hex(&rest[..end]) else { continue };
        match k.trim() {
            "field" => p.field = Some(c),
            "signal" => p.signal = Some(c),
            "grid" => p.grid = Some(c),
            "alarm" => p.alarm = Some(c),
            "dim" => p.dim = Some(c),
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none() {
        assert!(load_from(std::path::Path::new("/nonexistent-strata-test/palette.toml")).is_none());
    }

    #[test]
    fn partial_file() {
        let p = parse("signal = \"#00ff10\"\ngrid = \"#4CE8A8\"\n");
        assert_eq!(p.signal, Some([0, 0xff, 0x10]));
        assert_eq!(p.grid, Some([0x4c, 0xe8, 0xa8]));
        assert_eq!((p.field, p.alarm, p.dim), (None, None, None));
    }

    #[test]
    fn bad_values_ignored() {
        let p = parse("field = \"red\"\nsignal = \"#12345\"\nalarm = \"#GG0000\"\ndim = 5\ngrid = \"#abcdef\"\n[x]\nfoo");
        assert_eq!(p, Palette { grid: Some([0xab, 0xcd, 0xef]), ..Palette::default() });
    }

    #[test]
    fn comments() {
        let p = parse("# signal = \"#111111\"\n  # more\ndim = \"#a8521a\" # trailing\n\nfield = '#000001'\n");
        assert_eq!(p.signal, None);
        assert_eq!(p.dim, Some([0xa8, 0x52, 0x1a]));
        assert_eq!(p.field, Some([0, 0, 1]));
    }
}
