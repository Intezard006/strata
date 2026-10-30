mod app;
mod config;
mod keys;
mod layout;
mod links;
mod recolor;
mod render;
mod search;
mod text;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: strata <file>");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(arg);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let event_loop = match EventLoop::<()>::with_user_event().build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("strata: cannot create event loop: {e}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let (handle, sizes) = match render::spawn(path.clone(), event_loop.create_proxy()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("strata: cannot open {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let mut app = app::App::new(handle, sizes, path);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("strata: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
