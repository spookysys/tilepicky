//! `tilepick.json`: one file per directory that describes the sheets in it.
//! Each entry holds the sheet's grid, the origin of cells, and animations.
//! The source directory and your tilemap directory use the same format.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const BOOK: &str = "tilepick.json";

/// The regions of this sheet that came from one source file, as pixel
/// rectangles `[x, y, w, h]`. Where in the source they came from is not
/// recorded: the source file itself answers that.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provenance {
    pub source: String,
    pub rects: Vec<[u32; 4]>,
}

/// A strip of frames, left to right: a place on the bitmap, in pixels.
/// The grid plays no part in it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Animation {
    /// Top-left corner of the strip.
    pub px: [u32; 2],
    /// Size of one frame.
    pub frame: [u32; 2],
    pub frames: u32,
    pub ms: u32,
}

impl Animation {
    /// The strip in pixels: x0, y0, and one past x1, y1.
    pub fn px_rect(&self) -> (u32, u32, u32, u32) {
        (self.px[0], self.px[1], self.px[0] + self.frame[0] * self.frames, self.px[1] + self.frame[1])
    }
    pub fn px_overlaps(&self, other: (u32, u32, u32, u32)) -> bool {
        let a = self.px_rect();
        a.0 < other.2 && other.0 < a.2 && a.1 < other.3 && other.1 < a.3
    }
    /// The strip moved by a pixel offset.
    pub fn shifted(&self, dx: i64, dy: i64) -> Animation {
        let px = [(self.px[0] as i64 + dx) as u32, (self.px[1] as i64 + dy) as u32];
        Animation { px, ..self.clone() }
    }
}

/// A tile size: one number for square tiles, `[w, h]` otherwise.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(untagged)]
pub enum TileSize {
    Square(u32),
    Wh([u32; 2]),
}

impl TileSize {
    pub fn wh(self) -> [u32; 2] {
        match self {
            TileSize::Square(n) => [n, n],
            TileSize::Wh(a) => a,
        }
    }
    pub fn of(wh: [u32; 2]) -> Self {
        if wh[0] == wh[1] { TileSize::Square(wh[0]) } else { TileSize::Wh(wh) }
    }
}

/// What the book says about one sheet.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Sidecar {
    /// The sheet's tile size. Absent: the run's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<TileSize>,
    /// Pixels between neighbouring tiles on this sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<u32>,
    /// Pixels before the first tile, on both axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// Where the regions of this sheet came from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<Provenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<Animation>,
}

impl Sidecar {
    pub fn is_empty(&self) -> bool {
        self.tile.is_none() && self.gap.is_none() && self.offset.is_none() && self.provenance.is_empty() && self.animations.is_empty()
    }



}

pub type Book = BTreeMap<String, Sidecar>;

pub fn load_book(dir: &Path) -> Book {
    std::fs::read_to_string(dir.join(BOOK))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Moves or copies one entry to a new path, for a renamed or duplicated
/// sheet.
pub fn move_entry(dir: &Path, old: &str, new: &str, keep_old: bool) -> Result<(), String> {
    let mut book = load_book(dir);
    if let Some(e) = book.get(old).cloned() {
        if !keep_old {
            book.remove(old);
        }
        book.insert(new.to_string(), e);
        let json = serde_json::to_string_pretty(&book).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(BOOK), json).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Re-keys every entry under a renamed folder.
pub fn move_prefix(dir: &Path, old: &str, new: &str) -> Result<(), String> {
    let book = load_book(dir);
    let moved: Book = book
        .into_iter()
        .map(|(k, v)| match k.strip_prefix(&format!("{old}/")) {
            Some(rest) => (format!("{new}/{rest}"), v),
            None => (k, v),
        })
        .collect();
    let json = serde_json::to_string_pretty(&moved).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(BOOK), json).map_err(|e| e.to_string())
}

/// Writes one entry. The book is read again first, so that entries changed
/// by hand in the meantime survive. An empty entry is removed.
pub fn store_entry(dir: &Path, rel: &str, side: &Sidecar) -> Result<(), String> {
    let mut book = load_book(dir);
    if side.is_empty() {
        book.remove(rel);
    } else {
        book.insert(rel.to_string(), side.clone());
    }
    let json = serde_json::to_string_pretty(&book).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(BOOK), json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strip_is_a_pixel_rectangle() {
        let a = Animation { px: [128, 64], frame: [64, 96], frames: 6, ms: 100 };
        assert_eq!(a.px_rect(), (128, 64, 128 + 6 * 64, 64 + 96));
        assert!(a.px_overlaps((0, 0, 129, 65)));
        assert!(!a.px_overlaps((0, 0, 128, 64)));
        assert_eq!(a.shifted(-128, 32).px_rect(), (0, 96, 6 * 64, 96 + 96));
    }
}
