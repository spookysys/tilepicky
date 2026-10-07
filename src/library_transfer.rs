// SPDX-License-Identifier: GPL-3.0-only
//! Whole-sheet transfers keep file copying separate from canvas placement.

use crate::{App, Panel, Sheet, Sel, Rect, Id, Vec2, Color32, egui, workspace::Workspace};
use std::path::PathBuf;
use std::sync::mpsc;

pub(super) struct LibraryDrag {
    root: PathBuf,
    rel: String,
    tile: [u32; 2],
}

pub(super) struct FileCopy {
    root: PathBuf,
    result: mpsc::Receiver<Result<String, String>>,
}

impl App {
    pub(super) fn start_library_drag(&mut self, i: usize) {
        let Some(entry) = self.library.index.entries.get(i) else { return };
        self.library_drag = Some(LibraryDrag {
            root: self.library.index.root.clone(), rel: entry.rel.clone(), tile: self.library.index.tile,
        });
        self.library_sweep = None;
        self.workspace = Workspace::Build;
        self.drag = None;
    }

    pub(super) fn copy_library_sheet(&mut self, ctx: &egui::Context, i: usize) {
        let Some(entry) = self.library.index.entries.get(i) else { return };
        let source = LibraryDrag { root: self.library.index.root.clone(), rel: entry.rel.clone(), tile: self.library.index.tile };
        self.begin_sheet_copy(ctx, source, "");
    }

    fn begin_sheet_copy(&mut self, ctx: &egui::Context, source: LibraryDrag, folder: &str) {
        if !self.project.is_set() { self.status = "Open a project folder before copying a sheet.".into(); return; }
        if self.sheet_copy.is_some() { self.status = "Wait for the current sheet copy to finish.".into(); return; }
        let root = self.project.index.root.clone();
        let destination = root.clone();
        let folder = folder.to_string();
        let (sender, result) = mpsc::channel();
        let wake = ctx.clone();
        match std::thread::Builder::new().name("copy library sheet".into()).spawn(move || {
            let copied = crate::files::import_sheet(&source.root, &destination, &source.rel, &folder, source.tile);
            let _ = sender.send(copied);
            wake.request_repaint();
        }) {
            Ok(_) => { self.sheet_copy = Some(FileCopy { root, result }); self.status = "Copying sheet to the project...".into(); }
            Err(error) => self.status = format!("Cannot start the copy: {error}"),
        }
    }

    pub(super) fn poll_sheet_copy(&mut self, ctx: &egui::Context) {
        let Some(copy) = self.sheet_copy.take() else { return };
        match copy.result.try_recv() {
            Ok(Ok(rel)) => {
                if self.project.index.root == copy.root {
                    self.rescan_project();
                    if let Some(i) = self.project.index.position(&rel) {
                        self.project.at = Some(crate::tree::Row::File(i));
                        self.project.scroll = self.project.at.clone();
                    }
                }
                self.status = format!("Copied {rel} to {}", copy.root.display());
            }
            Ok(Err(error)) => self.status = error,
            Err(mpsc::TryRecvError::Empty) => {
                self.sheet_copy = Some(copy);
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(mpsc::TryRecvError::Disconnected) => self.status = "The sheet copy stopped before reporting its result.".into(),
        }
    }

    pub(super) fn drop_library_sheet(&mut self, ctx: &egui::Context, folder: Option<String>) {
        let Some(source) = &self.library_drag else { return };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { self.library_drag = None; return; }
        let Some(pointer) = ctx.pointer_latest_pos() else { return };
        let canvas = self.workspace == Workspace::Build && self.project_rect.contains(pointer);
        let text = if let Some(folder) = &folder {
            format!("Copy {} into {}", source.rel, if folder.is_empty() { "Project" } else { folder })
        } else if canvas {
            format!("Place all tiles from {}", source.rel)
        } else { "Drop into the Project tree to copy the file, or onto its canvas to place tiles".into() };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("library sheet drag")));
        let text = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
        let position = pointer + Vec2::new(12.0, 16.0);
        painter.rect_filled(Rect::from_min_size(position, text.size()).expand(4.0), 4.0, Color32::from_black_alpha(220));
        painter.galley(position, text, Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if !ctx.input(|i| i.pointer.primary_released()) { return; }
        let source = self.library_drag.take().unwrap();
        if let Some(folder) = folder { self.begin_sheet_copy(ctx, source, &folder); }
        else if canvas
            && let Err(error) = self.place_library_sheet(ctx, &source.root, &source.rel, source.tile, pointer) { self.status = error; }
    }

    pub(super) fn place_library_sheet(
        &mut self, ctx: &egui::Context, root: &std::path::Path, rel: &str, tile: [u32; 2], pointer: egui::Pos2,
    ) -> Result<(), String> {
        if !self.project.is_set() { return Err("Open a project folder before placing a sheet.".into()); }
        if self.project_eye { return Err("Turn off Inspect mode before placing tiles.".into()); }
        // Read the source without replacing the open library sheet or its selection.
        let side = crate::sidecar::load_book(root)?.sheets.get(rel).cloned().unwrap_or_default();
        let mut source = Sheet::open(ctx, root, rel, tile, side)?;
        source.library = true;
        let selection = Sel::rect((0, 0), (source.cols().saturating_sub(1), source.rows().saturating_sub(1)));
        let block = source.copy_sel(&selection).ok_or("This sheet has no tiles to place.")?;
        let at = if let Some(sheet) = &self.project.sheet {
            sheet.cell_at(pointer).ok_or("Drop inside the project canvas.")?
        } else {
            self.start_canvas(ctx, block.tile);
            (0, 0)
        };
        let sheet = self.project.sheet.as_mut().ok_or("No project canvas is open.")?;
        let overlay = ctx.input(|i| i.modifiers.shift && !i.modifiers.alt);
        if overlay { sheet.paste_overlay(ctx, at, &block); } else { sheet.paste(ctx, at, &block); }
        self.active = Panel::Project;
        self.pane = (Panel::Project, crate::Spot::Sheet);
        ctx.memory_mut(|memory| memory.request_focus(crate::project_id()));
        self.after_edit();
        self.status = format!("Placed all tiles from {rel}. Undo restores the previous canvas.");
        Ok(())
    }
}
