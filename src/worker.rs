//! Viewer side of the analysis child: spawn it niced in its own process group and relay its
//! stdout lines through a blocking reader thread (no polling).

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use winit::event_loop::EventLoopProxy;

#[derive(Debug, PartialEq)]
pub enum Msg {
    Progress(u32, u32),
    Status(String),
    Error(String),
    Done,
    /// The process ended (after any `Done`).
    Exited,
}

pub fn parse_line(l: &str) -> Option<Msg> {
    let (tag, rest) = l.split_once(' ').unwrap_or((l, ""));
    Some(match tag {
        "P" => {
            let (a, b) = rest.split_once(' ')?;
            Msg::Progress(a.parse().ok()?, b.parse().ok()?)
        }
        "S" => Msg::Status(rest.to_string()),
        "E" => Msg::Error(rest.to_string()),
        "D" => Msg::Done,
        _ => return None,
    })
}

pub struct Job {
    pub id: u64,
    pid: i32,
    child: Arc<Mutex<Child>>,
    exited: bool,
}

impl Job {
    /// `args` follow `strata`, e.g. `["--analyze", "/path/file.pdf"]`.
    pub fn spawn(id: u64, args: &[&str], tx: Sender<(u64, Msg)>, proxy: EventLoopProxy<()>) -> std::io::Result<Job> {
        let exe = std::env::current_exe()?;
        let build = |niced: bool| {
            let mut cmd = if niced {
                let mut c = Command::new("nice");
                c.args(["-n", "19", "ionice", "-c3"]).arg(&exe);
                c
            } else {
                Command::new(&exe)
            };
            cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
            // own process group: cancelling kills ocrmypdf and its workers too
            // and die with the viewer (SIGTERM; the child's handler then kills its group)
            let parent = unsafe { libc::getpid() };
            unsafe {
                cmd.pre_exec(move || {
                    libc::setsid();
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                    if libc::getppid() != parent {
                        libc::_exit(0); // the viewer is already gone
                    }
                    Ok(())
                });
            }
            cmd
        };
        let mut child = match build(true).spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => build(false).spawn()?,
            Err(e) => return Err(e),
        };
        let out = child.stdout.take().expect("piped stdout");
        let pid = child.id() as i32;
        let child = Arc::new(Mutex::new(child));
        let c2 = child.clone();
        std::thread::Builder::new().name("analysis-reader".into()).spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(m) = parse_line(&line) {
                    let _ = tx.send((id, m));
                    let _ = proxy.send_event(());
                }
            }
            let _ = c2.lock().map(|mut c| c.wait());
            let _ = tx.send((id, Msg::Exited));
            let _ = proxy.send_event(());
        })?;
        Ok(Job { id, pid, child, exited: false })
    }

    /// The process is gone; dropping the job must not signal a possibly reused pid.
    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    pub fn cancel(&self) {
        if self.exited {
            return;
        }
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
        let _ = self.child.lock().map(|mut c| c.kill());
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol() {
        assert_eq!(parse_line("P 12 552"), Some(Msg::Progress(12, 552)));
        assert_eq!(parse_line("S OCR running..."), Some(Msg::Status("OCR running...".into())));
        assert_eq!(parse_line("E boom"), Some(Msg::Error("boom".into())));
        assert_eq!(parse_line("D"), Some(Msg::Done));
        assert_eq!(parse_line("zzz"), None);
        assert_eq!(parse_line("P x y"), None);
    }
}
