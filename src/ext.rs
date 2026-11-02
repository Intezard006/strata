//! Integration with the outside world: signals and the D-Bus service (each on a dedicated thread that
//! blocks, so there are no timers or polling), plus the `--exec` / `--synctex-forward` client side.
//! External requests are queued on a channel and the event loop is woken through its proxy.

use dbus::arg::{RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::{MatchingReceiver, Sender};
use dbus::message::MatchRule;
use dbus::Message;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender as Tx;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

pub const NAME_PREFIX: &str = "org.strata.PID-";
const OBJECT: &str = "/org/strata";
const IFACE: &str = "org.strata";

/// A request from outside the UI thread.
#[derive(Debug, PartialEq)]
pub enum Ext {
    /// SIGTERM / SIGINT / SIGHUP: save and quit.
    Signal,
    Exec(String),
    Goto(u32),
    Synctex { input: String, line: u32, column: u32 },
    Open(PathBuf),
    /// Touchpad pinch: begin (with the wl_surface pointer), cumulative scale, end.
    PinchBegin(usize),
    PinchUpdate(f64),
    PinchEnd,
    /// The open file was written or replaced on disk (auto-reload).
    FileChanged,
    /// A message from a helper thread (clipboard, OCR): text and whether it is an error.
    Notice(String, bool),
}

/// What the D-Bus properties report; updated by the app when the page or file changes.
#[derive(Default, Clone)]
pub struct Info {
    pub filename: String,
    pub page: u32,
    pub pages: u32,
}

pub type SharedInfo = Arc<Mutex<Info>>;

pub fn spawn_signals(tx: Tx<Ext>, proxy: EventLoopProxy<()>) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let Ok(mut sig) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) else { return };
    let _ = std::thread::Builder::new().name("signals".into()).stack_size(64 << 10).spawn(move || {
        for _ in sig.forever() {
            let _ = tx.send(Ext::Signal);
            let _ = proxy.send_event(());
        }
    });
}

/// Start the D-Bus service. Failure (no session bus) is silent: the viewer works without it.
pub fn spawn_dbus(tx: Tx<Ext>, proxy: EventLoopProxy<()>, info: SharedInfo) {
    let _ = std::thread::Builder::new().name("dbus".into()).stack_size(256 << 10).spawn(move || {
        let Ok(conn) = Connection::new_session() else { return };
        let name = format!("{NAME_PREFIX}{}", std::process::id());
        if conn.request_name(name, false, true, false).is_err() {
            return;
        }
        conn.start_receive(
            MatchRule::new_method_call(),
            Box::new(move |msg, c| {
                if let Some(reply) = handle(&msg, &tx, &proxy, &info) {
                    let _ = c.send(reply);
                }
                true
            }),
        );
        // Blocks in poll(2) until a message arrives; the timeout is only the longest representable wait.
        while conn.process(Duration::from_millis(i32::MAX as u64)).is_ok() {}
    });
}

fn handle(msg: &Message, tx: &Tx<Ext>, proxy: &EventLoopProxy<()>, info: &SharedInfo) -> Option<Message> {
    if msg.path().as_deref() != Some(OBJECT) {
        return None; // default_reply answers unknown objects
    }
    let (iface, member) = (msg.interface()?, msg.member()?);
    let send = |e: Ext| {
        let _ = tx.send(e);
        let _ = proxy.send_event(());
    };
    let err = |m: &str| msg.error(&"org.freedesktop.DBus.Error.InvalidArgs".into(), &std::ffi::CString::new(m).unwrap());
    match (&*iface, &*member) {
        (IFACE, "ExecuteCommand") => match msg.read1::<&str>() {
            Ok(c) => send(Ext::Exec(c.to_string())),
            Err(_) => return Some(err("expected a string")),
        },
        (IFACE, "GotoPage") => match msg.read1::<u32>() {
            Ok(p) => send(Ext::Goto(p)),
            Err(_) => return Some(err("expected a page number")),
        },
        (IFACE, "OpenDocument") => match msg.read1::<&str>() {
            Ok(p) => send(Ext::Open(PathBuf::from(p))),
            Err(_) => return Some(err("expected a path")),
        },
        (IFACE, "SynctexView") => match msg.read3::<&str, u32, u32>() {
            Ok((i, l, c)) => send(Ext::Synctex { input: i.to_string(), line: l, column: c }),
            Err(_) => return Some(err("expected (s input, u line, u column)")),
        },
        ("org.freedesktop.DBus.Properties", "Get") => {
            let (i, p): (&str, &str) = msg.read2().ok()?;
            if i != IFACE {
                return Some(err("unknown interface"));
            }
            return Some(match prop(info, p) {
                Some(v) => msg.method_return().append1(Variant(v)),
                None => err("unknown property"),
            });
        }
        ("org.freedesktop.DBus.Properties", "GetAll") => {
            let map: std::collections::HashMap<String, Variant<Box<dyn RefArg>>> = ["filename", "pagenumber", "numberofpages"].iter().filter_map(|p| prop(info, p).map(|v| (p.to_string(), Variant(v)))).collect();
            return Some(msg.method_return().append1(map));
        }
        ("org.freedesktop.DBus.Introspectable", "Introspect") => return Some(msg.method_return().append1(INTROSPECT)),
        _ => return None,
    }
    Some(msg.method_return())
}

fn prop(info: &SharedInfo, name: &str) -> Option<Box<dyn RefArg>> {
    let i = info.lock().ok()?.clone();
    Some(match name {
        "filename" => Box::new(i.filename),
        "pagenumber" => Box::new(i.page),
        "numberofpages" => Box::new(i.pages),
        _ => return None,
    })
}

const INTROSPECT: &str = r#"<node><interface name="org.strata">
<method name="ExecuteCommand"><arg type="s" direction="in"/></method>
<method name="GotoPage"><arg type="u" direction="in"/></method>
<method name="SynctexView"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/></method>
<method name="OpenDocument"><arg type="s" direction="in"/></method>
<property name="filename" type="s" access="read"/>
<property name="pagenumber" type="u" access="read"/>
<property name="numberofpages" type="u" access="read"/>
</interface></node>"#;

// ---- client side ---------------------------------------------------------------------------

/// A running strata instance as seen by the CLI helpers.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub pid: u32,
    /// Process start time (clock ticks since boot): larger is newer.
    pub started: u64,
}

/// Which instance `--exec` talks to: an explicit pid, else the focused window's pid when it is a
/// strata instance, else the most recently started instance.
pub fn pick_target(explicit: Option<u32>, focused: Option<u32>, instances: &[Instance]) -> Option<u32> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(f) = focused.filter(|f| instances.iter().any(|i| i.pid == *f)) {
        return Some(f);
    }
    instances.iter().max_by_key(|i| (i.started, i.pid)).map(|i| i.pid)
}

fn start_time(pid: u32) -> u64 {
    // field 22 of /proc/PID/stat; the command name (field 2) may contain spaces, so split after its ')'
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(19).and_then(|v| v.parse().ok()))).unwrap_or(0)
}

pub fn instances(conn: &Connection) -> Vec<Instance> {
    let names: Vec<String> = conn.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", Duration::from_secs(2)).method_call("org.freedesktop.DBus", "ListNames", ()).map(|(n,): (Vec<String>,)| n).unwrap_or_default();
    names.iter().filter_map(|n| n.strip_prefix(NAME_PREFIX)?.parse::<u32>().ok()).map(|pid| Instance { pid, started: start_time(pid) }).collect()
}

/// The pid of Hyprland's focused window, if hyprctl exists and says so.
pub fn focused_pid() -> Option<u32> {
    let out = std::process::Command::new("hyprctl").args(["activewindow", "-j"]).output().ok()?;
    parse_active_pid(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_active_pid(json: &str) -> Option<u32> {
    serde_json::from_str::<serde_json::Value>(json).ok()?.get("pid")?.as_u64().map(|p| p as u32).filter(|&p| p > 0)
}

/// `strata --exec CMD [--pid N]`
pub fn run_exec(args: &[String]) -> std::process::ExitCode {
    let mut cmd = None;
    let mut pid = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pid" => pid = it.next().and_then(|v| v.parse::<u32>().ok()),
            _ if cmd.is_none() => cmd = Some(a.clone()),
            _ => cmd = cmd.map(|c| format!("{c} {a}")),
        }
    }
    let Some(cmd) = cmd else {
        eprintln!("usage: strata --exec '<command>' [--pid N]");
        return std::process::ExitCode::from(2);
    };
    let Ok(conn) = Connection::new_session() else {
        eprintln!("strata: no D-Bus session bus");
        return std::process::ExitCode::FAILURE;
    };
    let all = if pid.is_some() { Vec::new() } else { instances(&conn) };
    let focused = if pid.is_some() || all.len() < 2 { None } else { focused_pid() };
    let Some(target) = pick_target(pid, focused, &all) else {
        eprintln!("strata: no running instance");
        return std::process::ExitCode::FAILURE;
    };
    match call(&conn, target, "ExecuteCommand", (cmd,)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("strata: pid {target}: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn call<A: dbus::arg::AppendAll>(conn: &Connection, pid: u32, method: &str, args: A) -> Result<(), dbus::Error> {
    conn.with_proxy(format!("{NAME_PREFIX}{pid}"), OBJECT, Duration::from_secs(5)).method_call(IFACE, method, args)
}

/// Forward search against an instance that already shows `pdf`. Returns false when none does.
pub fn synctex_to_running(pdf: &Path, input: &str, line: u32, col: u32) -> bool {
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    let Ok(conn) = Connection::new_session() else { return false };
    let mut inst = instances(&conn);
    inst.sort_by_key(|i| std::cmp::Reverse(i.started));
    for i in inst {
        let p = conn.with_proxy(format!("{NAME_PREFIX}{}", i.pid), OBJECT, Duration::from_secs(2));
        let Ok(name) = p.get::<String>(IFACE, "filename") else { continue };
        if Path::new(&name) == pdf {
            return call(&conn, i.pid, "SynctexView", (input.to_string(), line, col)).is_ok();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(pid: u32, started: u64) -> Instance {
        Instance { pid, started }
    }

    #[test]
    fn target_selection() {
        let all = [inst(10, 500), inst(20, 900), inst(30, 100)];
        assert_eq!(pick_target(Some(77), Some(10), &all), Some(77), "explicit pid wins");
        assert_eq!(pick_target(None, Some(10), &all), Some(10), "focused strata window");
        assert_eq!(pick_target(None, Some(99), &all), Some(20), "focused window is not strata: newest");
        assert_eq!(pick_target(None, None, &all), Some(20));
        assert_eq!(pick_target(None, None, &[]), None);
    }

    #[test]
    fn hyprctl_json() {
        assert_eq!(parse_active_pid(r#"{"address":"0x1","pid": 4242,"class":"strata"}"#), Some(4242));
        assert_eq!(parse_active_pid("{}"), None);
        assert_eq!(parse_active_pid("not json"), None);
        assert_eq!(parse_active_pid(r#"{"pid": -1}"#), None);
    }

    #[test]
    fn own_start_time_is_found() {
        assert!(start_time(std::process::id()) > 0);
    }
}
