//! SyncTeX through the `synctex` command line tool: parse its output, expand the editor command.

use std::path::Path;
use std::process::{Command, Stdio};

/// A rectangle on a page, in points from the page's top-left.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct View {
    pub page: u32,
    pub rect: [f32; 4],
}

#[derive(Debug, PartialEq)]
pub struct Edit {
    pub input: String,
    pub line: u32,
    pub column: u32,
}

fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    block.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':')).map(str::trim)
}

/// The first result of `synctex view`. `v` is the baseline, so the box spans `v - H .. v`.
pub fn parse_view(out: &str) -> Option<View> {
    let start = out.find("SyncTeX result begin")?;
    let block = &out[start..];
    let num = |n: &str| field(block, n)?.parse::<f32>().ok();
    let (h, v, w, hh) = (num("h")?, num("v")?, num("W")?, num("H")?);
    let page = field(block, "Page")?.parse::<u32>().ok()?.checked_sub(1)?;
    Some(View { page, rect: [h, v - hh, h + w.max(2.0), v + 1.0] })
}

pub fn parse_edit(out: &str) -> Option<Edit> {
    let start = out.find("SyncTeX result begin")?;
    let block = &out[start..];
    let input = field(block, "Input")?.to_string();
    let line = field(block, "Line")?.parse::<i64>().ok()?.max(1) as u32;
    let column = field(block, "Column")?.parse::<i64>().ok().map_or(0, |c| c.max(0) as u32);
    Some(Edit { input, line, column })
}

pub fn view(pdf: &Path, input: &str, line: u32, col: u32) -> Result<View, String> {
    let out = Command::new("synctex")
        .args(["view", "-i", &format!("{line}:{col}:{input}"), "-o"])
        .arg(pdf)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("synctex: {e}"))?;
    parse_view(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "synctex: no match (is there a .synctex.gz next to the PDF?)".into())
}

pub fn edit(pdf: &Path, page: u32, x: f32, y: f32) -> Result<Edit, String> {
    let out = Command::new("synctex")
        .args(["edit", "-o", &format!("{}:{x:.2}:{y:.2}:", page + 1)])
        .arg(pdf)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("synctex: {e}"))?;
    parse_edit(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "synctex: no source location here".into())
}

/// Split `template` into words (quotes allowed) and substitute the placeholders inside each word,
/// so a path with spaces stays one argument and nothing is passed through a shell.
pub fn expand(template: &str, input: &str, line: u32, column: u32) -> Result<Vec<String>, String> {
    let words = crate::config::tokenize(template)?;
    Ok(words.into_iter().map(|w| w.replace("%{input}", input).replace("%{line}", &line.to_string()).replace("%{column}", &column.to_string())).collect())
}

/// Run the argument vector detached (own session, no stdio). `setsid -f` forks and returns at once,
/// so the child is reparented to init and never becomes our zombie.
pub fn spawn_detached(argv: &[String]) -> Result<(), String> {
    let (prog, args) = argv.split_first().ok_or("empty command")?;
    Command::new("setsid")
        .arg("-f")
        .arg(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("{prog}: {e}"))
        .and_then(|s| if s.success() { Ok(()) } else { Err(format!("{prog}: failed to start")) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = "This is SyncTeX command line utility, version 1.5\nSyncTeX result begin\nOutput:/tmp/a.pdf\nPage:3\nx:133.768356\ny:134.765\nh:133.768356\nv:137.763\nW:343.711\nH:9.962\nbefore:\noffset:0\nmiddle:\nafter:\nSyncTeX result end\n";

    #[test]
    fn view_output() {
        let v = parse_view(VIEW).unwrap();
        assert_eq!(v.page, 2, "pages are 1-based in synctex");
        assert!((v.rect[0] - 133.768).abs() < 1e-2 && (v.rect[1] - 127.801).abs() < 1e-2);
        assert!((v.rect[2] - 477.479).abs() < 1e-2);
        assert_eq!(parse_view("SyncTeX result begin\nSyncTeX result end"), None);
        assert_eq!(parse_view("garbage"), None);
    }

    #[test]
    fn edit_output() {
        let out = "SyncTeX result begin\nOutput:/tmp/a.pdf\nInput:/home/me/my paper/main.tex\nLine:42\nColumn:-1\nOffset:0\nContext:\nSyncTeX result end\n";
        assert_eq!(parse_edit(out), Some(Edit { input: "/home/me/my paper/main.tex".into(), line: 42, column: 0 }));
        assert_eq!(parse_edit("SyncTeX result begin\nSyncTeX result end"), None);
    }

    #[test]
    fn placeholders() {
        let a = expand("nvim --server /tmp/s --remote-expr \"f(%{line}, '%{input}', %{column})\"", "/a b/c.tex", 7, 3).unwrap();
        assert_eq!(a, ["nvim", "--server", "/tmp/s", "--remote-expr", "f(7, '/a b/c.tex', 3)"]);
        assert_eq!(expand("code -g %{input}:%{line}", "/x y.tex", 1, 0).unwrap(), ["code", "-g", "/x y.tex:1"]);
        assert!(expand("   ", "", 0, 0).unwrap().is_empty());
    }
}
