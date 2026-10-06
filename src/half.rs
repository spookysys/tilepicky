// SPDX-License-Identifier: GPL-3.0-only
//! One half of the window: a folder, its tree, and the sheet open from it.
//! The library and the project are the same thing; only the edits differ.

use crate::index::Index;
use crate::settings::SearchIn;
use crate::sidecar::Label;
use crate::sheet::Sheet;
use crate::tree::{Node, Row};
use eframe::egui;
use std::path::Path;

pub struct Half {
    /// The library half: its sheets are sources. See `Sheet::library`.
    pub library: bool,
    pub index: Index,
    pub tree: Node,
    /// Which entries the search leaves visible; `None` shows every one.
    pub visible: Option<Vec<bool>>,
    /// The current query's embedding, when semantic search is ready. It only
    /// applies while it was made with the library's own embedding model.
    pub embed_query: Option<Vec<f32>>,
    pub text_embed_query: Option<Vec<f32>>,
    pub sheet: Option<Sheet>,
    /// The entry of the open sheet. None for a sheet that has no file yet.
    pub sel: Option<usize>,
    /// The row the arrow keys stand on in the tree, folders included.
    pub at: Option<Row>,
    /// A row the arrow keys moved to, to bring into view next frame.
    pub scroll: Option<Row>,
    /// The rows the tree showed when it last drew, in reading order.
    pub rows: Vec<Row>,
    /// A folder the arrow keys opened or closed, applied on the next draw.
    pub open_dir: Option<(String, bool)>,
}

impl Half {
    pub fn new(index: Index, library: bool) -> Self {
        Self {
            library,
            tree: tree_of(&index),
            index,
            visible: None,
            embed_query: None,
            text_embed_query: None,
            sheet: None,
            sel: None,
            at: None,
            scroll: None,
            rows: Vec::new(),
            open_dir: None,
        }
    }

    /// Whether this half has a folder to work in.
    pub fn is_set(&self) -> bool {
        !self.index.root.as_os_str().is_empty()
    }

    /// Reads the folder again. The open sheet keeps its place in the tree.
    pub fn rescan(&mut self, query: &[String], search: SearchIn) {
        let cursor_rel = match &self.at {
            Some(Row::File(i)) => self.index.entries.get(*i).map(|entry| entry.rel.clone()),
            _ => None,
        };
        let index = Index::scan(&self.index.root, self.index.tile);
        if index.embed_model != self.index.embed_model || index.embed_provider != self.index.embed_provider { self.embed_query = None; }
        if index.text_embed_model != self.index.text_embed_model || index.text_embed_provider != self.index.text_embed_provider {
            self.text_embed_query = None;
        }
        self.index = index;
        self.tree = tree_of(&self.index);
        self.refresh_visible(query, search);
        self.sel = self.sheet.as_ref().and_then(|s| self.index.position(&s.rel));
        self.at = match &self.at {
            Some(Row::File(_)) => cursor_rel.and_then(|rel| self.index.position(&rel)).map(Row::File),
            Some(Row::Dir(dir)) if self.index.dirs.contains(dir) => Some(Row::Dir(dir.clone())),
            _ => None,
        };
        self.scroll = None;
        self.rows.clear();
    }

    /// Points the half at another folder. Nothing stays open.
    pub fn set_root(&mut self, root: &Path, query: &[String], search: SearchIn) {
        self.embed_query = None;
        self.text_embed_query = None;
        self.index.root = root.to_path_buf();
        self.at = None;
        self.open_dir = None;
        self.sheet = None;
        self.sel = None;
        self.rescan(query, search);
    }

    /// Filters the tree again after the entries or the query changed.
    pub fn refresh_visible(&mut self, query: &[String], search: SearchIn) {
        self.visible = self.index.visible(query, search, self.embed_query.as_deref(), self.text_embed_query.as_deref());
    }

    /// The tile size to assume for a sheet whose entry names none: the sheet
    /// now open, else the folder's default.
    pub fn inherited_tile(&self) -> [u32; 2] {
        self.sheet.as_ref().map_or(self.index.tile, |s| s.tile)
    }

    /// Opens entry `i`. The zoom carries over from the sheet it replaces,
    /// and the arrow keys stand on the file.
    pub fn open(&mut self, ctx: &egui::Context, i: usize) -> Result<(), String> {
        let e = &self.index.entries[i];
        let mut s = Sheet::open(ctx, &self.index.root, &e.rel, self.inherited_tile(), e.side.clone())?;
        s.library = self.library;
        if let Some(prev) = &self.sheet {
            s.zoom = prev.zoom;
        }
        self.sheet = Some(s);
        self.sel = Some(i);
        self.at = Some(Row::File(i));
        Ok(())
    }

    /// Copies the open sheet's book entry into the index, so that search
    /// sees an edit before it is saved.
    pub fn sync_entry(&mut self) {
        if let (Some(i), Some(sheet)) = (self.sel, &self.sheet) {
            let entry = &mut self.index.entries[i];
            if entry.side.label != sheet.side.label {
                entry.text_embed = None;
                if self.index.embed_labels { entry.embed = None; }
            }
            if sheet.dirty { entry.embed = None; }
            entry.side = sheet.side.clone();
        }
    }

    /// Puts labels that the book of `dir` now holds into the entries and
    /// the open sheet.
    pub fn apply_labels(&mut self, dir: &Path, labels: &[(String, Option<Label>)]) {
        if self.index.root == dir {
            for (rel, label) in labels {
                if let Some(i) = self.index.position(rel) {
                    let entry = &mut self.index.entries[i];
                    if entry.side.label != *label {
                        entry.text_embed = None;
                        if self.index.embed_labels { entry.embed = None; }
                    }
                    entry.side.label = label.clone();
                }
            }
        }
        if let Some(sheet) = self.sheet.as_mut().filter(|s| s.dir == dir)
            && let Some((_, label)) = labels.iter().find(|(rel, _)| *rel == sheet.rel)
        {
            sheet.side.label = label.clone();
        }
    }
}

fn tree_of(index: &Index) -> Node {
    Node::build(&index.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &index.dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rescan_tracks_the_cursor_by_path_and_forgets_deleted_rows() {
        let folder = crate::storage::tests::Folder::new();
        for rel in ["a.png", "b.png"] { image::RgbaImage::new(4, 4).save(folder.0.join(rel)).unwrap(); }
        let mut half = Half::new(Index::scan(&folder.0, [8, 8]), true);
        half.at = Some(Row::File(1));
        std::fs::remove_file(folder.0.join("a.png")).unwrap();
        half.rescan(&[], SearchIn::default());
        assert_eq!(half.at, Some(Row::File(0)));
        std::fs::remove_file(folder.0.join("b.png")).unwrap();
        half.rescan(&[], SearchIn::default());
        assert_eq!(half.at, None);
    }

    #[test]
    fn label_changes_invalidate_only_vectors_that_use_labels() {
        let folder = crate::storage::tests::Folder::new();
        image::RgbaImage::new(4, 4).save(folder.0.join("a.png")).unwrap();
        let mut half = Half::new(Index::scan(&folder.0, [8, 8]), true);
        let label = Label { provider: "p".into(), model: "m".into(), status: crate::sidecar::Status::Labeled,
            caption: "Tree".into(), tags: Vec::new(), tag_list: None };
        half.index.entries[0].embed = Some(vec![1.0]);
        half.index.entries[0].text_embed = Some(vec![1.0]);
        half.apply_labels(&folder.0, &[("a.png".into(), Some(label.clone()))]);
        assert!(half.index.entries[0].text_embed.is_none());
        assert!(half.index.entries[0].embed.is_some());
        half.index.entries[0].text_embed = Some(vec![1.0]);
        half.apply_labels(&folder.0, &[("a.png".into(), Some(label))]);
        assert!(half.index.entries[0].text_embed.is_some(), "unchanged labels keep their vectors");
        half.index.embed_labels = true;
        half.apply_labels(&folder.0, &[("a.png".into(), None)]);
        assert!(half.index.entries[0].text_embed.is_none());
        assert!(half.index.entries[0].embed.is_none());
    }
}
