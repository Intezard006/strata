mod analyze;
mod app;
mod cache;
mod config;
mod db;
mod ext;
mod heading;
mod hypr;
mod imagemap;
mod keys;
mod layout;
mod links;
mod pages;
mod palette;
mod pinch;
mod previews;
mod pstore;
mod recolor;
mod render;
mod search;
mod synctex;
mod text;
mod toc;
mod ui;
mod watch;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> ExitCode {
    // Fixed malloc thresholds: big pixmaps are mmapped and returned at once instead of
    // fragmenting the render thread's arena; few arenas keep freed memory reusable.
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    unsafe {
        mallopt(-8, 2); // M_ARENA_MAX
        mallopt(-3, 256 * 1024); // M_MMAP_THRESHOLD (also disables the dynamic raise)
        mallopt(-1, 512 * 1024); // M_TRIM_THRESHOLD
    }
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("--analyze") => return analyze::run(&argv[1..]),
        Some("--dump-lines") => return analyze::dump_lines(&argv[1..]),
        Some("--write-index") => return analyze::write_index(&argv[1..]),
        Some("--write-highlights") => return app::write_highlights(&argv[1..]),
        Some("--dump-previews") => return analyze::dump_previews(&argv[1..]),
        Some("--dump-recall") => return analyze::dump_recall(&argv[1..]),
        Some("--dump-images") => return render::dump_images(&argv[1..]),
        Some("--exec") => return ext::run_exec(&argv[1..]),
        _ => {}
    }
    let mut file = None;
    let mut sync = None;
    let mut float = false;
    let mut page = None;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        if a == "--float" {
            float = true;
        } else if a == "--page" {
            page = it.next().and_then(|v| v.to_string_lossy().parse::<u32>().ok());
        } else if a == "--synctex-forward" {
            let spec = it.next().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
            let mut p = spec.splitn(3, ':');
            match (p.next().and_then(|v| v.parse::<u32>().ok()), p.next().and_then(|v| v.parse::<u32>().ok()), p.next()) {
                (Some(l), Some(c), Some(f)) => sync = Some((f.to_string(), l, c)),
                _ => {
                    eprintln!("usage: strata --synctex-forward LINE:COL:TEXFILE PDF");
                    return ExitCode::from(2);
                }
            }
        } else if file.is_none() {
            file = Some(a);
        }
    }
    let Some(arg) = file else {
        eprintln!("usage: strata [--synctex-forward LINE:COL:TEXFILE] [--float [--page N]] <file>\n       strata --exec '<command>' [--pid N]");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(arg);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if let Some((f, l, c)) = &sync {
        if ext::synctex_to_running(&path, f, *l, *c) {
            return ExitCode::SUCCESS;
        }
    }
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
    let mut app = app::App::new(handle, sizes, path, event_loop.create_proxy(), app::StartOpts { synctex: sync, float, page });
    let r = event_loop.run_app(&mut app);
    app.shutdown();
    app.save_state();
    if std::env::var_os("STRATA_DEBUG").is_some() {
        eprintln!("strata: image classifier calls: {}", imagemap::CLASSIFY_CALLS.load(std::sync::atomic::Ordering::Relaxed));
    }
    if let Err(e) = r {
        eprintln!("strata: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
