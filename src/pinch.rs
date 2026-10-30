//! Touchpad pinch gestures: winit has none on Wayland, so this binds `zwp_pointer_gestures_v1` on winit's own
//! display connection with a private event queue. A thread blocks in the queue (poll on the display fd), so
//! there is no polling; events go to the event loop through the usual channel and proxy.

use crate::ext::Ext;
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
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

/// Start the gesture thread on the display of `window`. Silent when the compositor has no gestures.
pub fn spawn(window: &impl HasDisplayHandle, tx: Sender<Ext>, proxy: EventLoopProxy<()>) {
    let Ok(h) = window.display_handle() else { return };
    let RawDisplayHandle::Wayland(w) = h.as_raw() else { return };
    let display = w.display.as_ptr() as usize;
    let _ = std::thread::Builder::new().name("pinch".into()).stack_size(256 << 10).spawn(move || {
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
        while queue.blocking_dispatch(&mut st).is_ok() {}
    });
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
