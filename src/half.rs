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
        self.index = Index::scan(&self.index.root, self.index.tile);
        self.tree = tree_of(&self.index);
        self.refresh_visible(query, search);
        self.sel = self.sheet.as_ref().and_then(|s| self.index.position(&s.rel));
    }

    /// Points the half at another folder. Nothing stays open.
    pub fn set_root(&mut self, root: &Path, query: &[String], search: SearchIn) {
        self.index.root = root.to_path_buf();
        self.sheet = None;
        self.sel = None;
        self.rescan(query, search);
    }

    /// Filters the tree again after the entries or the query changed.
    pub fn refresh_visible(&mut self, query: &[String], search: SearchIn) {
        self.visible = self.index.visible(query, search);
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
            self.index.entries[i].side = sheet.side.clone();
        }
    }

    /// Puts labels that the book of `dir` now holds into the entries and
    /// the open sheet.
    pub fn apply_labels(&mut self, dir: &Path, labels: &[(String, Option<Label>)]) {
        if self.index.root == dir {
            for (rel, label) in labels {
                if let Some(i) = self.index.position(rel) {
                    self.index.entries[i].side.label = label.clone();
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
