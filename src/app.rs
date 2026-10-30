//! winit handler and view state. Everything is event driven (`ControlFlow::Wait`).

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use crate::cache::{self, Analysis, OcrState};
use crate::config::{self, premultiply, AutoIndex, Config};
use crate::keys::{self, Command, KeyState, Keymap, Mode};
use crate::layout::{page_px, tile_grid, Layout};
use crate::links::{hint_labels, open_external, Jumplist, LinkInfo, Target, HINT_ALPHABET};
use crate::recolor::Recolor;
use crate::previews::{Kind, Ref};
use crate::pstore::Store;
use crate::render::{Cmd, PreviewReq, RenderHandle, Request, Response, SearchReq, Tile, TileCache, TileKey};
use crate::search::{case_sensitive, Hit, Search};
use crate::text::TextRenderer;
use crate::ui::help::Help;
use crate::ui::pick::{self, Pick, PickItem, PickKind};
use crate::ui::plist::{self, PList};
use crate::db::{self, Db};
use crate::ext::{self, Ext, SharedInfo};
use crate::synctex;
use crate::ui::popup::{self, Popup};
use crate::ui::index::{self, Index, Item};
use crate::worker::{Job, Msg};
use crate::ui::input::{History, InputBar};
use crate::ui::statusbar::{self, Status};
use crate::ui::{over, pm, Canvas, FG};

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static FIRST_TILE: std::sync::Once = std::sync::Once::new();

fn debug() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var_os("STRATA_DEBUG").is_some())
}

const WHEEL_PX: f32 = 50.0; // logical px per wheel notch
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;
const HIT_COLOR: [u8; 3] = [0xf9, 0xe2, 0xaf];
const CUR_HIT_COLOR: [u8; 3] = [0xfa, 0xb3, 0x87];

enum Overlay {
    None,
    Input(InputBar),
    Help(Help),
    Hints(Hints),
    List(PList),
    Pick(Pick),
}

struct HintItem {
    page: u32,
    rect: [f32; 4],
    label: String,
    target: Target,
    /// Preview hints open this target's popup instead of following `target`.
    pv: Option<u32>,
}

struct Hints {
    typed: String,
    items: Vec<HintItem>,
}

struct Placed {
    key: TileKey,
    x: i64,
    y: i64,
}

/// Config values already pushed into the running state.
struct Applied {
    recolor: Recolor,
    gap: f32,
    font: (String, f32),
    auto_index: AutoIndex,
}

pub struct App {
    render: RenderHandle,
    layout: Layout,
    path: PathBuf,
    name: String,
    cfg: Config,
    applied: Applied,
    keymap: Keymap,
    keys: KeyState,
    mode: Mode,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    cache: TileCache,
    pending: HashSet<TileKey>,
    text: Option<TextRenderer>,
    bg: u32,
    mods: ModifiersState,
    cursor: (f64, f64),
    zoom: f32,
    dpi: f32,
    vp: (u32, u32),
    /// Scroll offset in points; `sx` only matters when the content is wider than the view.
    sx: f32,
    sy: f32,
    title_page: usize,
    message: Option<(String, bool)>,
    overlay: Overlay,
    hist: History,
    index: Index,
    help_rows: usize,
    index_rows: usize,
    links: HashMap<u32, Vec<LinkInfo>>,
    links_pending: HashSet<u32>,
    search: Option<Search>,
    search_id: u64,
    highlight: bool,
    pre_search_sy: f32,
    jumps: Jumplist,
    /// Set while an `open` is in flight: the new path, and the scroll to keep on reload.
    opening: Option<(PathBuf, Option<f32>)>,
    proxy: EventLoopProxy<()>,
    worker_tx: Sender<(u64, Msg)>,
    worker_rx: Receiver<(u64, Msg)>,
    job: Option<(Job, JobKind)>,
    job_seq: u64,
    job_status: Option<String>,
    outline_items: Vec<Item>,
    auto_items: Vec<Item>,
    index_auto: Option<&'static str>,
    analysis: Option<Analysis>,
    pv: Option<Store>,
    pv_refs: HashMap<u32, Vec<Ref>>,
    popup: Option<Popup>,
    popup_seq: u64,
    /// The reference under the mouse (page, index on page) and when its popup should open.
    hover: popup::HoverTimer,
    yank_pending: bool,
    db: Option<Db>,
    /// Cache key of the open document (the database key).
    key: Option<String>,
    info: SharedInfo,
    ext_rx: Receiver<Ext>,
    /// After `m` (true) or `'` (false): the next key names the mark.
    mark_prompt: Option<bool>,
    /// A position to go to once the document in `opening` is loaded (global marks).
    open_goto: Option<(u32, f32)>,
    /// SyncTeX target rectangle (page, rect); stays until the next key.
    flash: Option<(u32, [f32; 4])>,
    /// A forward search to run once the window is up (`--synctex-forward` on a fresh start).
    start_synctex: Option<(String, u32, u32)>,
}

#[derive(Clone, Debug)]
enum JobKind {
    Analyze,
    WriteIndex(PathBuf),
}

impl App {
    pub fn new(render: RenderHandle, sizes: Vec<(f32, f32)>, path: PathBuf, proxy: EventLoopProxy<()>, start_synctex: Option<(String, u32, u32)>) -> Self {
        START.get_or_init(Instant::now);
        let cfg = Config::default();
        let (worker_tx, worker_rx) = channel();
        let (ext_tx, ext_rx) = channel();
        let info: SharedInfo = Default::default();
        ext::spawn_signals(ext_tx.clone(), proxy.clone());
        ext::spawn_dbus(ext_tx, proxy.clone(), info.clone());
        let db = Db::open(&db::db_path()).ok();
        let key = cache::key(&path).ok();
        if let (Some(d), Some(k)) = (&db, &key) {
            d.note_open(k, &path);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut app = App {
            render,
            layout: Layout::new(sizes, cfg.page_gap),
            path,
            name,
            applied: Applied { recolor: cfg.effective_recolor(), gap: cfg.page_gap, font: (String::new(), 0.0), auto_index: cfg.auto_index },
            bg: premultiply(cfg.default_bg),
            cache: TileCache::new(cfg.cache_mb << 20),
            cfg,
            keymap: Keymap::with_defaults(),
            keys: KeyState::default(),
            mode: Mode::Normal,
            window: None,
            surface: None,
            pending: HashSet::new(),
            text: None,
            mods: ModifiersState::empty(),
            cursor: (0.0, 0.0),
            zoom: 1.0,
            dpi: 1.0,
            vp: (0, 0),
            sx: 0.0,
            sy: 0.0,
            title_page: usize::MAX,
            message: None,
            overlay: Overlay::None,
            hist: History::default(),
            index: Index::default(),
            help_rows: 20,
            index_rows: 20,
            links: HashMap::new(),
            links_pending: HashSet::new(),
            search: None,
            search_id: 0,
            highlight: false,
            pre_search_sy: 0.0,
            jumps: Jumplist::default(),
            opening: None,
            proxy,
            worker_tx,
            worker_rx,
            job: None,
            job_seq: 0,
            job_status: None,
            outline_items: Vec::new(),
            auto_items: Vec::new(),
            index_auto: None,
            pv: None,
            pv_refs: HashMap::new(),
            popup: None,
            popup_seq: 0,
            hover: popup::HoverTimer::default(),
            yank_pending: false,
            analysis: None,
            db,
            key,
            info,
            ext_rx,
            mark_prompt: None,
            open_goto: None,
            flash: None,
            start_synctex,
        };
        app.load_config();
        app.sync_info();
        app
    }

    // ---- config -------------------------------------------------------------------------

    fn load_config(&mut self) {
        self.cfg = Config::default();
        self.keymap = Keymap::with_defaults();
        let path = config::config_path();
        let mut errors = Vec::new();
        if path.exists() {
            let mut items = Vec::new();
            config::read(&path, 0, &mut items, &mut errors);
            for (file, line, w) in items {
                let r = match w[0].as_str() {
                    "set" | "map" | "unmap" => keys::parse_words(&w[0], &w[1..]).and_then(|c| self.apply_setting(c)),
                    other => Err(format!("unknown directive: {other}")),
                };
                if let Err(e) = r {
                    errors.push(format!("{file}:{line}: {e}"));
                }
            }
        }
        self.apply_config();
        if let Some(first) = errors.first() {
            let more = if errors.len() > 1 { format!(" (+{} more)", errors.len() - 1) } else { String::new() };
            self.err(format!("{first}{more}"));
        }
    }

    /// `set`, `map` and `unmap` — shared by the config file and the `:` line.
    fn apply_setting(&mut self, c: Command) -> Result<(), String> {
        match c {
            Command::Set(name, Some(v)) => self.cfg.set(&name, &v),
            Command::Set(name, None) => match self.cfg.get(&name) {
                Some(v) => {
                    self.msg(format!("{name} = {v}"));
                    Ok(())
                }
                None => Err(format!("unknown option: {name}")),
            },
            Command::Map(mode, keys, cmd) => self.keymap.bind(mode, &keys, &cmd),
            Command::Unmap(mode, keys) => {
                if self.keymap.unbind(mode, &keys) { Ok(()) } else { Err(format!("no such mapping: {keys}")) }
            }
            _ => Err("not a setting".into()),
        }
    }

    /// Push changed options into the running state.
    fn apply_config(&mut self) {
        self.bg = premultiply(self.cfg.default_bg);
        self.cache.set_cap(self.cfg.cache_mb << 20);
        let rc = self.cfg.effective_recolor();
        if rc != self.applied.recolor {
            self.applied.recolor = rc;
            self.invalidate(true);
        }
        if (self.cfg.page_gap - self.applied.gap).abs() > f32::EPSILON {
            self.applied.gap = self.cfg.page_gap;
            let cur = self.current_page();
            self.layout = Layout::new(self.layout.sizes().to_vec(), self.cfg.page_gap);
            self.goto_page(cur);
        }
        let font = (self.cfg.font.clone(), self.cfg.font_size * self.dpi);
        if font != self.applied.font {
            if font.0 != self.applied.font.0 || self.text.is_none() {
                match TextRenderer::load(&font.0, font.1) {
                    Ok(t) => self.text = Some(t),
                    Err(e) => self.err(e),
                }
            } else if let Some(t) = &mut self.text {
                t.set_px(font.1);
            }
            self.applied.font = font;
        }
        if self.cfg.auto_index != self.applied.auto_index {
            self.applied.auto_index = self.cfg.auto_index;
            self.refresh_index();
        }
        self.clamp_scroll();
        self.redraw();
    }

    // ---- analysis -----------------------------------------------------------------------

    /// Pick the real outline or the generated one according to `auto-index`.
    fn refresh_index(&mut self) {
        let auto = !self.auto_items.is_empty()
            && match self.cfg.auto_index {
                AutoIndex::Off => false,
                AutoIndex::Always => true,
                AutoIndex::Auto => self.outline_items.is_empty(),
            };
        let from_contents = self.analysis.as_ref().is_some_and(|a| a.index_source == cache::IndexSource::Contents);
        self.index_auto = auto.then_some(if from_contents { "contents" } else { "auto" });
        self.index = Index::new(if auto { self.auto_items.clone() } else { self.outline_items.clone() });
    }

    fn cancel_job(&mut self) {
        self.job = None; // Drop kills the process group
        self.job_status = None;
    }

    /// Use the cached analysis if valid, otherwise run the niced child. `force_ocr` / `recompute` skip the cache.
    fn start_analysis(&mut self, force_ocr: bool, mut recompute: bool) {
        self.cancel_job();
        let Ok(dir) = cache::dir_for(&self.path) else { return };
        if !force_ocr && !recompute {
            if let Some(a) = cache::load(&dir) {
                let need_previews = self.cfg.previews && !a.previews.done && self.cfg.auto_analyze;
                self.apply_analysis(a, &dir);
                if !need_previews {
                    return;
                }
                recompute = true; // cached before previews were wanted: one more pass
            }
        }
        if !self.cfg.auto_analyze && !force_ocr && !recompute {
            return;
        }
        let file = self.path.display().to_string();
        let cap = self.cfg.cache_max_mb.to_string();
        let mut args = vec!["--analyze", file.as_str(), "--cache-max-mb", cap.as_str()];
        if force_ocr {
            args.push("--ocr");
        } else if !self.cfg.auto_ocr {
            args.push("--no-ocr");
        }
        if recompute {
            args.push("--force");
        }
        if !self.cfg.previews {
            args.push("--no-previews");
        }
        self.spawn_job(&args, JobKind::Analyze);
    }

    fn spawn_job(&mut self, args: &[&str], kind: JobKind) {
        self.job_seq += 1;
        match Job::spawn(self.job_seq, args, self.worker_tx.clone(), self.proxy.clone()) {
            Ok(j) => {
                self.job = Some((j, kind));
                self.job_status = Some("starting analysis...".into());
            }
            Err(e) => self.err(format!("cannot start analysis: {e}")),
        }
    }

    fn apply_analysis(&mut self, a: Analysis, dir: &std::path::Path) {
        self.auto_items = a
            .index
            .iter()
            .map(|e| Item {
                title: e.title.clone(),
                depth: e.level as u32 - 1,
                page: Some(e.page),
                y: Some(e.y),
                uri: None,
                has_children: false,
            })
            .collect();
        // children flags from depth ordering
        for i in 0..self.auto_items.len().saturating_sub(1) {
            self.auto_items[i].has_children = self.auto_items[i + 1].depth > self.auto_items[i].depth;
        }
        let ocr = cache::ocr_path(dir);
        let src = (a.ocr == OcrState::Done && ocr.exists()).then_some(ocr);
        let _ = self.render.req_tx.send(Cmd::TextSource(src));
        if let OcrState::Failed(e) = &a.ocr {
            self.err(format!("OCR: {e}"));
        }
        self.pv = if self.cfg.previews && a.previews.done { Store::open(dir) } else { None };
        self.pv_refs.clear();
        self.popup = None;
        self.hover.cancel();
        self.analysis = Some(a);
        self.refresh_index();
    }

    fn on_worker(&mut self, id: u64, m: Msg) {
        if self.job.as_ref().map(|j| j.0.id) != Some(id) {
            return; // from a cancelled job
        }
        match m {
            Msg::Progress(d, t) => self.job_status = Some(format!("analysing {d}/{t}")),
            Msg::Status(s) => self.job_status = Some(s),
            Msg::Error(e) => self.err(e),
            Msg::Done => {
                let kind = self.job.as_ref().map(|j| j.1.clone());
                self.job_status = None;
                match kind {
                    Some(JobKind::WriteIndex(dst)) => self.msg(format!("Wrote {}", dst.display())),
                    _ => {
                        if let Ok(dir) = cache::dir_for(&self.path) {
                            if let Some(a) = cache::load(&dir) {
                                let ocr_done = a.ocr == OcrState::Done;
                                self.apply_analysis(a, &dir);
                                if ocr_done && self.message.is_none() {
                                    self.msg("OCR complete; search uses the OCR text".into());
                                }
                            }
                        }
                    }
                }
            }
            Msg::Exited => {
                if let Some(j) = self.job.as_mut() {
                    j.0.mark_exited();
                }
                self.job = None;
                self.job_status = None;
            }
        }
    }

    fn msg(&mut self, s: String) {
        self.message = Some((s, false));
    }

    fn err(&mut self, s: String) {
        self.message = Some((s, true));
    }

    // ---- view math ----------------------------------------------------------------------

    fn scale_key(&self) -> u32 {
        ((self.zoom * self.dpi * 1000.0).round() as u32).max(1)
    }

    fn scale(&self) -> f32 {
        self.scale_key() as f32 / 1000.0
    }

    fn bar_h(&self) -> i32 {
        match (&self.text, self.cfg.statusbar) {
            (Some(t), true) => statusbar::height(t),
            _ => 0,
        }
    }

    fn doc_h(&self) -> u32 {
        (self.vp.1 as i32 - self.bar_h()).max(1) as u32
    }

    fn vp_pt(&self) -> (f32, f32) {
        let s = self.scale();
        (self.vp.0 as f32 / s, self.doc_h() as f32 / s)
    }

    /// Left edge of the view in content coordinates (centered when content is narrower).
    fn x0(&self) -> f32 {
        let vw = self.vp_pt().0;
        let mw = self.layout.max_w;
        if mw <= vw { -(vw - mw) / 2.0 } else { self.sx.clamp(0.0, mw - vw) }
    }

    fn clamp_scroll(&mut self) {
        let (vw, vh) = self.vp_pt();
        self.sy = self.sy.clamp(0.0, (self.layout.total_h - vh).max(0.0));
        self.sx = self.sx.clamp(0.0, (self.layout.max_w - vw).max(0.0));
    }

    // ---- persistence --------------------------------------------------------------------

    fn sync_info(&self) {
        if let Ok(mut i) = self.info.lock() {
            i.filename = self.path.to_string_lossy().into_owned();
            i.pages = self.layout.len() as u32;
            i.page = self.current_page() as u32 + 1;
        }
    }

    /// (page, offset below the page top) of the top of the view.
    fn view_pos(&self) -> (u32, f32) {
        let p = self.layout.page_at(self.sy);
        (p as u32, self.sy - self.layout.top(p))
    }

    fn scroll_for(&self, page: u32, y: f32) -> f32 {
        self.layout.top((page as usize).min(self.layout.len() - 1)) + y
    }

    fn goto_saved(&mut self, page: u32, y: f32) {
        self.jumps.push(self.sy);
        self.sy = self.scroll_for(page, y);
        self.clamp_scroll();
    }

    /// Write the position and jumplist; called on close, focus loss, `open`, signals and exit.
    pub fn save_state(&self) {
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return };
        if self.opening.is_some() || self.window.is_none() {
            return;
        }
        let (page, y) = self.view_pos();
        db.save_pos(key, &self.path, &db::Pos { page, y, x: self.sx, zoom: self.zoom, recolor: Some(self.cfg.recolor.enabled) });
        let (pos, list) = self.jumps.saved();
        db.save_jumps(key, pos, list);
    }

    /// Restore the saved view of the current document. Returns whether a position was applied.
    fn restore_state(&mut self) -> bool {
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return false };
        if let Some((pos, list)) = db.load_jumps(key) {
            self.jumps = Jumplist::from_saved(pos, list);
        }
        if !self.cfg.restore_position {
            return false;
        }
        let Some(p) = db.load_pos(key, &self.path) else { return false };
        if let Some(r) = p.recolor {
            if r != self.cfg.recolor.enabled {
                self.cfg.recolor.enabled = r;
                self.apply_config();
            }
        }
        self.zoom = p.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.sx = p.x;
        self.sy = self.scroll_for(p.page, p.y);
        self.clamp_scroll();
        self.invalidate(false);
        true
    }

    fn refresh_key(&mut self) {
        self.key = cache::key(&self.path).ok();
        if let (Some(d), Some(k)) = (&self.db, &self.key) {
            d.note_open(k, &self.path);
        }
    }

    fn set_mark(&mut self, c: char) {
        let (Some(db), Some(key)) = (&self.db, self.key.clone()) else { return self.err("No state database".into()) };
        let (page, y) = self.view_pos();
        if c.is_ascii_uppercase() {
            db.set_gmark(c, &key, &self.path, page, y);
        } else {
            db.set_mark(&key, c, page, y);
        }
        self.msg(format!("Mark {c} set"));
    }

    fn jump_mark(&mut self, c: char, el: &ActiveEventLoop) {
        let Some(db) = &self.db else { return self.err("No state database".into()) };
        if c.is_ascii_uppercase() {
            match db.gmark(c) {
                Some((key, _, page, y)) if self.key.as_deref() == Some(&key) => self.goto_saved(page, y),
                Some((_, path, page, y)) => {
                    self.open_goto = Some((page, y));
                    self.exec(Command::Open(path.to_string_lossy().into_owned()), None, el);
                }
                None => self.err(format!("Mark {c} not set")),
            }
        } else {
            match self.key.as_ref().and_then(|k| db.mark(k, c)) {
                Some((page, y)) => self.goto_saved(page, y),
                None => self.err(format!("Mark {c} not set")),
            }
        }
    }

    fn bookmark_here(&mut self, name: String) {
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return self.err("No state database".into()) };
        let (page, y) = self.view_pos();
        db.set_bookmark(key, &name, page, y);
        self.msg(format!("Bookmark '{name}' set on page {}", page + 1));
    }

    fn open_blist(&mut self) {
        let items: Vec<PickItem> = match (&self.db, &self.key) {
            (Some(db), Some(key)) => db.bookmarks(key).into_iter().map(|(n, p, _)| PickItem { label: n.clone(), detail: String::new(), right: format!("p.{}", p + 1), arg: n }).collect(),
            _ => Vec::new(),
        };
        self.overlay = Overlay::Pick(Pick::new(PickKind::Bookmarks, "Bookmarks", items));
    }

    fn open_recent(&mut self) {
        let items: Vec<PickItem> = match &self.db {
            Some(db) => db.recent(60).into_iter().map(|(p, t)| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem { label: name, detail: dir, right: ago(t), arg: p.to_string_lossy().into_owned() }
            }).collect(),
            None => Vec::new(),
        };
        self.overlay = Overlay::Pick(Pick::new(PickKind::Recent, "Recent files", items));
    }

    // ---- synctex ------------------------------------------------------------------------

    fn synctex_view(&mut self, input: &str, line: u32, col: u32) {
        if !self.cfg.synctex {
            return self.err("synctex is off (set synctex true)".into());
        }
        match synctex::view(&self.path, input, line, col) {
            Ok(v) => {
                let p = (v.page as usize).min(self.layout.len() - 1);
                let (vw, vh) = self.vp_pt();
                self.jumps.push(self.sy);
                self.sy = self.layout.top(p) + v.rect[1] - vh / 3.0;
                self.sx = self.layout.left(p) + v.rect[0] - vw / 2.0;
                self.clamp_scroll();
                self.flash = Some((p as u32, v.rect));
                self.msg(format!("{input}:{line} -> page {}", p + 1));
            }
            Err(e) => self.err(e),
        }
        self.redraw();
    }

    /// Ctrl+click: source location of the clicked point, handed to `synctex-editor-command`.
    fn synctex_edit(&mut self) {
        if !self.cfg.synctex {
            return;
        }
        let s = self.scale();
        let (mx, my) = (self.cursor.0 as f32, self.cursor.1 as f32);
        if my >= self.doc_h() as f32 {
            return;
        }
        let (xp, yp) = (self.x0() + mx / s, self.sy + my / s);
        let p = self.layout.page_at(yp);
        let (lx, ly) = (xp - self.layout.left(p), yp - self.layout.top(p));
        match synctex::edit(&self.path, p as u32, lx, ly) {
            Err(e) => self.err(e),
            Ok(ed) => {
                if self.cfg.synctex_editor_command.trim().is_empty() {
                    self.msg(format!("{}:{} (set synctex-editor-command to open it in your editor)", ed.input, ed.line));
                } else {
                    match synctex::expand(&self.cfg.synctex_editor_command, &ed.input, ed.line, ed.column).and_then(|a| synctex::spawn_detached(&a)) {
                        Ok(()) => self.msg(format!("Editor: {}:{}", ed.input, ed.line)),
                        Err(e) => self.err(e),
                    }
                }
            }
        }
        self.redraw();
    }

    fn on_ext(&mut self, e: Ext, el: &ActiveEventLoop) {
        match e {
            Ext::Signal => el.exit(),
            Ext::Exec(line) => match keys::parse(&line) {
                Ok(c) => self.exec(c, None, el),
                Err(e) => self.err(e),
            },
            Ext::Goto(p) => {
                self.jumps.push(self.sy);
                self.goto_page(p.max(1) as usize - 1);
            }
            Ext::Open(p) => self.exec(Command::Open(p.to_string_lossy().into_owned()), None, el),
            Ext::Synctex { input, line, column } => self.synctex_view(&input, line, column),
        }
        self.redraw();
    }

    fn current_page(&self) -> usize {
        self.layout.page_at(self.sy + self.vp_pt().1 / 2.0)
    }

    /// Bump the generation so queued renders are dropped; optionally forget cached tiles.
    fn invalidate(&mut self, clear_cache: bool) {
        self.render.gen.fetch_add(1, Ordering::Relaxed);
        if clear_cache {
            self.cache.clear();
        }
        self.pending.clear();
    }

    fn set_zoom(&mut self, z: f32) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - self.zoom).abs() < 1e-6 {
            return;
        }
        let (vw, vh) = self.vp_pt();
        let (cx, cy) = (self.x0() + vw / 2.0, self.sy + vh / 2.0);
        self.zoom = z;
        let (vw, vh) = self.vp_pt();
        self.sx = cx - vw / 2.0;
        self.sy = cy - vh / 2.0;
        self.clamp_scroll();
        self.invalidate(false);
    }

    fn zoom_factor(&self, n: i32) -> f32 {
        (1.0 + self.cfg.zoom_step / 100.0).powi(n)
    }

    fn goto_arg(&mut self, g: crate::pages::GotoArg) {
        let map = self.analysis.as_ref().map(|a| &a.page_map);
        match crate::pages::resolve_goto(map, g, self.cfg.goto_printed, self.layout.len() as u32) {
            Ok((p, note)) => {
                self.jumps.push(self.sy);
                self.goto_page(p as usize);
                if let Some(n) = note {
                    self.err(n);
                }
            }
            Err(e) => self.err(e),
        }
    }

    fn goto_page(&mut self, p: usize) {
        self.sy = self.layout.top(p.min(self.layout.len() - 1));
        self.clamp_scroll();
    }

    /// Jump to `y` points below the page top (a little context is kept above).
    fn goto_page_y(&mut self, p: usize, y: Option<f32>) {
        let p = p.min(self.layout.len() - 1);
        self.sy = match y {
            Some(y) => (self.layout.top(p) + y - 24.0).max(self.layout.top(p)),
            None => self.layout.top(p),
        };
        self.clamp_scroll();
    }

    fn scroll_by(&mut self, dx_px: f32, dy_px: f32) {
        let s = self.scale();
        self.sx += dx_px / s;
        self.sy += dy_px / s;
        self.clamp_scroll();
    }

    fn jump_to_hit(&mut self, h: Hit) {
        let p = h.page as usize;
        let (vw, vh) = self.vp_pt();
        self.sy = self.layout.top(p) + h.rect[1] - vh / 3.0;
        self.sx = self.layout.left(p) + h.rect[0] - vw / 2.0;
        self.clamp_scroll();
        self.highlight = true;
    }

    // ---- commands -----------------------------------------------------------------------

    fn exec(&mut self, cmd: Command, count: Option<u32>, el: &ActiveEventLoop) {
        let n = count.unwrap_or(1).max(1);
        let step = self.cfg.scroll_step * self.dpi * n as f32;
        match cmd {
            Command::ScrollDown => self.scroll_by(0.0, step),
            Command::ScrollUp => self.scroll_by(0.0, -step),
            Command::ScrollLeft => self.scroll_by(-step, 0.0),
            Command::ScrollRight => self.scroll_by(step, 0.0),
            Command::HalfPageDown => self.scroll_by(0.0, self.doc_h() as f32 / 2.0 * n as f32),
            Command::HalfPageUp => self.scroll_by(0.0, -(self.doc_h() as f32) / 2.0 * n as f32),
            Command::NextPage => self.goto_page((self.current_page() + n as usize).min(self.layout.len() - 1)),
            Command::PrevPage => self.goto_page(self.current_page().saturating_sub(n as usize)),
            Command::FirstPage | Command::LastPage if count.is_some() => self.goto_arg(crate::pages::GotoArg::Number(n)),
            Command::FirstPage => self.goto_page(0),
            Command::LastPage => self.goto_page(self.layout.len() - 1),
            Command::Goto(a) => match crate::pages::parse_goto(&a) {
                Ok(g) => self.goto_arg(g),
                Err(e) => self.err(e),
            },
            Command::ZoomIn => self.set_zoom(self.zoom * self.zoom_factor(n as i32)),
            Command::ZoomOut => self.set_zoom(self.zoom / self.zoom_factor(n as i32)),
            Command::ZoomReset => self.set_zoom(1.0),
            Command::Zoom(pct) => self.set_zoom(pct / 100.0),
            Command::FitWidth => {
                let p = self.current_page();
                self.set_zoom(self.vp.0 as f32 / self.layout.size(p).0 / self.dpi);
                self.goto_page(p);
            }
            Command::FitPage => {
                let p = self.current_page();
                let (w, h) = self.layout.size(p);
                self.set_zoom((self.vp.0 as f32 / w).min(self.doc_h() as f32 / h) / self.dpi);
                self.goto_page(p);
            }
            Command::ToggleRecolor => {
                self.cfg.recolor.enabled = !self.cfg.recolor.enabled;
                self.apply_config();
            }
            Command::ToggleStatusbar => {
                self.cfg.statusbar = !self.cfg.statusbar;
                self.clamp_scroll();
            }
            Command::Quit => el.exit(),
            Command::SetMarkPrompt | Command::JumpMarkPrompt => {
                self.mark_prompt = Some(cmd == Command::SetMarkPrompt);
                self.msg(if cmd == Command::SetMarkPrompt { "Set mark: a-z, A-Z global".into() } else { "Jump to mark".into() });
            }
            Command::Mark(set, c) => {
                if set { self.set_mark(c) } else { self.jump_mark(c, el) }
            }
            Command::Bmark(n) => self.bookmark_here(n),
            Command::Blist => self.open_blist(),
            Command::Recent => self.open_recent(),
            Command::Bdelete(n) => {
                let ok = matches!((&self.db, &self.key), (Some(d), Some(k)) if d.delete_bookmark(k, &n));
                if ok { self.msg(format!("Deleted bookmark '{n}'")) } else { self.err(format!("No bookmark '{n}'")) }
            }
            Command::Bjump(n) => match self.db.as_ref().zip(self.key.as_ref()).and_then(|(d, k)| d.bookmark(k, &n)) {
                Some((page, y)) => self.goto_saved(page, y),
                None => self.err(format!("No bookmark '{n}'")),
            },
            Command::SynctexView(f, l, c) => self.synctex_view(&f, l, c),
            Command::CommandMode => self.overlay = Overlay::Input(InputBar::new(':')),
            Command::SearchForward => self.open_search_bar('/'),
            Command::SearchBackward => self.open_search_bar('?'),
            Command::Search(q) => self.start_search(q, true),
            Command::BSearch(q) => self.start_search(q, false),
            Command::SearchNext | Command::SearchPrev => {
                let fwd = cmd == Command::SearchNext;
                let hit = self.search.as_mut().and_then(|s| {
                    let d = if fwd { s.forward } else { !s.forward };
                    s.step(d)
                });
                match hit {
                    Some(h) => self.jump_to_hit(h),
                    None => self.err("No search results".into()),
                }
            }
            Command::ClearHighlight => self.highlight = false,
            Command::Follow => self.start_hints(),
            Command::JumpBack => {
                if let Some(y) = self.jumps.back(self.sy) {
                    self.sy = y;
                    self.clamp_scroll();
                }
            }
            Command::JumpForward => {
                if let Some(y) = self.jumps.forward() {
                    self.sy = y;
                    self.clamp_scroll();
                }
            }
            Command::Help => self.overlay = Overlay::Help(Help::new(&self.keymap)),
            Command::Previews => match &self.pv {
                Some(pv) => self.overlay = Overlay::List(PList::new(&pv.targets)),
                None => self.err("No previews (analysis pending, or `previews` is off)".into()),
            },
            Command::PreviewHints => self.start_preview_hints(),
            Command::PopupClose => self.close_popup(),
            Command::PopupJump => self.popup_jump(),
            Command::PopupDown | Command::PopupUp => {
                let d = (self.cfg.scroll_step * self.dpi * n as f32) as i32;
                if let Some(p) = self.popup.as_mut() {
                    p.scroll_by(if cmd == Command::PopupDown { d } else { -d });
                }
            }
            Command::PopupZoomIn | Command::PopupZoomOut => {
                if let Some(p) = self.popup.as_mut() {
                    p.zoom = (if cmd == Command::PopupZoomIn { p.zoom * 1.25 } else { p.zoom / 1.25 }).clamp(0.4, 4.0);
                    self.request_popup();
                }
            }
            Command::PopupYank => {
                if let (Some(p), Some(pv)) = (&self.popup, &self.pv) {
                    let t = &pv.targets[p.target as usize];
                    self.yank_pending = true;
                    let _ = self.render.req_tx.send(Cmd::Text { page: t.page, rect: t.rect });
                }
            }
            Command::Index => self.mode = if self.mode == Mode::Index { Mode::Normal } else { Mode::Index },
            Command::IndexDown => self.index.down(n as usize),
            Command::IndexUp => self.index.up(n as usize),
            Command::IndexExpand => self.index.expand(),
            Command::IndexCollapse => self.index.collapse(),
            Command::IndexFirst => self.index.first(),
            Command::IndexLast => self.index.last(),
            Command::IndexClose => self.mode = Mode::Normal,
            Command::IndexSelect => {
                if let Some(it) = self.index.current().cloned() {
                    match (it.page, it.uri) {
                        (Some(p), _) => {
                            self.jumps.push(self.sy);
                            self.goto_page_y(p as usize, it.y);
                            self.mode = Mode::Normal;
                        }
                        (None, Some(u)) => {
                            if let Err(e) = open_external(&u) {
                                self.err(e);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Command::Set(..) | Command::Map(..) | Command::Unmap(..) => {
                if let Err(e) = self.apply_setting(cmd) {
                    self.err(e);
                }
                self.apply_config();
            }
            Command::Open(p) => {
                let mut path = PathBuf::from(&p);
                if path.is_relative() {
                    path = std::env::current_dir().unwrap_or_default().join(path);
                }
                self.save_state();
                self.msg(format!("Opening {p}..."));
                self.opening = Some((path.clone(), None));
                let _ = self.render.req_tx.send(Cmd::Open(path));
            }
            Command::Reload => {
                self.opening = Some((self.path.clone(), Some(self.sy)));
                let _ = self.render.req_tx.send(Cmd::Open(self.path.clone()));
            }
            Command::Ocr => {
                self.start_analysis(true, false);
                if self.job.is_some() {
                    self.msg("OCR started (see statusbar)".into());
                }
            }
            Command::OcrCancel => {
                if self.job.is_some() {
                    self.cancel_job();
                    self.msg("Analysis cancelled".into());
                } else {
                    self.err("No job running".into());
                }
            }
            Command::Reindex => self.start_analysis(false, true),
            Command::WriteIndex(path, force) => self.write_index(path, force),
            Command::ReloadConfig => {
                self.load_config();
                if self.message.is_none() {
                    self.msg("Config reloaded".into());
                }
            }
        }
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn write_index(&mut self, path: Option<String>, force: bool) {
        if self.auto_items.is_empty() {
            return self.err("No generated index (wait for analysis or :reindex)".into());
        }
        if self.job.is_some() {
            return self.err("A background job is running; try again when it finishes".into());
        }
        let dst = match path {
            Some(p) => PathBuf::from(p),
            None => {
                let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.path.with_file_name(format!("{stem}.indexed.pdf"))
            }
        };
        if dst == self.path {
            return self.err("Refusing to overwrite the original".into());
        }
        if dst.exists() && !force {
            return self.err(format!("{} exists; use :writeindex! to overwrite", dst.display()));
        }
        let (src, d) = (self.path.display().to_string(), dst.display().to_string());
        self.spawn_job(&["--write-index", &src, &d], JobKind::WriteIndex(dst));
        self.job_status = Some("writing index...".into());
    }

    // ---- search -------------------------------------------------------------------------

    fn open_search_bar(&mut self, prefix: char) {
        self.pre_search_sy = self.sy;
        self.overlay = Overlay::Input(InputBar::new(prefix));
    }

    fn start_search(&mut self, q: String, forward: bool) {
        if q.is_empty() {
            self.search = None;
            self.highlight = false;
            let _ = self.render.req_tx.send(Cmd::CancelSearch);
            return;
        }
        self.search_id += 1;
        self.highlight = true;
        self.search = Some(Search::new(self.search_id, q.clone(), forward));
        let _ = self.render.req_tx.send(Cmd::Search(SearchReq {
            id: self.search_id,
            case_sensitive: case_sensitive(&q),
            query: q,
            forward,
            start_page: self.current_page() as u32,
        }));
    }

    // ---- links --------------------------------------------------------------------------

    fn follow(&mut self, t: &Target) {
        match t {
            Target::Page(p, y) => {
                self.jumps.push(self.sy);
                self.goto_page_y(*p as usize, *y);
            }
            Target::Uri(u) => match open_external(u) {
                Ok(()) => self.msg(format!("Opened {u}")),
                Err(e) => self.err(e),
            },
        }
    }

    /// Screen position (px) of the page's top-left corner.
    fn page_origin(&self, p: usize) -> (i64, i64) {
        let s = self.scale();
        (
            ((self.layout.left(p) - self.x0()) * s).round() as i64,
            ((self.layout.top(p) - self.sy) * s).round() as i64,
        )
    }

    fn visible_links(&self) -> Vec<(u32, [f32; 4], Target)> {
        let (_, vh) = self.vp_pt();
        let mut out = Vec::new();
        for p in self.layout.visible(self.sy, self.sy + vh) {
            for l in self.links.get(&(p as u32)).map(|v| v.as_slice()).unwrap_or(&[]) {
                let (_, py) = self.page_origin(p);
                let s = self.scale();
                let (y0, y1) = (py as f32 + l.rect[1] * s, py as f32 + l.rect[3] * s);
                if y1 > 0.0 && y0 < self.doc_h() as f32 {
                    out.push((p as u32, l.rect, l.target.clone()));
                }
            }
        }
        out
    }

    fn start_hints(&mut self) {
        let links = self.visible_links();
        if links.is_empty() {
            self.err("No links on screen".into());
            return;
        }
        let labels = hint_labels(links.len(), HINT_ALPHABET);
        let items = links
            .into_iter()
            .zip(labels)
            .map(|((page, rect, target), label)| HintItem { page, rect, label, target, pv: None })
            .collect();
        self.overlay = Overlay::Hints(Hints { typed: String::new(), items });
    }

    fn click(&mut self) {
        let s = self.scale();
        let (mx, my) = (self.cursor.0 as f32, self.cursor.1 as f32);
        if my >= self.doc_h() as f32 {
            return;
        }
        let (xp, yp) = (self.x0() + mx / s, self.sy + my / s);
        let p = self.layout.page_at(yp);
        let (lx, ly) = (xp - self.layout.left(p), yp - self.layout.top(p));
        let hit = self.links.get(&(p as u32)).and_then(|ls| {
            ls.iter().find(|l| lx >= l.rect[0] && lx <= l.rect[2] && ly >= l.rect[1] && ly <= l.rect[3]).map(|l| l.target.clone())
        });
        if let Some(t) = hit {
            self.follow(&t);
            self.redraw();
        }
    }

    // ---- previews -----------------------------------------------------------------------

    fn refs_for(&mut self, page: u32) -> &Vec<Ref> {
        if self.pv_refs.len() > 40 {
            self.pv_refs.clear();
        }
        let pv = &self.pv;
        self.pv_refs.entry(page).or_insert_with(|| pv.as_ref().map(|s| s.refs_on(page)).unwrap_or_default())
    }

    /// The reference under the mouse: (page, index on that page).
    fn ref_at(&mut self) -> Option<(u32, usize, Ref)> {
        self.pv.as_ref()?;
        let s = self.scale();
        let (mx, my) = (self.cursor.0 as f32, self.cursor.1 as f32);
        if my >= self.doc_h() as f32 || mx < 0.0 || my < 0.0 {
            return None;
        }
        let (xp, yp) = (self.x0() + mx / s, self.sy + my / s);
        let p = self.layout.page_at(yp);
        let (lx, ly) = (xp - self.layout.left(p), yp - self.layout.top(p));
        let page = p as u32;
        self.refs_for(page).iter().enumerate().find(|(_, r)| lx >= r.rect[0] - 1.0 && lx <= r.rect[2] + 1.0 && ly >= r.rect[1] - 1.0 && ly <= r.rect[3] + 1.0).map(|(i, r)| (page, i, *r))
    }

    /// Mouse moved: arm or cancel the hover timer, close a hover popup the mouse left.
    fn update_hover(&mut self) {
        let active = self.cfg.previews && self.pv.is_some() && matches!(self.overlay, Overlay::None) && self.mode == Mode::Normal;
        if !active {
            return;
        }
        if self.popup.as_ref().is_some_and(|p| p.hover && p.contains(self.cursor.0, self.cursor.1)) {
            self.hover.disarm();
            return; // moved onto the popup itself
        }
        let key = self.ref_at().map(|(p, i, _)| (p, i));
        if self.hover.moved(key, Instant::now(), Duration::from_millis(self.cfg.preview_hover_ms)) && self.popup.as_ref().is_some_and(|p| p.hover) {
            self.popup = None;
            self.redraw();
        }
    }

    /// Open the popup for the reference the mouse rests on.
    fn hover_fire(&mut self) {
        let Some((page, i)) = self.hover.key else { return };
        let Some(target) = self.refs_for(page).get(i).map(|r| r.target) else { return };
        self.open_popup(target, (self.cursor.0 as i32, self.cursor.1 as i32), true);
        self.redraw();
    }

    fn open_popup(&mut self, target: u32, anchor: (i32, i32), hover: bool) {
        let Some(pv) = &self.pv else { return };
        let Some(t) = pv.targets.get(target as usize) else { return };
        let title = if t.kind == Kind::Link { format!("Destination — p.{}", t.page + 1) } else { format!("{} — p.{}", t.title(), t.page + 1) };
        if debug() {
            eprintln!("open popup target {target} hover={hover}");
        }
        self.popup = Some(Popup { target, id: 0, tile: None, zoom: 1.0, scroll: 0, hover, anchor, title, rect: (0, 0, 0, 0) });
        if !hover {
            self.mode = Mode::Popup;
        }
        self.request_popup();
    }

    fn request_popup(&mut self) {
        let (Some(p), Some(pv)) = (self.popup.as_mut(), self.pv.as_ref()) else { return };
        let t = &pv.targets[p.target as usize];
        let (rw, rh) = ((t.rect[2] - t.rect[0]).max(4.0), (t.rect[3] - t.rect[1]).max(4.0));
        let base = (0.6 * self.vp.0 as f32 / rw).min(self.cfg.preview_max_scale * self.dpi);
        // crisp, but never more than ~6 Mpx however large the region
        let s = (base * p.zoom).min((6.0e6 / (rw * rh)).sqrt()).max(0.2);
        self.popup_seq += 1;
        p.id = self.popup_seq;
        let recolor = self.applied.recolor.with_enabled(self.cfg.recolor.enabled);
        let _ = self.render.req_tx.send(Cmd::Preview(PreviewReq { id: p.id, page: t.page, rect: t.rect, scale: s, recolor }));
    }

    fn close_popup(&mut self) {
        self.popup = None;
        if self.mode == Mode::Popup {
            self.mode = Mode::Normal;
        }
    }

    fn popup_jump(&mut self) {
        let Some(t) = self.popup.as_ref().and_then(|p| self.pv.as_ref().map(|pv| pv.targets[p.target as usize].clone())) else { return };
        self.close_popup();
        if matches!(self.overlay, Overlay::List(_)) {
            self.overlay = Overlay::None;
        }
        self.jumps.push(self.sy);
        self.goto_page_y(t.page as usize, Some(t.rect[1]));
    }

    fn start_preview_hints(&mut self) {
        if !self.cfg.previews || self.pv.is_none() {
            self.err("No previews (analysis pending, or `previews` is off)".into());
            return;
        }
        let (_, vh) = self.vp_pt();
        let s = self.scale();
        let doc_h = self.doc_h() as f32;
        let mut found: Vec<(u32, [f32; 4], u32)> = Vec::new();
        for p in self.layout.visible(self.sy, self.sy + vh) {
            let (_, py) = self.page_origin(p);
            for r in self.refs_for(p as u32).clone() {
                let (y0, y1) = (py as f32 + r.rect[1] * s, py as f32 + r.rect[3] * s);
                if y1 > 0.0 && y0 < doc_h {
                    found.push((p as u32, r.rect, r.target));
                }
            }
        }
        if found.is_empty() {
            self.err("No previewable references on screen".into());
            return;
        }
        let labels = hint_labels(found.len(), HINT_ALPHABET);
        let items = found
            .into_iter()
            .zip(labels)
            .map(|((page, rect, t), label)| HintItem { page, rect, label, target: Target::Page(0, None), pv: Some(t) })
            .collect();
        self.overlay = Overlay::Hints(Hints { typed: String::new(), items });
    }

    /// Hand text to the clipboard through `wl-copy`; the helper is reaped on its own thread.
    fn to_clipboard(&mut self, text: String) {
        use std::io::Write;
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        match std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
            Ok(mut child) => {
                if let Some(mut si) = child.stdin.take() {
                    let _ = si.write_all(text.as_bytes());
                }
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                self.msg("Copied to clipboard".into());
            }
            Err(e) => self.err(format!("wl-copy: {e}")),
        }
    }

    // ---- keys ---------------------------------------------------------------------------

    fn on_key(&mut self, token: String, ch: Option<char>, el: &ActiveEventLoop) {
        self.message = None;
        self.flash = None;
        if let (Some(set), Overlay::None) = (self.mark_prompt.take(), &self.overlay) {
            match ch.filter(|c| c.is_ascii_alphabetic()) {
                Some(c) => self.exec(Command::Mark(set, c), None, el),
                None => self.err("Not a mark name".into()),
            }
            self.redraw();
            return;
        }
        // a popup opened from the list takes the keys first
        if self.popup.as_ref().is_some_and(|p| !p.hover) && matches!(self.overlay, Overlay::List(_)) {
            if let Some((cmd, count)) = self.keys.feed(&self.keymap, Mode::Popup, &token) {
                self.exec(cmd, count, el);
            }
            self.redraw();
            return;
        }
        match &mut self.overlay {
            Overlay::List(l) => {
                let Some(pv) = &self.pv else {
                    self.overlay = Overlay::None;
                    return;
                };
                let tg = &pv.targets;
                match token.as_str() {
                    "<Esc>" => {
                        if l.query.is_empty() {
                            self.overlay = Overlay::None;
                        } else {
                            l.query.clear();
                            l.sel = 0;
                        }
                    }
                    "<BS>" => l.backspace(),
                    "<Down>" | "<C-n>" => l.move_sel(1, tg),
                    "<Up>" | "<C-p>" => l.move_sel(-1, tg),
                    "<PgDn>" | "<C-d>" => l.move_sel(10, tg),
                    "<PgUp>" | "<C-u>" => l.move_sel(-10, tg),
                    "<CR>" => {
                        if let Some(id) = l.current(tg) {
                            let t = tg[id as usize].clone();
                            self.overlay = Overlay::None;
                            self.jumps.push(self.sy);
                            self.goto_page_y(t.page as usize, Some(t.rect[1]));
                        }
                    }
                    "<Tab>" => {
                        if let Some(id) = l.current(tg) {
                            self.open_popup(id, (0, 0), false);
                        }
                    }
                    "<Space>" if l.query.is_empty() => {
                        if let Some(id) = l.current(tg) {
                            self.open_popup(id, (0, 0), false);
                        }
                    }
                    _ => {
                        if let Some(c) = ch.filter(|c| !c.is_control()) {
                            l.type_char(c);
                        }
                    }
                }
            }
            Overlay::Pick(pk) => match token.as_str() {
                "<Esc>" => {
                    if pk.query.is_empty() {
                        self.overlay = Overlay::None;
                    } else {
                        pk.query.clear();
                        pk.sel = 0;
                    }
                }
                "<BS>" => pk.backspace(),
                "<Down>" | "<C-n>" => pk.move_sel(1),
                "<Up>" | "<C-p>" => pk.move_sel(-1),
                "<PgDn>" | "<C-d>" => pk.move_sel(10),
                "<PgUp>" | "<C-u>" => pk.move_sel(-10),
                "<CR>" => {
                    let act = pk.current().map(|i| (pk.kind, i.arg.clone()));
                    self.overlay = Overlay::None;
                    match act {
                        Some((PickKind::Bookmarks, n)) => self.exec(Command::Bjump(n), None, el),
                        Some((PickKind::Recent, path)) => self.exec(Command::Open(path), None, el),
                        None => {}
                    }
                }
                _ => {
                    if let Some(c) = ch.filter(|c| !c.is_control()) {
                        pk.type_char(c);
                    }
                }
            },
            Overlay::None => {
                if let Some((cmd, count)) = self.keys.feed(&self.keymap, self.mode, &token) {
                    self.exec(cmd, count, el);
                }
            }
            Overlay::Input(_) => self.input_key(&token, ch, el),
            Overlay::Help(h) => {
                let rows = self.help_rows as i32;
                match token.as_str() {
                    "<Esc>" | "<F1>" => {
                        if h.query.is_empty() || token == "<F1>" {
                            self.overlay = Overlay::None;
                        } else {
                            h.query.clear();
                        }
                    }
                    "<BS>" => h.backspace(),
                    "<Down>" => h.scroll_by(1, self.help_rows),
                    "<Up>" => h.scroll_by(-1, self.help_rows),
                    "<PgDn>" | "<C-d>" => h.scroll_by(rows / 2, self.help_rows),
                    "<PgUp>" | "<C-u>" => h.scroll_by(-rows / 2, self.help_rows),
                    "<Home>" => h.scroll = 0,
                    _ => match ch {
                        Some('j') if h.query.is_empty() => h.scroll_by(1, self.help_rows),
                        Some('k') if h.query.is_empty() => h.scroll_by(-1, self.help_rows),
                        Some(c) => h.type_char(c),
                        None => {}
                    },
                }
            }
            Overlay::Hints(h) => {
                if token == "<Esc>" {
                    self.overlay = Overlay::None;
                } else if let Some(c) = ch {
                    h.typed.push(c);
                    let typed = h.typed.clone();
                    if let Some(i) = h.items.iter().position(|i| i.label == typed) {
                        let t = h.items[i].target.clone();
                        let pv = h.items[i].pv;
                        self.overlay = Overlay::None;
                        match pv {
                            Some(id) => self.open_popup(id, (0, 0), false),
                            None => self.follow(&t),
                        }
                    } else if !h.items.iter().any(|i| i.label.starts_with(&typed)) {
                        self.overlay = Overlay::None;
                    }
                }
            }
        }
        self.redraw();
    }

    fn input_key(&mut self, token: &str, ch: Option<char>, el: &ActiveEventLoop) {
        let Overlay::Input(bar) = &mut self.overlay else { return };
        let prefix = bar.prefix;
        let mut changed = false;
        match token {
            "<Esc>" => {
                self.overlay = Overlay::None;
                if prefix != ':' {
                    self.sy = self.pre_search_sy;
                    self.start_search(String::new(), true);
                }
                return;
            }
            "<CR>" => {
                let text = bar.string();
                self.overlay = Overlay::None;
                self.hist.push(prefix, &text);
                if prefix == ':' {
                    match keys::parse(&text) {
                        Ok(c) => self.exec(c, None, el),
                        Err(e) => self.err(e),
                    }
                } else {
                    let fwd = prefix == '/';
                    let same = self.search.as_ref().is_some_and(|s| s.query == text && s.forward == fwd);
                    if !(self.cfg.incremental_search && same) {
                        self.start_search(text, fwd);
                    }
                }
                return;
            }
            "<BS>" => {
                if bar.text.is_empty() {
                    self.overlay = Overlay::None;
                    return;
                }
                bar.backspace();
                changed = true;
            }
            "<Del>" => {
                bar.delete();
                changed = true;
            }
            "<Left>" => bar.left(),
            "<Right>" => bar.right(),
            "<Home>" | "<C-a>" => bar.home(),
            "<End>" | "<C-e>" => bar.end(),
            "<C-w>" => {
                bar.ctrl_w();
                changed = true;
            }
            "<C-u>" => {
                bar.ctrl_u();
                changed = true;
            }
            "<Up>" => {
                bar.history(&mut self.hist, true);
                changed = true;
            }
            "<Down>" => {
                bar.history(&mut self.hist, false);
                changed = true;
            }
            "<Tab>" => bar.tab(),
            _ => {
                if let Some(c) = ch.filter(|c| !c.is_control()) {
                    bar.insert(c);
                    changed = true;
                }
            }
        }
        if changed && prefix != ':' && self.cfg.incremental_search {
            let q = bar.string();
            self.start_search(q, prefix == '/');
        }
    }

    // ---- responses ----------------------------------------------------------------------

    fn on_opened(&mut self, r: Result<Vec<(f32, f32)>, String>) {
        let pending = self.opening.take();
        match r {
            Err(e) => self.err(e),
            Ok(sizes) => {
                let keep = pending.as_ref().and_then(|p| p.1);
                if let Some((path, _)) = pending {
                    self.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.path = path;
                }
                if keep.is_none() {
                    self.refresh_key();
                }
                self.layout = Layout::new(sizes, self.cfg.page_gap);
                self.links.clear();
                self.links_pending.clear();
                self.search = None;
                self.highlight = false;
                self.mode = Mode::Normal;
                self.cancel_job();
                self.analysis = None;
                self.auto_items.clear();
                self.outline_items.clear();
                self.index = Index::default();
                self.invalidate(true);
                self.sx = 0.0;
                self.sy = keep.unwrap_or(0.0);
                self.title_page = usize::MAX;
                self.clamp_scroll();
                self.start_analysis(false, false);
                if keep.is_none() {
                    self.jumps = Jumplist::default();
                    self.restore_state();
                    if let Some((page, y)) = self.open_goto.take() {
                        self.sy = self.scroll_for(page, y);
                        self.clamp_scroll();
                    }
                    self.msg(format!("Opened {}", self.name));
                } else {
                    self.msg("Reloaded".into());
                }
                self.sync_info();
            }
        }
    }

    // ---- painting -----------------------------------------------------------------------

    /// Tiles intersecting the view, with their screen positions.
    fn visible_tiles(&self) -> Vec<Placed> {
        let s = self.scale();
        let (_, vh_pt) = self.vp_pt();
        let vh = self.doc_h() as i64;
        let mut out = Vec::new();
        for p in self.layout.visible(self.sy, self.sy + vh_pt) {
            let (w, h) = page_px(self.layout.size(p), s);
            let (strip, n) = tile_grid(w, h);
            let (px, py) = self.page_origin(p);
            for t in 0..n {
                let ty = py + (t * strip) as i64;
                let th = (strip.min(h - t * strip)) as i64;
                if ty < vh && ty + th > 0 {
                    out.push(Placed { key: TileKey { page: p as u32, scale: self.scale_key(), tile: t }, x: px, y: ty });
                }
            }
        }
        out
    }

    fn tile_count(&self, page: usize) -> u32 {
        let (w, h) = page_px(self.layout.size(page), self.scale());
        tile_grid(w, h).1
    }

    /// One tile before the first and after the last visible tile.
    fn neighbours(&self, vis: &[Placed]) -> Vec<TileKey> {
        let (Some(first), Some(last)) = (vis.first(), vis.last()) else { return vec![] };
        let mut out = Vec::new();
        let (p, t) = (first.key.page, first.key.tile);
        if t > 0 {
            out.push(TileKey { tile: t - 1, ..first.key });
        } else if p > 0 {
            let pp = p - 1;
            out.push(TileKey { page: pp, tile: self.tile_count(pp as usize) - 1, ..first.key });
        }
        let (p, t) = (last.key.page, last.key.tile);
        if t + 1 < self.tile_count(p as usize) {
            out.push(TileKey { tile: t + 1, ..last.key });
        } else if (p as usize) + 1 < self.layout.len() {
            out.push(TileKey { page: p + 1, tile: 0, ..last.key });
        }
        out
    }

    fn paint(&mut self) {
        let Some(window) = self.window.clone() else { return };
        let size = window.inner_size();
        self.vp = (size.width, size.height);
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        self.clamp_scroll();

        let vis = self.visible_tiles();
        let mut wanted: Vec<TileKey> = vis.iter().map(|p| p.key).collect();
        wanted.extend(self.neighbours(&vis));
        *self.render.wanted.lock().unwrap() = wanted.clone();
        let gen = self.render.gen.load(Ordering::Relaxed);
        let recolor = self.applied.recolor.with_enabled(self.cfg.recolor.enabled);
        for k in &wanted {
            if !self.cache.contains(k) && self.pending.insert(*k) {
                let _ = self.render.req_tx.send(Cmd::Tile(Request { key: *k, gen, recolor }));
            }
        }
        for k in &wanted {
            self.cache.get(k); // keep neighbours fresh in the LRU
        }
        self.request_links();

        // Everything that needs &self before the surface is borrowed.
        let s = self.scale();
        let doc_h = self.doc_h() as i64;
        let mut hl: Vec<(i32, i32, i32, i32, bool)> = Vec::new();
        if let (true, Some(sr)) = (self.highlight, &self.search) {
            for p in self.layout.visible(self.sy, self.sy + self.vp_pt().1) {
                let (ox, oy) = self.page_origin(p);
                for hit in sr.on_page(p as u32) {
                    let r = hit.rect;
                    hl.push((
                        (ox as f32 + r[0] * s) as i32,
                        (oy as f32 + r[1] * s) as i32,
                        ((r[2] - r[0]) * s).ceil() as i32,
                        ((r[3] - r[1]) * s).ceil() as i32,
                        sr.cur == Some(*hit),
                    ));
                }
            }
        }
        if let Some((fp, r)) = self.flash {
            let (ox, oy) = self.page_origin(fp as usize);
            hl.push(((ox as f32 + r[0] * s) as i32, (oy as f32 + r[1] * s) as i32, ((r[2] - r[0]) * s).ceil() as i32, ((r[3] - r[1]) * s).ceil() as i32, true));
        }
        let mut hint_draw: Vec<(i32, i32, String, usize)> = Vec::new();
        if let Overlay::Hints(hs) = &self.overlay {
            for it in &hs.items {
                if !it.label.starts_with(&hs.typed) {
                    continue;
                }
                let (ox, oy) = self.page_origin(it.page as usize);
                hint_draw.push(((ox as f32 + it.rect[0] * s) as i32, (oy as f32 + it.rect[1] * s) as i32, it.label.clone(), hs.typed.len()));
            }
        }
        let name = self.name.clone();
        let msg = self.message.clone();
        let pending_keys = self.keys.display();
        let page = self.current_page();
        let printed = self.analysis.as_ref().and_then(|a| a.page_map.pdf_to_printed(page as u32));
        let total = self.layout.len();
        let zoom_pct = (self.zoom * 100.0).round() as u32;
        let mode_name = match (&self.overlay, self.mode) {
            (Overlay::Help(_), _) => "HELP",
            (Overlay::Hints(_), _) => "HINTS",
            (Overlay::List(_), _) => "PREVIEWS",
            (Overlay::Pick(p), _) => if p.kind == PickKind::Bookmarks { "BOOKMARKS" } else { "RECENT" },
            (_, Mode::Popup) => "PREVIEW",
            (_, Mode::Index) => "INDEX",
            _ => "NORMAL",
        };
        let job_text = self.job_status.clone();
        let index_auto = self.index_auto;
        let search_pos = self.search.as_ref().map(|s| (s.index().map_or(0, |i| i + 1), s.hits.len(), s.done));
        let bar_visible = self.cfg.statusbar;
        let under = !self.cfg.recolor.enabled;
        let bg = self.bg;

        let Some(surface) = self.surface.as_mut() else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        buf.fill(bg);
        let mut cv = Canvas { buf: &mut buf, w: size.width as i32, h: size.height as i32 };
        for p in &vis {
            match self.cache.get(&p.key) {
                Some(t) => cv.blit(t.w as i64, t.h as i64, &t.data, p.x, p.y, doc_h, under),
                None => fallback(&mut cv, &self.cache, &self.layout, p, doc_h, under),
            }
        }
        for &(x, y, ww, hh, cur) in &hl {
            let c = if cur { pm(CUR_HIT_COLOR, 0x90) } else { pm(HIT_COLOR, 0x55) };
            cv.fill_rect(x, y, ww, hh, c);
        }

        let full_w = cv.w;
        let mut help_rows = None;
        let mut index_rows = None;
        if let Some(t) = self.text.as_mut() {
            let bar = if bar_visible { statusbar::height(t) } else { 0 };
            for (x, y, label, typed) in &hint_draw {
                let lw = t.width(label) + 6;
                cv.fill_rect(*x, *y, lw, t.line_h, pm([0xf9, 0xe2, 0xaf], 0xf0));
                t.draw(&mut cv, *x + 3, *y, &label[*typed..], [0x11, 0x11, 0x1b], lw);
            }
            if let Overlay::Help(hp) = &self.overlay {
                help_rows = Some(crate::ui::help::draw(&mut cv, t, hp, bar));
            } else if let (Overlay::List(l), Some(pv)) = (&mut self.overlay, &self.pv) {
                plist::draw(&mut cv, t, l, &pv.targets, self.analysis.as_ref().map(|a| &a.page_map), bar);
            } else if let Overlay::Pick(p) = &mut self.overlay {
                pick::draw(&mut cv, t, p, bar);
            } else if self.mode == Mode::Index {
                index_rows = Some(index::draw(&mut cv, t, &mut self.index, index_auto, self.analysis.as_ref().map(|a| &a.page_map), bar));
            }
            if let Some(p) = self.popup.as_mut() {
                popup::draw(&mut cv, t, p);
            }
            if bar_visible {
                let st = Status {
                    name: &name,
                    message: msg.as_ref().map(|(m, e)| (m.as_str(), *e)),
                    page: page + 1,
                    printed: printed.as_deref(),
                    total,
                    zoom_pct,
                    mode: mode_name,
                    pending: &pending_keys,
                    job: job_text.as_deref(),
                    search: search_pos,
                };
                statusbar::draw(&mut cv, t, &st);
            } else if let (Some((m, e)), false) = (&msg, matches!(self.overlay, Overlay::Input(_))) {
                let y = cv.h - t.line_h - 4;
                cv.fill_rect(0, y, cv.w, t.line_h + 4, pm([0x11, 0x11, 0x1b], 0xcc));
                t.draw(&mut cv, 8, y + 2, m, if *e { crate::ui::ERR } else { FG }, full_w - 16);
            }
            if let Overlay::Input(bar_in) = &self.overlay {
                let y = cv.h - t.line_h - 6;
                cv.fill_rect(0, y, cv.w, t.line_h + 6, pm([0x1e, 0x1e, 0x2e], 0xff));
                let cw = t.width("M").max(1);
                let pre = bar_in.prefix.to_string();
                let mut x = t.draw(&mut cv, cw, y + 3, &pre, crate::ui::ACCENT, full_w);
                let before: String = bar_in.text[..bar_in.cursor].iter().collect();
                let after: String = bar_in.text[bar_in.cursor..].iter().collect();
                x = t.draw(&mut cv, x, y + 3, &before, FG, full_w);
                cv.fill_rect(x, y + 3, cw, t.line_h, pm(FG, 0xa0));
                t.draw(&mut cv, x, y + 3, &after, FG, full_w);
            }
        }
        let _ = window.pre_present_notify();
        let _ = buf.present();
        if let Some(r) = help_rows {
            self.help_rows = r;
        }
        if let Some(r) = index_rows {
            self.index_rows = r;
        }

        if page != self.title_page {
            self.title_page = page;
            if let Ok(mut i) = self.info.lock() {
                i.page = page as u32 + 1;
            }
            window.set_title(&format!("strata — {} [{}/{}]", self.name, page + 1, self.layout.len()));
        }
    }

    fn request_links(&mut self) {
        let (_, vh) = self.vp_pt();
        let vis = self.layout.visible(self.sy, self.sy + vh);
        if self.links.len() > 40 {
            let (a, b) = (*vis.start() as i64, *vis.end() as i64);
            self.links.retain(|&p, _| (p as i64) >= a - 10 && (p as i64) <= b + 10);
        }
        for p in vis {
            let p = p as u32;
            if !self.links.contains_key(&p) && self.links_pending.insert(p) {
                let _ = self.render.req_tx.send(Cmd::Links(p));
            }
        }
    }
}

/// Draw a stale tile (cached at another scale) scaled to stand in for a missing one.
fn fallback(cv: &mut Canvas, cache: &TileCache, layout: &Layout, p: &Placed, doc_h: i64, under: bool) {
    let k = p.key;
    let Some(old) = cache.nearest_scale(k.page, k.scale) else { return };
    let size = layout.size(k.page as usize);
    let (s_new, s_old) = (k.scale as f32 / 1000.0, old as f32 / 1000.0);
    let (w, h) = page_px(size, s_new);
    let (strip, _) = tile_grid(w, h);
    let (ow, oh) = page_px(size, s_old);
    let (ostrip, _) = tile_grid(ow, oh);
    let r = s_old / s_new;
    let th = strip.min(h - k.tile * strip) as i64;
    let (y0, y1) = (p.y.max(0), (p.y + th).min(doc_h).min(cv.h as i64));
    let (x0, x1) = (p.x.max(0), (p.x + w as i64).min(cv.w as i64));
    for y in y0..y1 {
        let src_row = (((k.tile * strip) as i64 + (y - p.y)) as f32 * r) as u32;
        let ti = src_row / ostrip;
        let key = TileKey { page: k.page, scale: old, tile: ti };
        let Some(t) = cache.peek(&key) else { continue };
        let ly = (src_row - ti * ostrip) as usize;
        if ly >= t.h as usize || t.w == 0 {
            continue;
        }
        let row = &t.data[ly * t.w as usize..(ly + 1) * t.w as usize];
        for x in x0..x1 {
            let sx = (((x - p.x) as f32 * r) as usize).min(t.w as usize - 1);
            let i = (y * cv.w as i64 + x) as usize;
            cv.buf[i] = over(row[sx], if under { 0xffff_ffff } else { cv.buf[i] });
        }
    }
}

fn key_input(ev: &KeyEvent, m: &ModifiersState) -> Option<(String, Option<char>)> {
    let (ctrl, shift) = (m.control_key(), m.shift_key());
    Some(match &ev.logical_key {
        Key::Named(n) => {
            let t = match n {
                NamedKey::Space if shift => "<S-Space>",
                NamedKey::Space => "<Space>",
                NamedKey::Enter => "<CR>",
                NamedKey::Tab if ctrl => "<C-i>",
                NamedKey::Tab => "<Tab>",
                NamedKey::Escape => "<Esc>",
                NamedKey::Backspace => "<BS>",
                NamedKey::Delete => "<Del>",
                NamedKey::ArrowUp => "<Up>",
                NamedKey::ArrowDown => "<Down>",
                NamedKey::ArrowLeft => "<Left>",
                NamedKey::ArrowRight => "<Right>",
                NamedKey::Home => "<Home>",
                NamedKey::End => "<End>",
                NamedKey::PageUp => "<PgUp>",
                NamedKey::PageDown => "<PgDn>",
                NamedKey::F1 => "<F1>",
                _ => return None,
            };
            (t.to_string(), if matches!(n, NamedKey::Space) && !ctrl { Some(' ') } else { None })
        }
        Key::Character(c) if ctrl => (format!("<C-{}>", c.to_lowercase()), None),
        Key::Character(c) => (c.to_string(), c.chars().next()),
        _ => return None,
    })
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("strata — {}", self.name))
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(900.0, 1000.0))
            .with_name("strata", "strata");
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let ctx = Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(Surface::new(&ctx, window.clone()).expect("softbuffer surface"));
        self.dpi = window.scale_factor() as f32;
        let sz = window.inner_size();
        self.vp = (sz.width, sz.height);
        self.window = Some(window);
        self.applied.font.1 = 0.0; // re-apply font size at the real dpi
        self.apply_config();
        if !self.restore_state() {
            self.exec(Command::FitWidth, None, el);
        }
        if let Some((f, l, c)) = self.start_synctex.take() {
            self.synctex_view(&f, l, c);
        }
        self.start_analysis(false, false);
    }

    /// The only timer in the program: armed while a hover is pending, otherwise `Wait`.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.hover.poll(Instant::now()) {
            popup::Poll::Fire => {
                self.hover_fire();
                el.set_control_flow(ControlFlow::Wait);
            }
            popup::Poll::Wait(d) => el.set_control_flow(ControlFlow::WaitUntil(d)),
            popup::Poll::Idle => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: ()) {
        while let Ok(e) = self.ext_rx.try_recv() {
            self.on_ext(e, el);
        }
        let cur = self.render.gen.load(Ordering::Relaxed);
        let mut changed = false;
        let mut tiles = false;
        while let Ok(r) = self.render.resp_rx.try_recv() {
            changed = true;
            match r {
                Response::Tile { key, gen, tile } if gen == cur => {
                    self.pending.remove(&key);
                    self.cache.insert(key, tile);
                    tiles = true;
                }
                Response::Tile { .. } => {}
                Response::Skipped { key } => {
                    self.pending.remove(&key);
                }
                Response::Hits { id, page, rects } => {
                    let hit = match self.search.as_mut() {
                        Some(s) if s.id == id => {
                            s.add(page, &rects);
                            if s.await_first {
                                s.await_first = false;
                                s.step(s.forward)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(h) = hit {
                        self.jump_to_hit(h);
                    }
                }
                Response::SearchDone { id } => {
                    if let Some(s) = self.search.as_mut().filter(|s| s.id == id) {
                        s.done = true;
                        if s.hits.is_empty() {
                            let q = s.query.clone();
                            self.err(format!("Pattern not found: {q}"));
                        }
                    }
                }
                Response::Links { page, links } => {
                    self.links_pending.remove(&page);
                    self.links.insert(page, links);
                }
                Response::Preview { id, tile } => {
                    if let Some(p) = self.popup.as_mut().filter(|p| p.id == id) {
                        if debug() {
                            eprintln!("popup {} rendered {}x{} px, hover={}", p.title, tile.w, tile.h, p.hover);
                        }
                        p.tile = Some(tile);
                    }
                }
                Response::Text(t) => {
                    if std::mem::take(&mut self.yank_pending) {
                        self.to_clipboard(t);
                    }
                }
                Response::Outline(items) => {
                    self.outline_items = items;
                    self.refresh_index();
                }
                Response::Opened(r) => self.on_opened(r),
            }
        }
        while let Ok((id, m)) = self.worker_rx.try_recv() {
            changed = true;
            self.on_worker(id, m);
        }
        if tiles {
            let vis = self.visible_tiles();
            if vis.iter().all(|p| self.cache.contains(&p.key)) {
                FIRST_TILE.call_once(|| {
                    if debug() {
                        eprintln!("strata: first complete frame after {:?}", START.get().map(|t| t.elapsed()).unwrap_or_default());
                    }
                });
                self.cache.retain_scale(self.scale_key());
            }
        }
        if changed {
            self.redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Focused(false) => self.save_state(),
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(sz) => {
                self.vp = (sz.width, sz.height);
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.dpi = scale_factor as f32;
                self.invalidate(false);
                self.apply_config();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.update_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.hover.cancel();
                if self.popup.as_ref().is_some_and(|p| p.hover) {
                    self.popup = None;
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                if matches!(self.overlay, Overlay::None) && self.mode == Mode::Normal {
                    if self.mods.control_key() && self.cfg.synctex {
                        self.synctex_edit();
                    } else {
                        self.click();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some((token, ch)) = key_input(&event, &self.mods) {
                    self.on_key(token, ch, el);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / (WHEEL_PX * self.dpi), p.y as f32 / (WHEEL_PX * self.dpi)),
                };
                let over_popup = self.popup.as_ref().is_some_and(|p| p.contains(self.cursor.0, self.cursor.1));
                match &mut self.overlay {
                    _ if over_popup => {
                        if let Some(p) = self.popup.as_mut() {
                            p.scroll_by((-dy * WHEEL_PX * self.dpi) as i32);
                        }
                    }
                    Overlay::List(l) => {
                        if let Some(pv) = &self.pv {
                            l.move_sel(-(dy * 3.0) as i32, &pv.targets);
                        }
                    }
                    Overlay::Pick(p) => p.move_sel(-(dy * 3.0) as i32),
                    Overlay::Help(h) => h.scroll_by(-(dy * 3.0) as i32, self.help_rows),
                    _ if self.mode == Mode::Index => {
                        if dy > 0.0 { self.index.up((dy * 3.0) as usize) } else { self.index.down((-dy * 3.0) as usize) }
                    }
                    _ if self.mods.control_key() => self.set_zoom(self.zoom * self.zoom_factor(1).powf(dy)),
                    _ => self.scroll_by(-dx * WHEEL_PX * self.dpi, -dy * WHEEL_PX * self.dpi),
                }
                self.redraw();
            }
            _ => {}
        }
    }
}

#[allow(dead_code)]
fn _assert_tile_send(_: Tile) {}

/// "3 h ago" style age for the recent-files list.
fn ago(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let d = (now - t).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86400),
    }
}
