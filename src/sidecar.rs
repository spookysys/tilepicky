// SPDX-License-Identifier: GPL-3.0-only
//! `tilepicky.json`: one file per directory that describes the sheets in it.
//! Each entry holds the grid, cell origins, animations, and the AI label.
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

/// "1 animation", "3 animations"; nothing for none.
pub fn count_text(n: usize, noun: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(format!("1 {noun}")),
        n => Some(format!("{n} {noun}s")),
    }
}

/// Whether the model could say what the sheet shows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status { Labeled, Unlabelable }

/// What a model said about a whole sheet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Label {
    pub provider: String,
    pub model: String,
    pub status: Status,
    pub caption: String,
    pub tags: Vec<String>,
    /// The tags the request asked the model to look for; see `Book::tag_list`.
    /// Absent on a label made before the tool asked for any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_list: Option<Vec<String>>,
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
    /// True when the grid was read off the pixels instead of chosen by a
    /// person. It is written so the reading happens once and not on every
    /// open, and it is cleared the moment someone sets the grid by hand.
    /// Anything that wants a grid a person stands behind must skip these.
    #[serde(default, skip_serializing_if = "not")]
    pub read: bool,
    /// Where the regions of this sheet came from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<Provenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<Animation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

impl Sidecar {
    pub fn is_empty(&self) -> bool {
        self.tile.is_none() && self.gap.is_none() && self.offset.is_none() && self.provenance.is_empty() && self.animations.is_empty()
            && self.label.is_none()
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
    /// The tags each labeling request asks the model to look for. Absent:
    /// `TAG_LIST`. It lives in the book of the library root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_list: Option<Vec<String>>,
    /// One library job and its durable commands. Credentials stay in the app configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_batch: Option<serde_json::Value>,
    #[serde(default)]
    pub sheets: BTreeMap<String, Sidecar>,
}

/// The tags a library looks for until someone changes its list.
pub const TAG_LIST: [&str; 11] = ["character", "NPC", "hero", "landscape", "building", "indoor", "UI", "font", "animation", "props", "background"];

/// The list of a book: its own, else `TAG_LIST`.
pub fn tag_list(book: &Book) -> Vec<String> {
    book.tag_list.clone().unwrap_or_else(|| TAG_LIST.map(String::from).to_vec())
}

/// Writes the tag list of a library. The default list is written as absent,
/// so that a library that never changed it follows a new default.
pub fn store_tag_list(dir: &Path, list: &[String]) -> Result<(), String> {
    update_book(dir, |book| {
        book.tag_list = (list != TAG_LIST).then(|| list.to_vec());
        Ok(())
    })
}

/// A missing book is empty. Unreadable books must not be overwritten.
/// An entry that holds only keys this version does not know is dropped.
pub fn load_book(dir: &Path) -> Result<Book, String> {
    let mut book: Book = crate::storage::read(&dir.join(BOOK))?;
    book.sheets.retain(|_, side| !side.is_empty());
    #[cfg(windows)]
    let book = normalize_windows_paths(book)?;
    Ok(book)
}

#[cfg(any(windows, test))]
fn normalize_windows_paths(mut book: Book) -> Result<Book, String> {
    let old = std::mem::take(&mut book.sheets);
    for (path, mut side) in old {
        let path = path.replace('\\', "/");
        for p in &mut side.provenance { p.source = p.source.replace('\\', "/"); }
        if book.sheets.insert(path.clone(), side).is_some() { return Err(format!("Duplicate book entry: {path}")); }
    }
    Ok(book)
}

/// Holds the lock across the read, change, and atomic replacement.
/// Readers see either complete version. Writers cannot overwrite another writer's changes.
pub fn update_book<T>(dir: &Path, change: impl FnOnce(&mut Book) -> Result<T, String>) -> Result<T, String> {
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(dir.join(".tilepicky-book.lock")).map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < std::time::Duration::from_secs(2) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(format!("Cannot lock the library book: {error}")),
        }
    }
    let mut book = load_book(dir)?;
    let before = book.clone();
    let result = change(&mut book)?;
    if book != before { crate::storage::write(&dir.join(BOOK), &book)?; }
    Ok(result)
}

/// Remembers the tile size of a directory, for the sheets that name none.
pub fn store_tile(dir: &Path, tile: [u32; 2]) -> Result<(), String> {
    update_book(dir, |book| { book.tile = Some(Pair::of(tile)); Ok(()) })
}

/// Moves or copies one entry to a new path, for a renamed or duplicated
/// sheet.
pub fn move_entry(dir: &Path, old: &str, new: &str, keep_old: bool) -> Result<(), String> {
    update_book(dir, |book| {
        if let Some(e) = book.sheets.get(old).cloned() {
            if !keep_old {
                book.sheets.remove(old);
            }
            book.sheets.insert(new.to_string(), e);
        }
        Ok(())
    })
}

/// Re-keys every entry under a renamed folder.
pub fn move_prefix(dir: &Path, old: &str, new: &str) -> Result<(), String> {
    update_book(dir, |book| {
        book.sheets = std::mem::take(&mut book.sheets)
            .into_iter()
            .map(|(k, v)| match k.strip_prefix(&format!("{old}/")) {
                Some(rest) => (format!("{new}/{rest}"), v),
                None => (k, v),
            })
            .collect();
        Ok(())
    })
}

fn not(b: &bool) -> bool {
    !*b
}

/// Writes one entry. The book is read again first, so that entries changed
/// by hand in the meantime survive. An empty entry is removed.
pub fn store_entry(dir: &Path, rel: &str, side: &Sidecar) -> Result<(), String> {
    update_book(dir, |book| {
        if side.is_empty() {
            book.sheets.remove(rel);
        } else {
            book.sheets.insert(rel.to_string(), side.clone());
        }
        Ok(())
    })
}

/// Writes or removes the labels of some sheets, in one write of the book.
/// The rest of each entry stays.
pub fn store_labels<'a>(dir: &Path, labels: impl IntoIterator<Item = (&'a str, Option<Label>)>) -> Result<(), String> {
    update_book(dir, |book| {
        for (rel, label) in labels {
            let side = book.sheets.entry(rel.into()).or_default();
            side.label = label;
            if side.is_empty() { book.sheets.remove(rel); }
        }
        Ok(())
    })
}

/// Clears all labels in the book, including entries for missing files.
pub fn clear_labels(dir: &Path) -> Result<Vec<String>, String> {
    update_book(dir, |book| {
        let mut cleared = Vec::new();
        for (rel, side) in &mut book.sheets {
            if side.label.take().is_some() { cleared.push(rel.clone()); }
        }
        book.sheets.retain(|_, side| !side.is_empty());
        Ok(cleared)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_windows_paths_keep_entries_and_sources() {
        let side = Sidecar { provenance: vec![Provenance { source: r"pack\tree.png".into(), rects: vec![[0, 0, 8, 8]] }],
            ..Sidecar::default() };
        let mut book = Book::default();
        book.sheets.insert(r"folder\sheet.png".into(), side);
        let normalized = normalize_windows_paths(book).unwrap();
        assert_eq!(normalized.sheets["folder/sheet.png"].provenance[0].source, "pack/tree.png");
        let mut duplicate = normalized;
        duplicate.sheets.insert(r"folder\sheet.png".into(), Sidecar::default());
        assert!(normalize_windows_paths(duplicate).is_err());
    }

    use super::*;

    #[test]
    fn labels_roundtrip_rename_and_remove_keep_the_grid() {
        let folder = crate::storage::tests::Folder::new();
        let dir = &folder.0;
        let label = Label { provider: "test".into(), model: "instant".into(), status: Status::Labeled,
            caption: "Tree".into(), tags: vec!["forest".into()], tag_list: Some(vec!["tree".into()]) };
        let grid = Sidecar { tile: Some(Pair::Two([10, 20])), ..Sidecar::default() };
        store_entry(dir, "folder/sheet.png", &grid).unwrap();
        store_labels(dir, [("folder/sheet.png", Some(label.clone()))]).unwrap();
        let loaded = load_book(dir).unwrap().sheets.remove("folder/sheet.png").unwrap();
        assert_eq!(loaded.label, Some(label.clone()));
        assert_eq!(loaded.tile, grid.tile);
        move_entry(dir, "folder/sheet.png", "folder/renamed.png", false).unwrap();
        move_prefix(dir, "folder", "assets").unwrap();
        let book = load_book(dir).unwrap();
        assert_eq!(book.sheets.len(), 1);
        assert_eq!(book.sheets["assets/renamed.png"], loaded);
        store_labels(dir, [("assets/renamed.png", None), ("a.png", Some(label.clone())), ("b.png", Some(label))]).unwrap();
        let book = load_book(dir).unwrap();
        assert_eq!(book.sheets["assets/renamed.png"], grid);
        assert_eq!(book.sheets.len(), 3);
        store_labels(dir, [("a.png", None)]).unwrap();
        assert!(!load_book(dir).unwrap().sheets.contains_key("a.png"));
    }

    #[test]
    fn clear_all_keeps_metadata_and_refuses_damaged_books() {
        let folder = crate::storage::tests::Folder::new();
        let label = Label { provider: "test".into(), model: "test".into(), status: Status::Labeled,
            caption: "Tree".into(), tags: vec!["tree".into()], tag_list: None };
        let side = Sidecar { tile: Some(Pair::One(16)), gap: Some(Pair::One(1)),
            provenance: vec![Provenance { source: "source.png".into(), rects: vec![[0, 0, 16, 16]] }], ..Sidecar::default() };
        store_entry(&folder.0, "nested/sheet.png", &side).unwrap();
        store_tag_list(&folder.0, &["tree".into()]).unwrap();
        store_labels(&folder.0, [("nested/sheet.png", Some(label.clone())), ("missing.png", Some(label))]).unwrap();
        let cleared = clear_labels(&folder.0).unwrap();
        assert_eq!(cleared, ["missing.png", "nested/sheet.png"]);
        let book = load_book(&folder.0).unwrap();
        assert_eq!(book.sheets.len(), 1);
        assert_eq!(book.sheets["nested/sheet.png"], side);
        assert_eq!(book.tag_list, Some(vec!["tree".into()]));
        std::fs::write(folder.0.join(BOOK), "broken").unwrap();
        assert!(clear_labels(&folder.0).is_err());
        assert_eq!(std::fs::read_to_string(folder.0.join(BOOK)).unwrap(), "broken");
    }

    /// Books from before whole-sheet labels hold island regions under `labels`.
    /// They read without an error, and the next write leaves them out.
    #[test]
    fn old_island_labels_are_dropped() {
        let folder = crate::storage::tests::Folder::new();
        std::fs::write(folder.0.join(BOOK), r#"{"sheets": {
            "a.png": {"tile": 16, "labels": {"identity": "x", "islands": [{"rects": [[0, 0, 16, 16]], "label": null}]}},
            "b.png": {"labels": {"identity": "x", "islands": []}}
        }}"#).unwrap();
        let book = load_book(&folder.0).unwrap();
        assert_eq!(book.sheets.keys().collect::<Vec<_>>(), ["a.png"]);
        crate::storage::write(&folder.0.join(BOOK), &book).unwrap();
        assert!(!std::fs::read_to_string(folder.0.join(BOOK)).unwrap().contains("islands"));
    }

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

    /// A library starts with the default tag list, and keeps one of its own
    /// at the root of its book. The default list is written as absent.
    #[test]
    fn the_tag_list_lives_at_the_root_of_the_book() {
        let folder = crate::storage::tests::Folder::new();
        let dir = &folder.0;
        assert_eq!(tag_list(&load_book(dir).unwrap()), TAG_LIST);
        let own = vec!["character".to_string(), "boss".into()];
        store_tag_list(dir, &own).unwrap();
        store_entry(dir, "a.png", &Sidecar { tile: Some(Pair::One(8)), ..Sidecar::default() }).unwrap();
        let text = std::fs::read_to_string(dir.join(BOOK)).unwrap();
        assert!(text.contains(r#""tag_list": ["#) || text.contains(r#""tag_list":["#), "{text}");
        assert_eq!(tag_list(&load_book(dir).unwrap()), own);
        store_tag_list(dir, &[]).unwrap();
        assert!(tag_list(&load_book(dir).unwrap()).is_empty(), "an empty list stays empty");
        store_tag_list(dir, &TAG_LIST.map(String::from)).unwrap();
        assert_eq!(load_book(dir).unwrap().tag_list, None);
    }
}
