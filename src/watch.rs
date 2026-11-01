//! Auto-reload: an inotify watch on the open file's directory. A thread blocks in poll(2) on the inotify fd
//! and an eventfd (to stop it); there is no polling and no timer. Watching the directory rather than the
//! file survives write-then-rename savers such as LaTeX tooling. Without the option, none of this exists.

use crate::ext::Ext;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::mpsc::Sender;
use winit::event_loop::EventLoopProxy;

pub struct Watch {
    efd: i32,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Watch `path`'s directory for it being written or replaced. `None` when inotify is unavailable.
    pub fn spawn(path: &Path, tx: Sender<Ext>, proxy: EventLoopProxy<()>) -> Option<Watch> {
        let name: OsString = path.file_name()?.to_owned();
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let ifd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if ifd < 0 {
            return None;
        }
        let cdir = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let wd = unsafe { libc::inotify_add_watch(ifd, cdir.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) };
        let efd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
        if wd < 0 || efd < 0 {
            unsafe {
                libc::close(ifd);
                if efd >= 0 {
                    libc::close(efd);
                }
            }
            return None;
        }
        let thread = std::thread::Builder::new()
            .name("watch".into())
            .stack_size(64 << 10)
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    let mut fds = [libc::pollfd { fd: ifd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: efd, events: libc::POLLIN, revents: 0 }];
                    let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                    if r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        break;
                    }
                    if fds[1].revents != 0 {
                        break;
                    }
                    // drain everything pending, so a burst of writes is one reload
                    let mut hit = false;
                    loop {
                        let n = unsafe { libc::read(ifd, buf.as_mut_ptr() as *mut _, buf.len()) };
                        if n <= 0 {
                            break;
                        }
                        hit |= events_name_matches(&buf[..n as usize], &name);
                    }
                    if hit {
                        let _ = tx.send(Ext::FileChanged);
                        let _ = proxy.send_event(());
                    }
                }
                unsafe { libc::close(ifd) }; // also removes the watch
            })
            .ok();
        match thread {
            Some(t) => Some(Watch { efd, thread: Some(t) }),
            None => {
                unsafe { libc::close(efd) };
                None
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(t) = self.thread.take() {
            let one: u64 = 1;
            unsafe { libc::write(self.efd, &one as *const u64 as *const _, 8) };
            let _ = t.join();
            unsafe { libc::close(self.efd) };
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether any `struct inotify_event` in `buf` is for `name`.
fn events_name_matches(buf: &[u8], name: &OsString) -> bool {
    const HDR: usize = std::mem::size_of::<libc::inotify_event>();
    let (mut i, mut hit) = (0, false);
    while i + HDR <= buf.len() {
        let len = u32::from_ne_bytes(buf[i + 12..i + 16].try_into().unwrap()) as usize;
        let end = (i + HDR + len).min(buf.len());
        let raw = &buf[i + HDR..end];
        let n = raw.iter().position(|&b| b == 0).map_or(raw, |z| &raw[..z]);
        hit |= n == name.as_bytes();
        i += HDR + len;
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn ev(name: &str) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        let padded = (name.len() + 1).div_ceil(16) * 16;
        v.extend_from_slice(&(padded as u32).to_ne_bytes());
        v.extend_from_slice(name.as_bytes());
        v.resize(16 + padded, 0);
        v
    }

    #[test]
    fn matches_only_the_watched_name() {
        let n = OsString::from_vec(b"main.pdf".to_vec());
        let mut b = ev("other.aux");
        assert!(!events_name_matches(&b, &n));
        b.extend(ev("main.pdf"));
        assert!(events_name_matches(&b, &n));
        assert!(!events_name_matches(&ev("main.pdf.tmp"), &n));
    }
}
