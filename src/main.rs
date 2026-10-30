mod analyze;
mod app;
mod cache;
mod config;
mod heading;
mod keys;
mod layout;
mod links;
mod recolor;
mod render;
mod search;
mod text;
mod ui;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("--analyze") => return analyze::run(&argv[1..]),
        Some("--dump-lines") => return analyze::dump_lines(&argv[1..]),
        Some("--write-index") => return analyze::write_index(&argv[1..]),
        _ => {}
    }
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
    let mut app = app::App::new(handle, sizes, path, event_loop.create_proxy());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("strata: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
