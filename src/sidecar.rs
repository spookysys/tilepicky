//! `tilepick.json`: one file per directory that describes the sheets in it.
//! Each entry holds tags, the origin of cells, and animations. The source
//! directory and the directory of your own tilemaps use the same format.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Every sheet is on a 32 px grid.
pub const TILE: u32 = 32;
pub const BOOK: &str = "tilepick.json";

/// One cell: its origin and the words that describe it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// Path of the original sheet, relative to the source root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Cell position `[x, y]` in the original sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A strip of frames, left to right. Each frame is `w` x `h` cells; the strip
/// starts at cell `(x, y)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Animation {
    pub x: u32,
    #[serde(alias = "row")]
    pub y: u32,
    #[serde(default = "one")]
    pub w: u32,
    #[serde(default = "one")]
    pub h: u32,
    pub frames: u32,
    pub ms: u32,
}

fn one() -> u32 {
    1
}

impl Animation {
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.x + self.w * self.frames && y >= self.y && y < self.y + self.h
    }
    /// Cells of the strip, as a rectangle: x0, y0, x1, y1 inclusive.
    pub fn area(&self) -> (u32, u32, u32, u32) {
        (self.x, self.y, self.x + self.w * self.frames - 1, self.y + self.h - 1)
    }
}

/// What the book says about one sheet.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Sidecar {
    /// Words that describe the whole sheet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Keyed by `"x,y"` so that the file stays readable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Cell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<Animation>,
}

impl Sidecar {
    pub fn key(x: u32, y: u32) -> String {
        format!("{x},{y}")
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.cells.is_empty() && self.animations.is_empty()
    }

    pub fn get(&self, x: u32, y: u32) -> Option<&Cell> {
        self.cells.get(&Self::key(x, y))
    }

    pub fn set(&mut self, x: u32, y: u32, cell: Option<Cell>) {
        match cell {
            Some(c) => {
                self.cells.insert(Self::key(x, y), c);
            }
            None => {
                self.cells.remove(&Self::key(x, y));
            }
        }
    }

    pub fn animation_at(&self, x: u32, y: u32) -> Option<&Animation> {
        self.animations.iter().find(|a| a.contains(x, y))
    }

    pub fn animation_at_mut(&mut self, x: u32, y: u32) -> Option<&mut Animation> {
        self.animations.iter_mut().find(|a| a.contains(x, y))
    }
}

pub type Book = BTreeMap<String, Sidecar>;

pub fn load_book(dir: &Path) -> Book {
    std::fs::read_to_string(dir.join(BOOK))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
