//! Hyprland placement of the float window, through `hyprctl` with Lua dispatchers (0.56).

use std::process::Command;

const MARGIN: i32 = 24;

fn dispatch(lua: &str) {
    let _ = Command::new("hyprctl").args(["dispatch", lua]).output();
}

/// Top-left of a `w` x `h` window in a monitor rectangle `(x, y, w, h)` for a position name.
pub fn origin(pos: &str, mon: (i32, i32, i32, i32), size: (i32, i32)) -> (i32, i32) {
    let (mx, my, mw, mh) = mon;
    let right = mx + mw - size.0 - MARGIN;
    let bottom = my + mh - size.1 - MARGIN;
    let (left, top) = (mx + MARGIN, my + MARGIN);
    let (x, y) = match pos {
        "top-left" => (left, top),
        "bottom-left" => (left, bottom),
        "bottom-right" => (right, bottom),
        "center" => (mx + (mw - size.0) / 2, my + (mh - size.1) / 2),
        _ => (right, top),
    };
    (x.max(mx), y.max(my))
}

/// The focused monitor in layout coordinates: `(x, y, w, h)` in logical pixels.
fn monitor() -> Option<(i32, i32, i32, i32)> {
    let out = Command::new("hyprctl").args(["monitors", "-j"]).output().ok()?;
    parse_monitor(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_monitor(json: &str) -> Option<(i32, i32, i32, i32)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let ms = v.as_array()?;
    let m = ms.iter().find(|m| m["focused"].as_bool() == Some(true)).or_else(|| ms.first())?;
    let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
    let n = |k: &str| m[k].as_f64().unwrap_or(0.0);
    Some((n("x") as i32, n("y") as i32, (n("width") / scale) as i32, (n("height") / scale) as i32))
}

/// Make the window of `class` floating, pinned (on every workspace), sized and placed. Runs once,
/// from a short-lived thread, after the window is mapped.
pub fn place_float(class: &str, w: u32, h: u32, pos: &str) {
    let sel = format!("window=\"class:{class}\"");
    let (w, h) = (w as i32, h as i32);
    let (x, y) = origin(pos, monitor().unwrap_or((0, 0, 1920, 1080)), (w, h));
    dispatch(&format!("hl.dsp.window.float({{action=\"enable\", {sel}}})"));
    dispatch(&format!("hl.dsp.window.pin({{action=\"enable\", {sel}}})"));
    dispatch(&format!("hl.dsp.window.resize({{x={w},y={h},exact=true,{sel}}})"));
    dispatch(&format!("hl.dsp.window.move({{x={x},y={y},{sel}}})"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions() {
        let mon = (0, 0, 1920, 1080);
        assert_eq!(origin("top-right", mon, (520, 700)), (1376, 24));
        assert_eq!(origin("bottom-left", mon, (520, 700)), (24, 356));
        assert_eq!(origin("center", mon, (520, 700)), (700, 190));
        assert_eq!(origin("top-right", (1920, 0, 1920, 1080), (520, 700)), (3296, 24));
        assert_eq!(origin("bottom-right", mon, (3000, 3000)), (0, 0), "oversized clamps to the monitor corner");
    }

    #[test]
    fn monitor_json() {
        let j = r#"[{"x":0,"y":0,"width":3840,"height":2160,"scale":2.0,"focused":false},{"x":1920,"y":0,"width":2560,"height":1440,"scale":1.0,"focused":true}]"#;
        assert_eq!(parse_monitor(j), Some((1920, 0, 2560, 1440)));
        assert_eq!(parse_monitor("[]"), None);
    }
}
