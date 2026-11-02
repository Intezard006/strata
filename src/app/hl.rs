//! Text selection, highlights and notes, and snips to the clipboard (methods of `App`).

use super::*;

/// Pointer travel (logical pixels) before a press becomes a selection drag.
const SLOP: f64 = 4.0;

impl App {
    // ---- selection ----------------------------------------------------------------------

    pub(super) fn clear_selection(&mut self) {
        self.sel = None;
        self.sel_pts = None;
        self.sel_chars = None;
        self.sel_chars_pending = None;
        self.drag = Drag::None;
    }

    /// The window point as (page, x, y in that page's points), for any page (the point may lie outside it).
    fn point_on(&self, p: usize, mx: f64, my: f64) -> (f32, f32) {
        let s = self.scale();
        (self.x0() + mx as f32 / s - self.layout.left(p), self.sy + my as f32 / s - self.layout.top(p))
    }

    pub(super) fn mouse_press(&mut self) {
        let (mx, my) = self.cursor;
        if my >= self.doc_h() as f64 {
            return;
        }
        if self.snip_armed || self.mods.alt_key() {
            self.drag = Drag::Snip { from: (mx, my), to: (mx, my) };
            return;
        }
        let had = self.sel.is_some();
        self.clear_selection();
        self.drag = Drag::Select { press: (mx, my), moved: false };
        if had {
            self.redraw();
        }
    }

    pub(super) fn mouse_move(&mut self) {
        let (mx, my) = self.cursor;
        match &mut self.drag {
            Drag::None => {}
            Drag::Snip { to, .. } => {
                *to = (mx, my);
                self.redraw();
            }
            Drag::Select { press, moved } => {
                if !*moved && (press.0 - mx).hypot(press.1 - my) < SLOP * self.dpi as f64 {
                    return;
                }
                let press = *press;
                *moved = true;
                let s = self.scale();
                let (xp, yp) = (self.x0() + press.0 as f32 / s, self.sy + press.1 as f32 / s);
                let page = match &self.sel_pts {
                    Some(sp) => sp.page,
                    None => self.layout.page_at_xy(xp, yp) as u32,
                };
                let from = match &self.sel_pts {
                    Some(sp) => sp.from,
                    None => self.point_on(page as usize, press.0, press.1),
                };
                let to = self.point_on(page as usize, mx, my);
                self.sel_pts = Some(SelPts { page, from, to });
                if self.sel_chars.as_ref().map(|c| c.0) != Some(page) && self.sel_chars_pending != Some(page) {
                    self.sel_chars_pending = Some(page);
                    let _ = self.render.req_tx.send(Cmd::PageChars { page });
                }
                self.recompute_selection();
                self.redraw();
            }
        }
    }

    pub(super) fn mouse_release(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Select { moved: false, .. } => {
                if matches!(self.overlay, Overlay::None) && self.mode == Mode::Normal {
                    self.click();
                }
            }
            Drag::Snip { from, to } => self.finish_snip(from, to),
            _ => {}
        }
    }

    pub(super) fn on_page_chars(&mut self, page: u32, chars: Vec<Ch>) {
        if self.sel_chars_pending == Some(page) {
            self.sel_chars_pending = None;
            if self.sel_pts.as_ref().is_some_and(|s| s.page == page) {
                self.sel_chars = Some((page, chars));
                self.recompute_selection();
            }
        }
    }

    fn recompute_selection(&mut self) {
        let (Some(sp), Some((cp, chars))) = (&self.sel_pts, &self.sel_chars) else { return };
        if sp.page != *cp {
            return;
        }
        let (Some(a), Some(b)) = (select::nearest(chars, sp.from.0, sp.from.1), select::nearest(chars, sp.to.0, sp.to.1)) else {
            self.sel = None;
            return;
        };
        let (lo, hi) = select::span(a, b);
        let (hl, color) = self.sel.as_ref().map_or((None, self.sel_color), |s| (s.hl, s.color));
        self.sel = Some(Selection { page: sp.page, rects: select::rects(chars, lo, hi), text: select::text(chars, lo, hi), hl, color });
    }

    pub(super) fn copy_selection(&mut self) {
        match self.sel.as_ref().map(|s| s.text.clone()).filter(|t| !t.is_empty()) {
            Some(t) => self.to_clipboard(t),
            None => self.err("No selection (drag over text)".into()),
        }
    }

    // ---- highlights ---------------------------------------------------------------------

    /// Make a highlight of the selection (once); returns its id.
    fn ensure_highlight(&mut self) -> Option<i64> {
        let sel = self.sel.as_mut()?;
        if let Some(id) = sel.hl {
            return Some(id);
        }
        let (db, key) = (self.db.as_ref()?, self.key.as_ref()?);
        let h = Highlight { id: 0, page: sel.page, rects: sel.rects.clone(), color: sel.color, text: sel.text.clone(), note: String::new(), created: 0 };
        let id = db.add_highlight(key, &h)?;
        sel.hl = Some(id);
        self.hl_any = true;
        self.hl_cache.remove(&h.page);
        Some(id)
    }

    pub(super) fn highlight_selection(&mut self) {
        if self.sel.as_ref().is_none_or(|s| s.rects.is_empty()) {
            return self.err("No selection (drag over text)".into());
        }
        if self.sel.as_ref().is_some_and(|s| s.hl.is_some()) {
            return self.msg("Already highlighted (:hdelete removes it)".into());
        }
        match self.ensure_highlight() {
            Some(_) => self.msg("Highlighted".into()),
            None => self.err("Cannot save the highlight (no state database)".into()),
        }
    }

    pub(super) fn set_hl_color(&mut self, n: u8) {
        let c = n.clamp(1, 4) - 1;
        self.sel_color = c;
        let Some(sel) = self.sel.as_mut() else { return };
        sel.color = c;
        if let (Some(id), Some(db)) = (sel.hl, &self.db) {
            db.set_highlight_color(id, c);
            self.hl_cache.remove(&sel.page);
        }
    }

    pub(super) fn note_selection(&mut self, text: String) {
        if self.sel.as_ref().is_none_or(|s| s.rects.is_empty()) {
            return self.err("No selection: drag over text, then :note TEXT".into());
        }
        let Some(id) = self.ensure_highlight() else { return self.err("Cannot save the note (no state database)".into()) };
        if let Some(db) = &self.db {
            db.set_highlight_note(id, &text);
        }
        if let Some(sel) = &self.sel {
            self.hl_cache.remove(&sel.page);
        }
        self.msg("Note attached (click its margin marker to read it)".into());
    }

    pub(super) fn delete_selection_highlight(&mut self) {
        let Some(sel) = self.sel.as_mut() else { return self.err("No selection".into()) };
        let Some(id) = sel.hl.take() else { return self.err("The selection is not highlighted".into()) };
        let page = sel.page;
        if let Some(db) = &self.db {
            db.delete_highlight(id);
        }
        self.hl_cache.remove(&page);
        self.msg("Highlight deleted".into());
    }

    /// Load the highlights of the pages in view (one indexed query per new page; nothing when the
    /// document has none).
    fn load_visible_highlights(&mut self) {
        if !self.hl_any {
            return;
        }
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return };
        let vis = self.shown_pages(self.vp_pt().1);
        if self.hl_cache.len() > 24 {
            let (a, b) = (*vis.start() as i64, *vis.end() as i64);
            self.hl_cache.retain(|&p, _| (p as i64) >= a - 4 && (p as i64) <= b + 4);
        }
        for p in vis {
            self.hl_cache.entry(p as u32).or_insert_with(|| db.highlights_on_page(key, p as u32));
        }
    }

    /// Filled rectangles (window pixels, premultiplied colour) for highlights, note markers, the
    /// selection and the snip rubber band, drawn over the page.
    pub(super) fn mark_rects(&mut self) -> Vec<(i32, i32, i32, i32, u32)> {
        let mut out = Vec::new();
        if self.in_float {
            return out;
        }
        self.load_visible_highlights();
        let s = self.scale();
        let px = |ox: i64, oy: i64, r: [f32; 4]| ((ox as f32 + r[0] * s) as i32, (oy as f32 + r[1] * s) as i32, ((r[2] - r[0]) * s).ceil() as i32, ((r[3] - r[1]) * s).ceil() as i32);
        let vis = self.shown_pages(self.vp_pt().1);
        if self.hl_any {
            for p in vis.clone() {
                let Some(hs) = self.hl_cache.get(&p.try_into().unwrap_or(0)) else { continue };
                let (ox, oy) = self.page_origin(p);
                let pw = self.layout.size(p).0;
                for h in hs {
                    let c = pm(select::COLORS[(h.color as usize).min(3)], 0x66);
                    for r in &h.rects {
                        let (x, y, w, hh) = px(ox, oy, *r);
                        out.push((x, y, w, hh, c));
                    }
                    if let Some(m) = select::marker_rect(h, pw) {
                        let (x, y, w, hh) = px(ox, oy, m);
                        out.push((x, y, w, hh, pm(crate::ui::ACCENT, 0xf0)));
                        out.push((x + w / 4, y + hh / 4, w / 2, hh / 2, pm([0x11, 0x11, 0x1b], 0xff)));
                    }
                }
            }
        }
        if let Some(sel) = &self.sel {
            if vis.contains(&(sel.page as usize)) {
                let (ox, oy) = self.page_origin(sel.page as usize);
                let edge = pm(crate::ui::ACCENT, 0x70);
                for r in &sel.rects {
                    let (x, y, w, h) = px(ox, oy, *r);
                    if sel.hl.is_none() {
                        out.push((x, y, w, h, pm(crate::ui::ACCENT, 0x55)));
                    }
                    out.push((x, y + h - 1, w, 1, edge));
                }
            }
        }
        if let Drag::Snip { from, to } = &self.drag {
            let r = select::norm_rect(*from, *to);
            let (x, y, w, h) = (r[0] as i32, r[1] as i32, (r[2] - r[0]) as i32, (r[3] - r[1]) as i32);
            let e = pm(crate::ui::ACCENT, 0xff);
            out.push((x, y, w, h, pm(crate::ui::ACCENT, 0x28)));
            out.extend([(x, y, w, 2, e), (x, y + h - 2, w, 2, e), (x, y, 2, h, e), (x + w - 2, y, 2, h, e)]);
        }
        out
    }

    /// A click on a note marker opens the note in the popup panel. Returns true when it did.
    pub(super) fn click_marker(&mut self) -> bool {
        if !self.hl_any {
            return false;
        }
        let (mx, my) = self.cursor;
        let s = self.scale();
        let (xp, yp) = (self.x0() + mx as f32 / s, self.sy + my as f32 / s);
        let (p, lx, ly) = self.page_point(xp, yp);
        let pw = self.layout.size(p).0;
        let Some(h) = self.hl_cache.get(&(p as u32)).and_then(|hs| hs.iter().find(|h| select::marker_rect(h, pw).is_some_and(|m| select::hit(m, (lx, ly), 2.0)))).cloned() else { return false };
        let map = self.analysis.as_ref().map(|a| &a.page_map);
        let title = format!("Note — p.{}", pages::label(map, h.page));
        self.popup = Some(Popup { target: u32::MAX, id: 0, tile: None, zoom: 1.0, scroll: 0, hover: false, anchor: (mx as i32, my as i32), ref_rect: None, title, note: Some(h.note), rect: (0, 0, 0, 0) });
        self.mode = Mode::Popup;
        true
    }

    // ---- list, export, write ------------------------------------------------------------

    pub(super) fn open_hlist(&mut self) {
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return self.err("No state database".into()) };
        let map = self.analysis.as_ref().map(|a| &a.page_map);
        let items: Vec<PickItem> = db
            .highlights(key)
            .into_iter()
            .map(|h| {
                let one_line = h.text.split_whitespace().collect::<Vec<_>>().join(" ");
                PickItem {
                    label: one_line,
                    detail: if h.note.is_empty() { String::new() } else { format!("[{}]", h.note.replace('\n', " ")) },
                    right: format!("p.{}", pages::label(map, h.page)),
                    act: Act::Highlight(h.id, h.page, h.rects.first().map_or(0.0, |r| r[1])),
                    group: Group::Plain,
                    bonus: 0,
                }
            })
            .collect();
        self.overlay = Overlay::Pick(Pick::new(PickKind::Highlights, "Highlights", items));
    }

    /// Ctrl-X in the highlight list.
    pub(super) fn hlist_delete_current(&mut self) {
        let Overlay::Pick(pk) = &mut self.overlay else { return };
        let Some(Act::Highlight(id, page, _)) = pk.current().map(|i| i.act.clone()) else { return };
        pk.remove_current();
        if let Some(db) = &self.db {
            db.delete_highlight(id);
        }
        self.hl_cache.remove(&page);
        if self.sel.as_ref().is_some_and(|s| s.hl == Some(id)) {
            self.clear_selection();
        }
    }

    fn out_path(&self, path: Option<String>, suffix: &str) -> PathBuf {
        match path {
            Some(p) => {
                let p = PathBuf::from(p);
                if p.is_relative() { std::env::current_dir().unwrap_or_default().join(p) } else { p }
            }
            None => {
                let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.path.with_file_name(format!("{stem}.{suffix}"))
            }
        }
    }

    pub(super) fn export_highlights(&mut self, path: Option<String>) {
        let (Some(db), Some(key)) = (&self.db, &self.key) else { return self.err("No state database".into()) };
        let hs = db.highlights(key);
        if hs.is_empty() {
            return self.err("No highlights in this document".into());
        }
        let dst = self.out_path(path, "highlights.md");
        if dst == self.path {
            return self.err("Refusing to overwrite the original".into());
        }
        let map = self.analysis.as_ref().map(|a| &a.page_map);
        let md = select::markdown(&self.name, &hs, |p| pages::label(map, p));
        match std::fs::write(&dst, md) {
            Ok(()) => self.msg(format!("Wrote {} ({} highlights)", dst.display(), hs.len())),
            Err(e) => self.err(format!("{}: {e}", dst.display())),
        }
    }

    pub(super) fn write_highlights_pdf(&mut self, path: Option<String>, force: bool) {
        let (Some(db), Some(key)) = (&self.db, self.key.clone()) else { return self.err("No state database".into()) };
        if db.highlight_count(&key) == 0 {
            return self.err("No highlights in this document".into());
        }
        if self.job.is_some() {
            return self.err("A background job is running; try again when it finishes".into());
        }
        let dst = self.out_path(path, "highlighted.pdf");
        if dst == self.path {
            return self.err("Refusing to overwrite the original".into());
        }
        if dst.exists() && !force {
            return self.err(format!("{} exists; use :hwrite! to overwrite", dst.display()));
        }
        let (src, d) = (self.path.display().to_string(), dst.display().to_string());
        self.spawn_job(&["--write-highlights", &src, &d, &key], JobKind::WriteHighlights(dst));
        self.job_status = Some("writing highlights...".into());
    }

    // ---- snips --------------------------------------------------------------------------

    pub(super) fn arm_snip(&mut self, text: bool) {
        self.snip_armed = true;
        self.snip_text = text;
        self.msg("Drag a rectangle (Esc cancels)".into());
    }

    fn finish_snip(&mut self, from: (f64, f64), to: (f64, f64)) {
        let text = std::mem::take(&mut self.snip_armed) && self.snip_text;
        let r = select::norm_rect(from, to);
        if r[2] - r[0] < 6.0 || r[3] - r[1] < 6.0 {
            self.redraw();
            return;
        }
        let s = self.scale();
        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let page = self.layout.page_at_xy(self.x0() + cx as f32 / s, self.sy + cy as f32 / s);
        let (x0, y0) = self.point_on(page, r[0], r[1]);
        let (x1, y1) = self.point_on(page, r[2], r[3]);
        let (pw, ph) = self.layout.size(page);
        let Some(rect) = select::clip([x0, y0, x1, y1], [0.0, 0.0, pw, ph], 2.0) else {
            self.err("Snip is outside the page".into());
            return self.redraw();
        };
        self.snip_id += 1;
        let scale = select::snip_scale(rect, s, if text { 4.0 } else { 2.0 }, 6.0e6);
        let recolor = (!self.cfg.snip_original).then(|| self.applied.recolor.with_enabled(self.cfg.recolor.enabled));
        let _ = self.render.req_tx.send(Cmd::Snip(SnipReq { id: self.snip_id, page: page as u32, rect, scale, recolor, text }));
        self.msg(if text { "OCR of the snip...".into() } else { "Rendering the snip...".into() });
        self.redraw();
    }

    pub(super) fn on_snip(&mut self, png: Option<Vec<u8>>, text: bool) {
        let Some(png) = png else { return self.err("Snip failed (render error)".into()) };
        let (tx, proxy) = (self.ext_tx.clone(), self.proxy.clone());
        let notify = move |m: String, e: bool| {
            let _ = tx.send(Ext::Notice(m, e));
            let _ = proxy.send_event(());
        };
        if !text {
            let n = png.len();
            let spawned = std::thread::Builder::new().name("snip".into()).spawn(move || match wl_copy(&png, Some("image/png")) {
                Ok(()) => notify(format!("Copied image ({} KB)", n / 1024), false),
                Err(e) => notify(format!("wl-copy: {e}"), true),
            });
            if spawned.is_err() {
                self.err("Cannot start the clipboard thread".into());
            }
            return;
        }
        let spawned = std::thread::Builder::new().name("snip-ocr".into()).spawn(move || {
            let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let file = dir.join(format!("strata-snip-{}.png", std::process::id()));
            if let Err(e) = std::fs::write(&file, &png) {
                return notify(format!("snip: {e}"), true);
            }
            drop(png);
            let out = std::process::Command::new("nice").args(["-n", "19", "tesseract"]).arg(&file).arg("-").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
            let _ = std::fs::remove_file(&file);
            match out {
                Ok(o) if o.status.success() => {
                    let t = String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
                    if t.is_empty() {
                        return notify("OCR found no text".into(), true);
                    }
                    match wl_copy(t.as_bytes(), None) {
                        Ok(()) => notify(format!("Copied OCR text ({} chars)", t.chars().count()), false),
                        Err(e) => notify(format!("wl-copy: {e}"), true),
                    }
                }
                Ok(_) => notify("tesseract failed".into(), true),
                Err(e) => notify(format!("tesseract: {e}"), true),
            }
        });
        if spawned.is_err() {
            self.err("Cannot start the OCR thread".into());
        }
    }
}

/// Hand `data` to `wl-copy` (it keeps serving the selection in the background).
pub(super) fn wl_copy(data: &[u8], mime: Option<&str>) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("wl-copy");
    if let Some(m) = mime {
        cmd.args(["--type", m]);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    if let Some(mut si) = child.stdin.take() {
        si.write_all(data)?;
    }
    child.wait().map(|_| ())
}

/// `strata --write-highlights SRC DST KEY`: copy SRC to DST with the stored highlights as PDF annotations.
pub fn write_highlights(args: &[String]) -> std::process::ExitCode {
    let (Some(src), Some(dst), Some(key)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("usage: strata --write-highlights <src> <dst> <key>");
        return std::process::ExitCode::from(2);
    };
    match write_highlights_inner(src, dst, key) {
        Ok(()) => {
            crate::analyze::say("D");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            crate::analyze::say(&format!("E {e}"));
            std::process::ExitCode::FAILURE
        }
    }
}

fn write_highlights_inner(src: &str, dst: &str, key: &str) -> Result<(), String> {
    use mupdf::color::AnnotationColor;
    use mupdf::pdf::{AnnotationQuadPoints, PdfDocument};
    use mupdf::{Point, Quad};
    if src == dst {
        return Err("refusing to overwrite the original".into());
    }
    let db = Db::open(&db::db_path()).map_err(|e| e.to_string())?;
    let hs = db.highlights(key);
    if hs.is_empty() {
        return Err("no highlights".into());
    }
    let pdf = PdfDocument::open(src).map_err(|_| "not a PDF".to_string())?;
    for h in &hs {
        let Ok(mut page) = pdf.load_pdf_page(h.page as i32) else { continue };
        let Ok(b) = page.bounds() else { continue };
        let quads: Vec<Quad> = h
            .rects
            .iter()
            .map(|r| {
                let (x0, y0, x1, y1) = (r[0] + b.x0, r[1] + b.y0, r[2] + b.x0, r[3] + b.y0);
                Quad::new(Point::new(x0, y0), Point::new(x1, y0), Point::new(x0, y1), Point::new(x1, y1))
            })
            .collect();
        if quads.is_empty() {
            continue;
        }
        let mut a = page.add_highlight_annotation(AnnotationQuadPoints::new(quads)).map_err(|e| e.to_string())?;
        let c = select::COLORS[(h.color as usize).min(3)];
        let _ = a.set_color(AnnotationColor::Rgb { red: c[0] as f32 / 255.0, green: c[1] as f32 / 255.0, blue: c[2] as f32 / 255.0 });
        if !h.note.is_empty() {
            let _ = a.set_contents(&h.note);
        }
        let _ = a.update();
    }
    pdf.save(dst).map_err(|e| e.to_string())
}
