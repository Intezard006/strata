//! Links, hint labels, jumplist and external opening.

use std::process::{Command, Stdio};

use crate::search::Rect4;

#[derive(Clone, Debug)]
pub enum Target {
    /// Page and optional y (points from the page origin).
    Page(u32, Option<f32>),
    Uri(String),
}

#[derive(Clone, Debug)]
pub struct LinkInfo {
    /// Page points relative to the page origin.
    pub rect: Rect4,
    pub target: Target,
}

pub const HINT_ALPHABET: &[char] = &['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];

/// Fixed-length labels so no label is a prefix of another.
pub fn hint_labels(n: usize, alphabet: &[char]) -> Vec<String> {
    let k = alphabet.len();
    let mut len = 1;
    let mut cap = k;
    while cap < n {
        len += 1;
        cap *= k;
    }
    (0..n)
        .map(|mut i| {
            let mut s = vec![alphabet[0]; len];
            for slot in s.iter_mut().rev() {
                *slot = alphabet[i % k];
                i /= k;
            }
            s.into_iter().collect()
        })
        .collect()
}

/// Spawn `xdg-open` fully detached (the intermediate shell exits at once, so no zombie is left).
pub fn open_external(uri: &str) -> Result<(), String> {
    if uri.starts_with('-') || uri.is_empty() {
        return Err("refusing odd link".into());
    }
    let mut child = Command::new("sh")
        .args(["-c", "xdg-open \"$1\" >/dev/null 2>&1 &", "sh", uri])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let _ = child.wait();
    Ok(())
}

/// Scroll positions (points) visited before jumps.
#[derive(Default)]
pub struct Jumplist {
    list: Vec<f32>,
    pos: usize,
}

impl Jumplist {
    pub fn push(&mut self, cur: f32) {
        self.list.truncate(self.pos);
        self.list.push(cur);
        self.pos = self.list.len();
    }

    pub fn back(&mut self, cur: f32) -> Option<f32> {
        if self.pos == 0 {
            return None;
        }
        if self.pos == self.list.len() {
            self.list.push(cur);
        }
        self.pos -= 1;
        Some(self.list[self.pos])
    }

    pub fn forward(&mut self) -> Option<f32> {
        if self.pos + 1 < self.list.len() {
            self.pos += 1;
            Some(self.list[self.pos])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let l = hint_labels(5, HINT_ALPHABET);
        assert_eq!(l, ["a", "s", "d", "f", "g"]);
        let l = hint_labels(12, HINT_ALPHABET);
        assert!(l.iter().all(|s| s.len() == 2));
        let mut u = l.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), 12);
        assert_eq!(l[0], "aa");
        assert_eq!(l[1], "as");
        assert!(hint_labels(0, HINT_ALPHABET).is_empty());
        assert_eq!(hint_labels(82, HINT_ALPHABET)[81].len(), 3);
    }

    #[test]
    fn jumplist() {
        let mut j = Jumplist::default();
        assert_eq!(j.back(5.0), None);
        j.push(10.0);
        j.push(20.0);
        assert_eq!(j.back(30.0), Some(20.0));
        assert_eq!(j.back(0.0), Some(10.0));
        assert_eq!(j.back(0.0), None);
        assert_eq!(j.forward(), Some(20.0));
        assert_eq!(j.forward(), Some(30.0));
        assert_eq!(j.forward(), None);
    }
}
