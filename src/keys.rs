//! Commands, the command registry (names/descriptions for `:` and help) and the keymap.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Normal,
    Index,
    Popup,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Normal, Mode::Index, Mode::Popup];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Index => "index",
            Mode::Popup => "popup",
        }
    }

    pub fn from_name(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
    HalfPageDown,
    HalfPageUp,
    NextPage,
    PrevPage,
    /// First page, or page N with a count.
    FirstPage,
    /// Last page, or page N with a count.
    LastPage,
    /// Raw argument: `120`, `#120` or `xii` (resolved by the app against the page map).
    Goto(String),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Zoom to a percentage.
    Zoom(f32),
    FitWidth,
    FitPage,
    ToggleRecolor,
    /// Two-page spreads on or off.
    Book,
    /// Presentation: fullscreen, one page at a time.
    Present,
    /// Cycle the recolor/keep override of the image under the cursor (or the largest on screen).
    ImageToggle,
    ToggleStatusbar,
    Quit,
    CommandMode,
    SearchForward,
    SearchBackward,
    Search(String),
    BSearch(String),
    SearchNext,
    SearchPrev,
    ClearHighlight,
    Follow,
    JumpBack,
    JumpForward,
    Help,
    Index,
    IndexDown,
    IndexUp,
    IndexExpand,
    IndexCollapse,
    IndexSelect,
    IndexClose,
    IndexFirst,
    IndexLast,
    /// List of all previewable targets.
    Previews,
    PreviewHints,
    /// `m`: the next key names a mark to set.
    SetMarkPrompt,
    /// `'`: the next key names a mark to jump to.
    JumpMarkPrompt,
    Bmark(String),
    Blist,
    Palette,
    Profile(String),
    ProfileCycle,
    Float,
    Bdelete(String),
    Bjump(String),
    Recent,
    SynctexView(String, u32, u32),
    /// Mark commands reached by D-Bus/prompt: set (true) or jump (false) to a mark.
    Mark(bool, char),
    PopupClose,
    PopupJump,
    PopupDown,
    PopupUp,
    PopupZoomIn,
    PopupZoomOut,
    PopupYank,
    Set(String, Option<String>),
    Map(Mode, String, String),
    Unmap(Mode, String),
    Open(String),
    Reload,
    ReloadConfig,
    Ocr,
    OcrCancel,
    Reindex,
    WriteIndex(Option<String>, bool),
}

/// `:` command registry: (name, argument synopsis, description).
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("scroll-down", "", "Scroll down"),
    ("scroll-up", "", "Scroll up"),
    ("scroll-left", "", "Scroll left"),
    ("scroll-right", "", "Scroll right"),
    ("half-page-down", "", "Scroll half a page down"),
    ("half-page-up", "", "Scroll half a page up"),
    ("next-page", "", "Next page"),
    ("prev-page", "", "Previous page"),
    ("first-page", "", "First page (or page N with a count)"),
    ("last-page", "", "Last page (or page N with a count)"),
    ("goto", "N|#N|ROMAN", "Go to printed page N (#N: PDF page, xii: roman front matter)"),
    ("zoom-in", "", "Zoom in"),
    ("zoom-out", "", "Zoom out"),
    ("zoom-reset", "", "Reset zoom to 100%"),
    ("zoom", "PERCENT", "Set zoom to PERCENT"),
    ("fit-width", "", "Fit page width"),
    ("fit-page", "", "Fit whole page"),
    ("toggle-recolor", "", "Toggle recolor"),
    ("present", "", "Presentation: fullscreen, one page at fit-page, no statusbar; j/k/Space/BackSpace/arrows/click change page, Esc/q/F5 end (option presentation-bg)"),
    ("book", "", "Toggle book mode: two-page spreads (options first-page-single, book-rtl); J/K move by spread"),
    ("image-toggle", "", "Cycle the image under the cursor (else the largest on screen) through keep / recolor / auto; saved per document"),
    ("toggle-statusbar", "", "Show or hide the statusbar"),
    ("quit", "", "Quit"),
    ("command-mode", "", "Open the command line"),
    ("search-forward", "", "Start a forward search"),
    ("search-backward", "", "Start a backward search"),
    ("search", "TEXT", "Search forward for TEXT"),
    ("bsearch", "TEXT", "Search backward for TEXT"),
    ("search-next", "", "Next search hit"),
    ("search-prev", "", "Previous search hit"),
    ("nohl", "", "Clear search highlights"),
    ("follow", "", "Show link hints and follow a link"),
    ("jump-back", "", "Jump back in the jumplist"),
    ("jump-forward", "", "Jump forward in the jumplist"),
    ("help", "", "Show the help overlay"),
    ("index", "", "Toggle the index (outline) panel"),
    ("index-down", "", "Index: move down"),
    ("index-up", "", "Index: move up"),
    ("index-expand", "", "Index: expand entry"),
    ("index-collapse", "", "Index: collapse entry"),
    ("index-select", "", "Index: jump to entry"),
    ("index-close", "", "Index: close panel"),
    ("index-first", "", "Index: first entry"),
    ("index-last", "", "Index: last entry"),
    ("previews", "", "List all equations, theorems, figures... and preview or jump to one"),
    ("preview-hints", "", "Show hint labels over references; type one to preview its target"),
    ("mark-prompt", "", "Set a mark: the next key a-z (local) or A-Z (global) names it"),
    ("jump-mark-prompt", "", "Jump to a mark: the next key names it"),
    ("bmark", "NAME", "Bookmark the current position as NAME"),
    ("palette", "", "Command palette: commands, index, bookmarks, previews, recent files (prefixes > # @ = %)"),
    ("profile", "NAME", "Apply the look profile NAME (defined with `profile NAME OPTION VALUE` in stratarc)"),
    ("profile-cycle", "", "Switch to the next look profile"),
    ("float", "", "Open (or close) a small floating window with the current page or preview target"),
    ("blist", "", "List bookmarks (type to filter, Enter jumps)"),
    ("bdelete", "NAME", "Delete the bookmark NAME"),
    ("bjump", "NAME", "Jump to the bookmark NAME"),
    ("recent", "", "List recently opened files (Enter opens)"),
    ("mark", "KEY", "Set mark KEY (a-z local, A-Z global); same as m KEY"),
    ("jump-mark", "KEY", "Jump to mark KEY; same as ' KEY"),
    ("synctex-view", "LINE:COL:FILE", "Forward search: show the PDF position of a TeX source line"),
    ("popup-close", "", "Preview: close the popup"),
    ("popup-jump", "", "Preview: jump to the target (Ctrl-o returns)"),
    ("popup-down", "", "Preview: scroll down"),
    ("popup-up", "", "Preview: scroll up"),
    ("popup-zoom-in", "", "Preview: zoom in"),
    ("popup-zoom-out", "", "Preview: zoom out"),
    ("popup-yank", "", "Preview: copy the target's text to the clipboard (wl-copy)"),
    ("set", "OPTION [VALUE]", "Show or change an option"),
    ("map", "[MODE] KEYS COMMAND", "Bind KEYS to COMMAND"),
    ("unmap", "[MODE] KEYS", "Remove a binding"),
    ("open", "PATH", "Open another file"),
    ("reload", "", "Reload the current file"),
    ("reload-config", "", "Re-read the config file"),
    ("source", "", "Re-read the config file"),
    ("ocr", "", "Run OCR on this document (text layer for search and index)"),
    ("ocr-cancel", "", "Cancel a running OCR / analysis job"),
    ("reindex", "", "Recompute the generated index"),
    ("writeindex", "[PATH]", "Write a copy of the PDF with the generated index as outline"),
    ("writeindex!", "[PATH]", "Like writeindex, overwriting an existing file"),
];

pub fn describe(name: &str) -> &'static str {
    COMMANDS.iter().find(|c| c.0 == name).map_or("", |c| c.2)
}

/// Parse a command line such as `goto 5` or `map x zoom-in`.
pub fn parse(line: &str) -> Result<Command, String> {
    let words = crate::config::tokenize(line)?;
    let Some((name, args)) = words.split_first() else { return Err("empty command".into()) };
    parse_words(name, args)
}

pub fn parse_words(name: &str, args: &[String]) -> Result<Command, String> {
    let one = || args.first().cloned().ok_or_else(|| format!("{name}: missing argument"));
    let rest = || {
        if args.is_empty() { Err(format!("{name}: missing argument")) } else { Ok(args.join(" ")) }
    };
    use Command::*;
    Ok(match name {
        "scroll-down" => ScrollDown,
        "scroll-up" => ScrollUp,
        "scroll-left" => ScrollLeft,
        "scroll-right" => ScrollRight,
        "half-page-down" => HalfPageDown,
        "half-page-up" => HalfPageUp,
        "next-page" => NextPage,
        "prev-page" => PrevPage,
        "first-page" => FirstPage,
        "last-page" => LastPage,
        "goto" => {
            let a = one()?;
            crate::pages::parse_goto(&a)?;
            Goto(a)
        }
        "zoom-in" => ZoomIn,
        "zoom-out" => ZoomOut,
        "zoom-reset" => ZoomReset,
        "zoom" => Zoom(one()?.trim_end_matches('%').parse().map_err(|_| "zoom: not a number".to_string())?),
        "fit-width" => FitWidth,
        "fit-page" => FitPage,
        "toggle-recolor" => ToggleRecolor,
        "book" => Book,
        "present" => Present,
        "image-toggle" => ImageToggle,
        "toggle-statusbar" => ToggleStatusbar,
        "quit" | "q" => Quit,
        "command-mode" => CommandMode,
        "search-forward" => SearchForward,
        "search-backward" => SearchBackward,
        "search" => Search(rest()?),
        "bsearch" => BSearch(rest()?),
        "search-next" => SearchNext,
        "search-prev" => SearchPrev,
        "nohl" => ClearHighlight,
        "follow" => Follow,
        "jump-back" => JumpBack,
        "jump-forward" => JumpForward,
        "help" => Help,
        "index" => Index,
        "index-down" => IndexDown,
        "index-up" => IndexUp,
        "index-expand" => IndexExpand,
        "index-collapse" => IndexCollapse,
        "index-select" => IndexSelect,
        "index-close" => IndexClose,
        "index-first" => IndexFirst,
        "index-last" => IndexLast,
        "previews" => Previews,
        "preview-hints" => PreviewHints,
        "mark-prompt" => SetMarkPrompt,
        "jump-mark-prompt" => JumpMarkPrompt,
        "bmark" => Bmark(rest()?),
        "blist" => Blist,
        "profile" => Profile(rest()?),
        "profile-cycle" => ProfileCycle,
        "float" => Float,
        "palette" => Palette,
        "bdelete" => Bdelete(rest()?),
        "bjump" => Bjump(rest()?),
        "recent" => Recent,
        "mark" | "jump-mark" => {
            let c = one()?;
            let mut cs = c.chars();
            match (cs.next(), cs.next()) {
                (Some(ch), None) if ch.is_ascii_alphabetic() => Mark(name == "mark", ch),
                _ => return Err(format!("{name}: expected one letter")),
            }
        }
        "synctex-view" => {
            let a = rest()?;
            let mut p = a.splitn(3, ':');
            let (l, c, f) = (p.next().and_then(|v| v.parse().ok()), p.next().and_then(|v| v.parse().ok()), p.next());
            match (l, c, f) {
                (Some(l), Some(c), Some(f)) => SynctexView(f.to_string(), l, c),
                _ => return Err("synctex-view: expected LINE:COL:FILE".into()),
            }
        }
        "popup-close" => PopupClose,
        "popup-jump" => PopupJump,
        "popup-down" => PopupDown,
        "popup-up" => PopupUp,
        "popup-zoom-in" => PopupZoomIn,
        "popup-zoom-out" => PopupZoomOut,
        "popup-yank" => PopupYank,
        "set" => Set(one()?, args.get(1).cloned()),
        "map" | "unmap" => {
            let (mode, a) = match args.first().and_then(|m| Mode::from_name(m)) {
                Some(m) => (m, &args[1..]),
                None => (Mode::Normal, args),
            };
            let keys = a.first().cloned().ok_or_else(|| format!("{name}: missing keys"))?;
            if name == "unmap" {
                Unmap(mode, keys)
            } else if a.len() < 2 {
                return Err("map: missing command".into());
            } else {
                let cmd = a[1..].join(" ");
                parse(&cmd)?; // validate now
                Map(mode, keys, cmd)
            }
        }
        "ocr" => Ocr,
        "ocr-cancel" => OcrCancel,
        "reindex" => Reindex,
        "writeindex" => WriteIndex(args.first().cloned(), false),
        "writeindex!" => WriteIndex(args.first().cloned(), true),
        "open" => Open(rest()?),
        "reload" => Reload,
        "reload-config" | "source" => ReloadConfig,
        _ => return Err(format!("not a command: {name}")),
    })
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub keys: String,
    pub mode: Mode,
    pub command: Command,
    /// Command line text as written (for the help overlay).
    pub text: String,
    pub description: String,
}

/// Key tokens: plain characters ("j", "G", "+"), "<C-x>", "<Space>", "<S-Space>", "<CR>", "<Tab>", "<Esc>", arrows ("<Up>"), "<F1>".
const DEFAULTS: &[(Mode, &str, &str)] = &[
    (Mode::Normal, "j", "scroll-down"),
    (Mode::Normal, "k", "scroll-up"),
    (Mode::Normal, "h", "scroll-left"),
    (Mode::Normal, "l", "scroll-right"),
    (Mode::Normal, "<Down>", "scroll-down"),
    (Mode::Normal, "<Up>", "scroll-up"),
    (Mode::Normal, "<Left>", "scroll-left"),
    (Mode::Normal, "<Right>", "scroll-right"),
    (Mode::Normal, "<C-d>", "half-page-down"),
    (Mode::Normal, "<C-u>", "half-page-up"),
    (Mode::Normal, "J", "next-page"),
    (Mode::Normal, "K", "prev-page"),
    (Mode::Normal, "<Space>", "next-page"),
    (Mode::Normal, "<S-Space>", "prev-page"),
    (Mode::Normal, "gg", "first-page"),
    (Mode::Normal, "G", "last-page"),
    (Mode::Normal, "+", "zoom-in"),
    (Mode::Normal, "-", "zoom-out"),
    (Mode::Normal, "=", "zoom-reset"),
    (Mode::Normal, "s", "fit-width"),
    (Mode::Normal, "a", "fit-page"),
    (Mode::Normal, "<C-r>", "toggle-recolor"),
    (Mode::Normal, "d", "book"),
    (Mode::Normal, "<F5>", "present"),
    (Mode::Normal, "<C-n>", "toggle-statusbar"),
    (Mode::Normal, ":", "command-mode"),
    (Mode::Normal, "/", "search-forward"),
    (Mode::Normal, "?", "search-backward"),
    (Mode::Normal, "n", "search-next"),
    (Mode::Normal, "N", "search-prev"),
    (Mode::Normal, "<Esc>", "nohl"),
    (Mode::Normal, "f", "follow"),
    (Mode::Normal, "<C-o>", "jump-back"),
    (Mode::Normal, "<C-i>", "jump-forward"),
    (Mode::Normal, "<F1>", "help"),
    (Mode::Normal, "<Tab>", "index"),
    (Mode::Normal, "q", "quit"),
    (Mode::Normal, "p", "preview-hints"),
    (Mode::Normal, "gp", "previews"),
    (Mode::Normal, "m", "mark-prompt"),
    (Mode::Normal, "'", "jump-mark-prompt"),
    (Mode::Normal, "gb", "blist"),
    (Mode::Normal, "gf", "float"),
    (Mode::Normal, "gi", "image-toggle"),
    (Mode::Normal, "<C-t>", "profile-cycle"),
    (Mode::Normal, "<C-p>", "palette"),
    (Mode::Popup, "<CR>", "popup-jump"),
    (Mode::Popup, "o", "popup-jump"),
    (Mode::Popup, "j", "popup-down"),
    (Mode::Popup, "k", "popup-up"),
    (Mode::Popup, "<Down>", "popup-down"),
    (Mode::Popup, "<Up>", "popup-up"),
    (Mode::Popup, "+", "popup-zoom-in"),
    (Mode::Popup, "=", "popup-zoom-in"),
    (Mode::Popup, "-", "popup-zoom-out"),
    (Mode::Popup, "y", "popup-yank"),
    (Mode::Popup, "q", "popup-close"),
    (Mode::Popup, "<Esc>", "popup-close"),
    (Mode::Index, "j", "index-down"),
    (Mode::Index, "k", "index-up"),
    (Mode::Index, "<Down>", "index-down"),
    (Mode::Index, "<Up>", "index-up"),
    (Mode::Index, "l", "index-expand"),
    (Mode::Index, "<Right>", "index-expand"),
    (Mode::Index, "h", "index-collapse"),
    (Mode::Index, "<Left>", "index-collapse"),
    (Mode::Index, "<CR>", "index-select"),
    (Mode::Index, "gg", "index-first"),
    (Mode::Index, "G", "index-last"),
    (Mode::Index, "<Tab>", "index-close"),
    (Mode::Index, "<Esc>", "index-close"),
    (Mode::Index, "q", "index-close"),
];

pub struct Keymap {
    pub bindings: Vec<Binding>,
}

#[derive(Debug, PartialEq)]
pub enum Match {
    Exact(Command),
    /// A longer binding starts with the pending keys.
    Prefix,
    None,
}

impl Keymap {
    pub fn with_defaults() -> Self {
        let mut km = Keymap { bindings: Vec::new() };
        for &(mode, keys, text) in DEFAULTS {
            km.bind(mode, keys, text).expect("default binding parses");
        }
        km
    }

    pub fn bind(&mut self, mode: Mode, keys: &str, text: &str) -> Result<(), String> {
        let command = parse(text)?;
        let name = text.split_whitespace().next().unwrap_or("");
        let b = Binding { keys: keys.into(), mode, command, text: text.into(), description: describe(name).into() };
        match self.bindings.iter_mut().find(|x| x.mode == mode && x.keys == keys) {
            Some(slot) => *slot = b,
            None => self.bindings.push(b),
        }
        Ok(())
    }

    pub fn unbind(&mut self, mode: Mode, keys: &str) -> bool {
        let n = self.bindings.len();
        self.bindings.retain(|b| !(b.mode == mode && b.keys == keys));
        n != self.bindings.len()
    }

    pub fn lookup(&self, mode: Mode, pending: &str) -> Match {
        let mut prefix = false;
        for b in self.bindings.iter().filter(|b| b.mode == mode) {
            if b.keys == pending {
                return Match::Exact(b.command.clone());
            }
            prefix |= b.keys.starts_with(pending);
        }
        if prefix { Match::Prefix } else { Match::None }
    }
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

    /// Text for the statusbar: count and partial key sequence.
    pub fn display(&self) -> String {
        format!("{}{}", self.count.map(|c| c.to_string()).unwrap_or_default(), self.pending)
    }

    /// Feed one key token. Returns a command and its count when a binding completes.
    pub fn feed(&mut self, km: &Keymap, mode: Mode, token: &str) -> Option<(Command, Option<u32>)> {
        if self.pending.is_empty() && token.len() == 1 {
            let d = token.as_bytes()[0];
            if d.is_ascii_digit() && (d != b'0' || self.count.is_some()) {
                let n = self.count.unwrap_or(0).saturating_mul(10).saturating_add((d - b'0') as u32);
                self.count = Some(n.min(1_000_000));
                return None;
            }
        }
        self.pending.push_str(token);
        match km.lookup(mode, &self.pending) {
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
        let km = Keymap::with_defaults();
        let mut k = KeyState::default();
        let n = Mode::Normal;
        assert_eq!(k.feed(&km, n, "5"), None);
        assert_eq!(k.feed(&km, n, "j"), Some((Command::ScrollDown, Some(5))));
        assert_eq!(k.feed(&km, n, "2"), None);
        assert_eq!(k.feed(&km, n, "0"), None);
        assert_eq!(k.feed(&km, n, "G"), Some((Command::LastPage, Some(20))));
        assert_eq!(k.feed(&km, n, "g"), None);
        assert_eq!(k.display(), "g");
        assert_eq!(k.feed(&km, n, "g"), Some((Command::FirstPage, None)));
        assert_eq!(k.feed(&km, n, "g"), None);
        assert_eq!(k.feed(&km, n, "x"), None);
        assert_eq!(k.feed(&km, n, "q"), Some((Command::Quit, None)));
    }

    #[test]
    fn map_unmap_and_modes() {
        let mut km = Keymap::with_defaults();
        km.bind(Mode::Normal, "x", "goto 5").unwrap();
        assert_eq!(km.lookup(Mode::Normal, "x"), Match::Exact(Command::Goto("5".into())));
        assert_eq!(km.lookup(Mode::Index, "x"), Match::None);
        km.bind(Mode::Normal, "j", "scroll-up").unwrap(); // override
        assert_eq!(km.lookup(Mode::Normal, "j"), Match::Exact(Command::ScrollUp));
        assert!(km.unbind(Mode::Normal, "j"));
        assert_eq!(km.lookup(Mode::Normal, "j"), Match::None);
        assert!(km.bind(Mode::Normal, "y", "bogus").is_err());
    }

    #[test]
    fn parse_commands() {
        assert_eq!(parse("goto 7").unwrap(), Command::Goto("7".into()));
        assert_eq!(parse("set cache-mb 32").unwrap(), Command::Set("cache-mb".into(), Some("32".into())));
        assert_eq!(parse("map index j index-down").unwrap(), Command::Map(Mode::Index, "j".into(), "index-down".into()));
        assert_eq!(parse("map <C-x> goto 3").unwrap(), Command::Map(Mode::Normal, "<C-x>".into(), "goto 3".into()));
        assert!(parse("goto zz").is_err());
        assert!(parse("map x nonsense").is_err());
    }

    #[test]
    fn registry_is_consistent() {
        for (i, a) in COMMANDS.iter().enumerate() {
            assert!(!a.2.is_empty());
            assert!(COMMANDS[i + 1..].iter().all(|c| c.0 != a.0), "dup {}", a.0);
        }
        let km = Keymap::with_defaults();
        for (i, a) in km.bindings.iter().enumerate() {
            assert!(!a.description.is_empty(), "{}", a.text);
            assert!(km.bindings[i + 1..].iter().all(|c| !(c.mode == a.mode && c.keys == a.keys)));
        }
    }
}
