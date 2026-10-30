//! Keybinding table and sequence matching. Phase 2 derives the help overlay and
//! config `map` handling from `BINDINGS`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
    HalfPageDown,
    HalfPageUp,
    NextPage,
    PrevPage,
    /// `gg`: first page, or page N with a count.
    GotoFirst,
    /// `G`: last page, or page N with a count.
    GotoLast,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    FitWidth,
    FitPage,
    ToggleRecolor,
    Quit,
}

#[allow(dead_code)] // `description` is consumed by the phase 2 help overlay
pub struct Binding {
    pub keys: &'static str,
    pub mode: Mode,
    pub command: Command,
    pub description: &'static str,
}

const fn b(keys: &'static str, command: Command, description: &'static str) -> Binding {
    Binding { keys, mode: Mode::Normal, command, description }
}

/// Key tokens: plain characters ("j", "G", "+"), "<C-x>" for Ctrl, "<Space>", "<S-Space>".
pub const BINDINGS: &[Binding] = &[
    b("j", Command::ScrollDown, "Scroll down"),
    b("k", Command::ScrollUp, "Scroll up"),
    b("h", Command::ScrollLeft, "Scroll left"),
    b("l", Command::ScrollRight, "Scroll right"),
    b("<C-d>", Command::HalfPageDown, "Scroll half a page down"),
    b("<C-u>", Command::HalfPageUp, "Scroll half a page up"),
    b("J", Command::NextPage, "Next page"),
    b("K", Command::PrevPage, "Previous page"),
    b("<Space>", Command::NextPage, "Next page"),
    b("<S-Space>", Command::PrevPage, "Previous page"),
    b("gg", Command::GotoFirst, "First page (or page N with a count)"),
    b("G", Command::GotoLast, "Last page (or page N with a count)"),
    b("+", Command::ZoomIn, "Zoom in"),
    b("-", Command::ZoomOut, "Zoom out"),
    b("=", Command::ZoomReset, "Reset zoom"),
    b("s", Command::FitWidth, "Fit width"),
    b("a", Command::FitPage, "Fit page"),
    b("<C-r>", Command::ToggleRecolor, "Toggle recolor"),
    b("q", Command::Quit, "Quit"),
];

#[derive(Debug, PartialEq, Eq)]
pub enum Match {
    Exact(Command),
    /// A longer binding starts with the pending keys.
    Prefix,
    None,
}

/// Look up the pending key sequence (tokens concatenated) in `mode`.
pub fn lookup(mode: Mode, pending: &str) -> Match {
    let mut prefix = false;
    for bd in BINDINGS.iter().filter(|bd| bd.mode == mode) {
        if bd.keys == pending {
            return Match::Exact(bd.command);
        }
        prefix |= bd.keys.starts_with(pending);
    }
    if prefix { Match::Prefix } else { Match::None }
}

/// Accumulates counts and key tokens between commands.
#[derive(Default)]
pub struct KeyState {
    pending: String,
    count: Option<u32>,
}

impl KeyState {
    pub fn clear(&mut self) {
        self.pending.clear();
        self.count = None;
    }

    /// Feed one key token. Returns a command and its count when a binding completes.
    pub fn feed(&mut self, mode: Mode, token: &str) -> Option<(Command, Option<u32>)> {
        if self.pending.is_empty() && token.len() == 1 {
            let d = token.as_bytes()[0];
            if d.is_ascii_digit() && (d != b'0' || self.count.is_some()) {
                let n = self.count.unwrap_or(0).saturating_mul(10).saturating_add((d - b'0') as u32);
                self.count = Some(n.min(1_000_000));
                return None;
            }
        }
        self.pending.push_str(token);
        match lookup(mode, &self.pending) {
            Match::Exact(cmd) => {
                let count = self.count;
                self.clear();
                Some((cmd, count))
            }
            Match::Prefix => None,
            Match::None => {
                self.clear();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_sequences() {
        let mut k = KeyState::default();
        assert_eq!(k.feed(Mode::Normal, "5"), None);
        assert_eq!(k.feed(Mode::Normal, "j"), Some((Command::ScrollDown, Some(5))));
        assert_eq!(k.feed(Mode::Normal, "2"), None);
        assert_eq!(k.feed(Mode::Normal, "0"), None);
        assert_eq!(k.feed(Mode::Normal, "G"), Some((Command::GotoLast, Some(20))));
        assert_eq!(k.feed(Mode::Normal, "g"), None);
        assert_eq!(k.feed(Mode::Normal, "g"), Some((Command::GotoFirst, None)));
        assert_eq!(k.feed(Mode::Normal, "g"), None);
        assert_eq!(k.feed(Mode::Normal, "x"), None); // aborted sequence
        assert_eq!(k.feed(Mode::Normal, "q"), Some((Command::Quit, None)));
    }

    #[test]
    fn no_duplicate_bindings() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for c in &BINDINGS[i + 1..] {
                assert!(!(a.mode == c.mode && a.keys == c.keys), "{}", a.keys);
            }
        }
    }
}
