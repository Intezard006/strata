//! Touchpad pinch gestures: winit has none on Wayland, so this binds `zwp_pointer_gestures_v1` on winit's own
//! display connection with a private event queue. A thread blocks in the queue (poll on the display fd), so
//! there is no polling; events go to the event loop through the usual channel and proxy.

use crate::ext::Ext;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::os::fd::AsRawFd;
use std::sync::mpsc::Sender;
use wayland_backend::sys::client::Backend;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::pointer_gestures::zv1::client::{zwp_pointer_gesture_pinch_v1, zwp_pointer_gestures_v1};
use winit::event_loop::EventLoopProxy;

struct State {
    tx: Sender<Ext>,
    proxy: EventLoopProxy<()>,
    /// The surface (wl_surface pointer) the running gesture began on.
    surface: usize,
}

impl State {
    fn send(&self, e: Ext) {
        let _ = self.tx.send(e);
        let _ = self.proxy.send_event(());
    }
}

/// Handle to the gesture thread. `stop` wakes it through an eventfd and joins it, so that it never touches the
/// wl_display after winit has disconnected it (that read produced "Io error: Cannot allocate memory" at exit).
pub struct Pinch {
    efd: i32,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Pinch {
    pub fn stop(&mut self) {
        if let Some(t) = self.thread.take() {
            let one: u64 = 1;
            unsafe { libc::write(self.efd, &one as *const u64 as *const _, 8) };
            let _ = t.join();
            unsafe { libc::close(self.efd) };
        }
    }
}

impl Drop for Pinch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the gesture thread on the display of `window`. Silent when the compositor has no gestures.
pub fn spawn(window: &impl HasDisplayHandle, tx: Sender<Ext>, proxy: EventLoopProxy<()>) -> Option<Pinch> {
    let h = window.display_handle().ok()?;
    let RawDisplayHandle::Wayland(w) = h.as_raw() else { return None };
    let display = w.display.as_ptr() as usize;
    let efd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if efd < 0 {
        return None;
    }
    let thread = std::thread::Builder::new().name("pinch".into()).stack_size(256 << 10).spawn(move || {
        // SAFETY: the pointer is winit's wl_display, alive for the whole process; the foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display as *mut _) };
        let conn = Connection::from_backend(backend);
        let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else { return };
        let qh = queue.handle();
        let (Ok(seat), Ok(gestures)) = (
            globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=7, ()),
            globals.bind::<zwp_pointer_gestures_v1::ZwpPointerGesturesV1, _, _>(&qh, 1..=3, ()),
        ) else {
            if std::env::var_os("STRATA_DEBUG").is_some() {
                eprintln!("strata: no pointer gestures protocol");
            }
            return;
        };
        let pointer = seat.get_pointer(&qh, ());
        let _pinch = gestures.get_pinch_gesture(&pointer, &qh, ());
        if std::env::var_os("STRATA_DEBUG").is_some() {
            eprintln!("strata: pinch gestures bound");
        }
        let mut st = State { tx, proxy, surface: 0 };
        loop {
            if queue.dispatch_pending(&mut st).is_err() {
                return;
            }
            if conn.flush().is_err() {
                return;
            }
            let Some(guard) = queue.prepare_read() else { continue };
            let mut fds = [
                libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: efd, events: libc::POLLIN, revents: 0 },
            ];
            let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return;
            }
            if fds[1].revents != 0 {
                return; // guard drops: read cancelled
            }
            if fds[0].revents != 0 {
                match guard.read() {
                    Err(wayland_backend::client::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return,
                    Ok(_) => {}
                }
            }
        }
    });
    match thread {
        Ok(t) => Some(Pinch { efd, thread: Some(t) }),
        Err(_) => {
            unsafe { libc::close(efd) };
            None
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(_: &mut Self, _: &wl_pointer::WlPointer, _: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwp_pointer_gestures_v1::ZwpPointerGesturesV1, ()> for State {
    fn event(_: &mut Self, _: &zwp_pointer_gestures_v1::ZwpPointerGesturesV1, _: zwp_pointer_gestures_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, ()> for State {
    fn event(st: &mut Self, _: &zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, ev: zwp_pointer_gesture_pinch_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_pointer_gesture_pinch_v1::Event;
        match ev {
            Event::Begin { surface, .. } => {
                st.surface = surface.id().as_ptr() as usize;
                st.send(Ext::PinchBegin(st.surface));
            }
            Event::Update { scale, .. } => st.send(Ext::PinchUpdate(scale)),
            Event::End { .. } => st.send(Ext::PinchEnd),
            _ => {}
        }
    }
}
