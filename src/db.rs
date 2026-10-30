//! Persistent state in SQLite (system libsqlite3): positions, bookmarks, marks, jumplists, recent files.
//! One connection, WAL, cached prepared statements, no background activity: every write is made by
//! the caller at an event (close, focus loss, `open`, signal, bookmark/mark commands).

use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub const JUMPS_MAX: usize = 100;

/// Where the reader was: page, offset below that page's top (points), zoom and recolor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pos {
    pub page: u32,
    pub y: f32,
    pub x: f32,
    pub zoom: f32,
    pub recolor: Option<bool>,
}

pub struct Db {
    conn: Connection,
}

pub fn db_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("strata")
        .join("strata.db")
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS docs(
  key TEXT PRIMARY KEY, path TEXT NOT NULL, opened INTEGER NOT NULL,
  page INTEGER, y REAL, x REAL, zoom REAL, recolor INTEGER);
CREATE INDEX IF NOT EXISTS docs_path ON docs(path);
CREATE TABLE IF NOT EXISTS bookmarks(
  key TEXT NOT NULL, name TEXT NOT NULL, page INTEGER NOT NULL, y REAL NOT NULL, made INTEGER NOT NULL,
  PRIMARY KEY(key, name));
CREATE TABLE IF NOT EXISTS marks(
  key TEXT NOT NULL, ch TEXT NOT NULL, page INTEGER NOT NULL, y REAL NOT NULL,
  PRIMARY KEY(key, ch));
CREATE TABLE IF NOT EXISTS gmarks(
  ch TEXT PRIMARY KEY, key TEXT NOT NULL, path TEXT NOT NULL, page INTEGER NOT NULL, y REAL NOT NULL);
CREATE TABLE IF NOT EXISTS jumps(
  key TEXT PRIMARY KEY, pos INTEGER NOT NULL, list TEXT NOT NULL);
";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
        // The returned row of the pragma is ignored on purpose.
        let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()));
        let _ = conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;");
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn memory() -> Db {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(conn: Connection) -> rusqlite::Result<Db> {
        conn.execute_batch(SCHEMA)?;
        conn.set_prepared_statement_cache_capacity(24);
        Ok(Db { conn })
    }

    /// Record that `key` was opened at `path` (updates the path when the file moved).
    pub fn note_open(&self, key: &str, path: &Path) {
        let p = path.to_string_lossy();
        let _ = self.conn.prepare_cached("INSERT INTO docs(key,path,opened) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET path=?2, opened=?3").and_then(|mut s| s.execute(params![key, p, now()]));
    }

    pub fn save_pos(&self, key: &str, path: &Path, p: &Pos) {
        let rc = p.recolor.map(|b| b as i64);
        let _ = self.conn.prepare_cached(
            "INSERT INTO docs(key,path,opened,page,y,x,zoom,recolor) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) \
             ON CONFLICT(key) DO UPDATE SET path=?2, page=?4, y=?5, x=?6, zoom=?7, recolor=?8",
        ).and_then(|mut s| s.execute(params![key, path.to_string_lossy(), now(), p.page, p.y, p.x, p.zoom, rc]));
    }

    /// The saved position for `key`; failing that, for the same path (the file was rebuilt).
    pub fn load_pos(&self, key: &str, path: &Path) -> Option<Pos> {
        let row = |r: &rusqlite::Row| -> rusqlite::Result<Option<Pos>> {
            let page: Option<u32> = r.get(0)?;
            Ok(page.map(|page| Pos { page, y: r.get(1).unwrap_or(0.0), x: r.get(2).unwrap_or(0.0), zoom: r.get(3).unwrap_or(1.0), recolor: r.get::<_, Option<i64>>(4).ok().flatten().map(|v| v != 0) }))
        };
        let by_key = self.conn.prepare_cached("SELECT page,y,x,zoom,recolor FROM docs WHERE key=?1").ok()?.query_row([key], row).optional().ok().flatten().flatten();
        by_key.or_else(|| {
            self.conn.prepare_cached("SELECT page,y,x,zoom,recolor FROM docs WHERE path=?1 AND page IS NOT NULL ORDER BY opened DESC LIMIT 1").ok()?.query_row([path.to_string_lossy()], row).optional().ok().flatten().flatten()
        })
    }

    pub fn recent(&self, limit: usize) -> Vec<(PathBuf, i64)> {
        let Ok(mut s) = self.conn.prepare_cached("SELECT path,opened FROM docs ORDER BY opened DESC LIMIT ?1") else { return Vec::new() };
        s.query_map([limit as i64], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)?))).map(|it| it.flatten().collect()).unwrap_or_default()
    }

    pub fn set_bookmark(&self, key: &str, name: &str, page: u32, y: f32) {
        let _ = self.conn.prepare_cached("INSERT OR REPLACE INTO bookmarks(key,name,page,y,made) VALUES(?1,?2,?3,?4,?5)").and_then(|mut s| s.execute(params![key, name, page, y, now()]));
    }

    pub fn bookmark(&self, key: &str, name: &str) -> Option<(u32, f32)> {
        self.conn.prepare_cached("SELECT page,y FROM bookmarks WHERE key=?1 AND name=?2").ok()?.query_row(params![key, name], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten()
    }

    /// (name, page, y) ordered by page.
    pub fn bookmarks(&self, key: &str) -> Vec<(String, u32, f32)> {
        let Ok(mut s) = self.conn.prepare_cached("SELECT name,page,y FROM bookmarks WHERE key=?1 ORDER BY page,y,name") else { return Vec::new() };
        s.query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|it| it.flatten().collect()).unwrap_or_default()
    }

    pub fn delete_bookmark(&self, key: &str, name: &str) -> bool {
        self.conn.prepare_cached("DELETE FROM bookmarks WHERE key=?1 AND name=?2").and_then(|mut s| s.execute(params![key, name])).map_or(false, |n| n > 0)
    }

    pub fn set_mark(&self, key: &str, ch: char, page: u32, y: f32) {
        let _ = self.conn.prepare_cached("INSERT OR REPLACE INTO marks(key,ch,page,y) VALUES(?1,?2,?3,?4)").and_then(|mut s| s.execute(params![key, ch.to_string(), page, y]));
    }

    pub fn mark(&self, key: &str, ch: char) -> Option<(u32, f32)> {
        self.conn.prepare_cached("SELECT page,y FROM marks WHERE key=?1 AND ch=?2").ok()?.query_row(params![key, ch.to_string()], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten()
    }

    pub fn set_gmark(&self, ch: char, key: &str, path: &Path, page: u32, y: f32) {
        let _ = self.conn.prepare_cached("INSERT OR REPLACE INTO gmarks(ch,key,path,page,y) VALUES(?1,?2,?3,?4,?5)").and_then(|mut s| s.execute(params![ch.to_string(), key, path.to_string_lossy(), page, y]));
    }

    /// (key, path, page, y)
    pub fn gmark(&self, ch: char) -> Option<(String, PathBuf, u32, f32)> {
        self.conn.prepare_cached("SELECT key,path,page,y FROM gmarks WHERE ch=?1").ok()?.query_row([ch.to_string()], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get(2)?, r.get(3)?))).optional().ok().flatten()
    }

    pub fn save_jumps(&self, key: &str, pos: usize, list: &[f32]) {
        let list = &list[list.len().saturating_sub(JUMPS_MAX)..];
        let text: Vec<String> = list.iter().map(|v| v.to_string()).collect();
        let pos = pos.min(list.len());
        let _ = self.conn.prepare_cached("INSERT OR REPLACE INTO jumps(key,pos,list) VALUES(?1,?2,?3)").and_then(|mut s| s.execute(params![key, pos as i64, text.join(",")]));
    }

    pub fn load_jumps(&self, key: &str) -> Option<(usize, Vec<f32>)> {
        let (pos, text): (i64, String) = self.conn.prepare_cached("SELECT pos,list FROM jumps WHERE key=?1").ok()?.query_row([key], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten()?;
        let list: Vec<f32> = text.split(',').filter_map(|v| v.parse().ok()).take(JUMPS_MAX).collect();
        Some(((pos.max(0) as usize).min(list.len()), list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_round_trip_and_moved_file() {
        let db = Db::memory();
        let p = Pos { page: 41, y: 120.5, x: 3.0, zoom: 1.75, recolor: Some(false) };
        assert_eq!(db.load_pos("k1", Path::new("/a.pdf")), None);
        db.save_pos("k1", Path::new("/a.pdf"), &p);
        assert_eq!(db.load_pos("k1", Path::new("/a.pdf")), Some(p));
        // the file moved: same key, new path
        db.save_pos("k1", Path::new("/b/a.pdf"), &Pos { recolor: None, ..p });
        assert_eq!(db.load_pos("k1", Path::new("/zzz")).unwrap().recolor, None);
        assert_eq!(db.recent(10)[0].0, Path::new("/b/a.pdf"));
        assert_eq!(db.recent(10).len(), 1);
        // rebuilt file (new key) at the same path falls back to the path
        assert_eq!(db.load_pos("k2", Path::new("/b/a.pdf")).unwrap().page, 41);
        assert_eq!(db.load_pos("k2", Path::new("/other.pdf")), None);
    }

    #[test]
    fn bookmarks_marks_and_global_marks() {
        let db = Db::memory();
        db.set_bookmark("k", "ch3", 30, 5.0);
        db.set_bookmark("k", "intro", 2, 0.0);
        db.set_bookmark("k", "ch3", 31, 6.0); // replaces
        assert_eq!(db.bookmark("k", "ch3"), Some((31, 6.0)));
        assert_eq!(db.bookmarks("k").iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["intro", "ch3"]);
        assert!(db.bookmarks("other").is_empty());
        assert!(db.delete_bookmark("k", "intro"));
        assert!(!db.delete_bookmark("k", "intro"));
        db.set_mark("k", 'a', 7, 1.5);
        assert_eq!(db.mark("k", 'a'), Some((7, 1.5)));
        assert_eq!(db.mark("k", 'b'), None);
        assert_eq!(db.mark("j", 'a'), None, "marks are per document");
        db.set_gmark('A', "k", Path::new("/a.pdf"), 9, 2.0);
        assert_eq!(db.gmark('A'), Some(("k".into(), PathBuf::from("/a.pdf"), 9, 2.0)));
        assert_eq!(db.gmark('B'), None);
    }

    #[test]
    fn jumplist_is_bounded() {
        let db = Db::memory();
        let list: Vec<f32> = (0..250).map(|i| i as f32).collect();
        db.save_jumps("k", 250, &list);
        let (pos, got) = db.load_jumps("k").unwrap();
        assert_eq!(got.len(), JUMPS_MAX);
        assert_eq!(got[0], 150.0, "oldest entries are dropped");
        assert_eq!(pos, JUMPS_MAX);
        assert_eq!(db.load_jumps("nope"), None);
    }

    #[test]
    fn file_db_uses_wal() {
        let dir = std::env::temp_dir().join(format!("strata-dbtest-{}", std::process::id()));
        let path = dir.join("sub").join("strata.db");
        let db = Db::open(&path).unwrap();
        db.set_bookmark("k", "x", 1, 1.0);
        let mode: String = db.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        drop(db);
        assert_eq!(Db::open(&path).unwrap().bookmark("k", "x"), Some((1, 1.0)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
