// SPDX-License-Identifier: GPL-3.0-only
//! `tilepicky.json`: one file per directory that describes the sheets in it.
//! Each entry holds the sheet's grid, the origin of cells, and animations.
//! The library and the project use the same format.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const BOOK: &str = "tilepicky.json";

/// The regions of this sheet that came from one source file, as pixel
/// rectangles `[x, y, w, h]`. Where in the source they came from is not
/// recorded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provenance {
    pub source: String,
    pub rects: Vec<[u32; 4]>,
}

/// A block of frames: a place on the bitmap, in pixels. The frames lie in a
/// grid and they play left to right, then the next row down. The tile grid
/// of the sheet plays no part in it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Animation {
    /// Top-left corner of the block.
    pub px: [u32; 2],
    /// Size of one frame.
    pub frame: [u32; 2],
    /// Frames in a row, and the number of rows. One number is one row.
    pub frames: Pair,
    pub ms: u32,
}

impl Animation {
    /// Frames in a row, and the number of rows.
    pub fn grid(&self) -> [u32; 2] {
        let [c, r] = self.frames.row();
        [c.max(1), r.max(1)]
    }
    /// How many frames the block holds.
    pub fn count(&self) -> u32 {
        let [c, r] = self.grid();
        c * r
    }
    /// The top-left corner of frame `i`, in pixels.
    pub fn frame_px(&self, i: u32) -> [u32; 2] {
        let [c, _] = self.grid();
        [self.px[0] + (i % c) * self.frame[0], self.px[1] + (i / c) * self.frame[1]]
    }
    /// The block in pixels: x0, y0, and one past x1, y1.
    pub fn px_rect(&self) -> (u32, u32, u32, u32) {
        let [c, r] = self.grid();
        (self.px[0], self.px[1], self.px[0] + self.frame[0] * c, self.px[1] + self.frame[1] * r)
    }
    pub fn px_overlaps(&self, other: (u32, u32, u32, u32)) -> bool {
        let a = self.px_rect();
        a.0 < other.2 && other.0 < a.2 && a.1 < other.3 && other.1 < a.3
    }
    /// The block moved by a pixel offset.
    pub fn shifted(&self, dx: i64, dy: i64) -> Animation {
        let px = [(self.px[0] as i64 + dx) as u32, (self.px[1] as i64 + dy) as u32];
        Animation { px, ..self.clone() }
    }
}

/// One number or two. What one number means belongs to the field that holds
/// it: a tile size, a gap, and an offset write one number when both axes
/// agree (`of` and `xy`); a frame count writes one number when there is a
/// single row (`strip` and `row`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(untagged)]
pub enum Pair<T = u32> {
    One(T),
    Two([T; 2]),
}

impl<T: Copy + PartialEq> Pair<T> {
    pub fn xy(self) -> [T; 2] {
        match self {
            Pair::One(n) => [n, n],
            Pair::Two(a) => a,
        }
    }
    pub fn of(xy: [T; 2]) -> Self {
        if xy[0] == xy[1] { Pair::One(xy[0]) } else { Pair::Two(xy) }
    }
}

impl Pair {
    pub fn row(self) -> [u32; 2] {
        match self {
            Pair::One(n) => [n, 1],
            Pair::Two(a) => a,
        }
    }
    pub fn strip(xy: [u32; 2]) -> Self {
        if xy[1] == 1 { Pair::One(xy[0]) } else { Pair::Two(xy) }
    }
}

/// What the book says about one sheet.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Sidecar {
    /// The sheet's tile size. Absent: the run's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<Pair>,
    /// Pixels between neighbouring tiles: one number for both axes, `[x, y]`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<Pair>,
    /// Pixels before the first tile: one number for both axes, `[x, y]`
    /// otherwise. Negative when the first tile starts before the image edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Pair<i32>>,
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

/// The book of one directory: the tile size the directory used last, and
/// one entry per sheet in it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Book {
    /// The tile size a sheet here starts with when it has no entry of its
    /// own. It follows the directory, so a project keeps its own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<Pair>,
    #[serde(default)]
    pub sheets: BTreeMap<String, Sidecar>,
}

/// Reads the book. A missing or unreadable file is an empty book.
pub fn load_book(dir: &Path) -> Book {
    std::fs::read_to_string(dir.join(BOOK))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_book(dir: &Path, book: &Book) -> Result<(), String> {
    let json = serde_json::to_string_pretty(book).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(BOOK), json).map_err(|e| e.to_string())
}

/// Remembers the tile size of a directory, for the sheets that name none.
pub fn store_tile(dir: &Path, tile: [u32; 2]) -> Result<(), String> {
    let mut book = load_book(dir);
    let want = Some(Pair::of(tile));
    if book.tile == want {
        return Ok(());
    }
    book.tile = want;
    write_book(dir, &book)
}

/// Moves or copies one entry to a new path, for a renamed or duplicated
/// sheet.
pub fn move_entry(dir: &Path, old: &str, new: &str, keep_old: bool) -> Result<(), String> {
    let mut book = load_book(dir);
    if let Some(e) = book.sheets.get(old).cloned() {
        if !keep_old {
            book.sheets.remove(old);
        }
        book.sheets.insert(new.to_string(), e);
        write_book(dir, &book)?;
    }
    Ok(())
}

/// Re-keys every entry under a renamed folder.
pub fn move_prefix(dir: &Path, old: &str, new: &str) -> Result<(), String> {
    let mut book = load_book(dir);
    book.sheets = book
        .sheets
        .into_iter()
        .map(|(k, v)| match k.strip_prefix(&format!("{old}/")) {
            Some(rest) => (format!("{new}/{rest}"), v),
            None => (k, v),
        })
        .collect();
    write_book(dir, &book)
}

/// Writes one entry. The book is read again first, so that entries changed
/// by hand in the meantime survive. An empty entry is removed.
pub fn store_entry(dir: &Path, rel: &str, side: &Sidecar) -> Result<(), String> {
    let mut book = load_book(dir);
    if side.is_empty() {
        book.sheets.remove(rel);
    } else {
        book.sheets.insert(rel.to_string(), side.clone());
    }
    write_book(dir, &book)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strip_is_a_pixel_rectangle() {
        let a = Animation {
            px: [128, 64],
            frame: [64, 96],
            frames: Pair::One(6),
            ms: 100,
        };
        assert_eq!(a.px_rect(), (128, 64, 128 + 6 * 64, 64 + 96));
        assert!(a.px_overlaps((0, 0, 129, 65)));
        assert!(!a.px_overlaps((0, 0, 128, 64)));
        assert_eq!(a.shifted(-128, 32).px_rect(), (0, 96, 6 * 64, 96 + 96));
    }

    #[test]
    fn a_block_of_frames_reads_row_by_row() {
        let a = Animation {
            px: [10, 20],
            frame: [8, 8],
            frames: Pair::Two([4, 3]),
            ms: 100,
        };
        assert_eq!(a.count(), 12);
        assert_eq!(a.px_rect(), (10, 20, 10 + 32, 20 + 24));
        assert_eq!(a.frame_px(0), [10, 20]);
        assert_eq!(a.frame_px(3), [10 + 24, 20]);
        assert_eq!(a.frame_px(4), [10, 28]);
        assert_eq!(a.frame_px(11), [10 + 24, 36]);
    }

    #[test]
    fn one_number_means_one_row_for_frames() {
        assert_eq!(Pair::strip([6, 1]), Pair::One(6));
        assert_eq!(Pair::One(6).row(), [6, 1]);
        assert_eq!(Pair::strip([4, 2]), Pair::Two([4, 2]));
        // The same number reads differently as a tile size.
        assert_eq!(Pair::One(6).xy(), [6, 6]);
    }

    #[test]
    fn a_gap_or_an_offset_is_one_number_or_two() {
        let one: Sidecar = serde_json::from_str(r#"{"gap": 1}"#).unwrap();
        assert_eq!(one.gap.map(Pair::xy), Some([1, 1]));
        let two: Sidecar = serde_json::from_str(r#"{"gap": [1, 2]}"#).unwrap();
        assert_eq!(two.gap.map(Pair::xy), Some([1, 2]));
        assert_eq!(serde_json::to_string(&two).unwrap(), r#"{"gap":[1,2]}"#);
        let neg: Sidecar = serde_json::from_str(r#"{"offset": [-3, 0]}"#).unwrap();
        assert_eq!(neg.offset.map(Pair::xy), Some([-3, 0]));
        assert_eq!(serde_json::to_string(&neg).unwrap(), r#"{"offset":[-3,0]}"#);
    }
}
