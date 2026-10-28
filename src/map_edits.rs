// SPDX-License-Identifier: GPL-3.0-only
//! Sheet edits that can change map references, their warning, and recording controls.

use super::*;
use crate::map_changes::{self, Recording, Update};

#[derive(Clone)]
enum Edit {
    Move { from: Sel, at: (u32, u32), block: Block, swap: bool },
    Paste { at: (u32, u32), block: Block, blend: bool },
    Delete(Sel),
    Resize(u32, u32),
    Grid([u32; 2], [u32; 2], [i32; 2]),
}

#[derive(Default)]
pub(super) struct MapUi {
    pending: Option<Edit>,
    clear: bool,
    picking: Option<std::sync::mpsc::Receiver<Option<Vec<PathBuf>>>>,
    updates: Option<Vec<Update>>,
    error: Option<String>,
}

impl Sheet {
    pub fn map_dialog_open(&self) -> bool {
        self.map_ui.pending.is_some() || self.map_ui.clear || self.map_ui.updates.is_some()
            || self.map_ui.picking.is_some() || self.map_ui.error.is_some()
    }
    pub(super) fn map_history_allowed(&mut self, redo: bool) -> bool {
        let step = if redo { self.redo.last() } else { self.undo.last() };
        let current = self.side.map_changes.as_ref().map(|r| &r.session);
        if step.is_some_and(|s| s.side.map_changes.as_ref().map(|r| &r.session) != current) {
            self.map_ui.error = Some("Undo and Redo stop at the start or clearing of a map recording.".into());
            return false;
        }
        !self.map_dialog_open()
    }
    pub fn move_block(&mut self, ctx: &egui::Context, from: &Sel, at: (u32, u32), block: &Block) {
        self.request_edit(ctx, Edit::Move { from: from.clone(), at, block: block.clone(), swap: false });
    }
    pub fn swap_block(&mut self, ctx: &egui::Context, from: &Sel, at: (u32, u32), block: &Block) {
        self.request_edit(ctx, Edit::Move { from: from.clone(), at, block: block.clone(), swap: true });
    }
    pub(super) fn paste_guarded(&mut self, ctx: &egui::Context, at: (u32, u32), block: &Block, blend: bool) -> Result<(), String> {
        if !blend && self.tile != block.tile && !block.props.is_empty() {
            return Err("Tiles with properties need the same tile size in both sheets. The paste was cancelled.".into());
        }
        self.request_edit(ctx, Edit::Paste { at, block: block.clone(), blend });
        Ok(())
    }
    pub fn clear_selection(&mut self, ctx: &egui::Context) {
        if !self.sel.is_empty() { self.request_edit(ctx, Edit::Delete(self.sel.clone())); }
    }
    pub fn trim(&mut self, ctx: &egui::Context) {
        if let Some((cols, rows)) = self.trim_size() { self.resize_canvas(ctx, cols, rows); }
    }
    pub(super) fn resize_canvas(&mut self, ctx: &egui::Context, cols: u32, rows: u32) {
        if (cols, rows) != (self.cols(), self.rows()) { self.request_edit(ctx, Edit::Resize(cols, rows)); }
    }
    pub fn set_grid(&mut self, ctx: &egui::Context, tile: [u32; 2], gap: [u32; 2], offset: [i32; 2]) -> bool {
        if tile.contains(&0) || (tile, gap, clamp_offset(offset, tile, gap)) == (self.tile, self.gap, self.offset)
            || !self.props.is_empty() || self.props_error.is_some() { return false; }
        self.request_edit(ctx, Edit::Grid(tile, gap, offset))
    }

    /// Maps old cell identities through the same writes as the pixel operation.
    fn edit_mapping(&self, edit: &Edit) -> Vec<Option<u32>> {
        let (cols, rows) = (self.cols(), self.rows());
        let (nc, nr) = match edit {
            Edit::Resize(c, r) => (*c, *r),
            Edit::Move { at, block, .. } | Edit::Paste { at, block, .. } => (
                cols.max(at.0 + block.img.width().div_ceil(self.tile[0])),
                rows.max(at.1 + block.img.height().div_ceil(self.tile[1]))),
            _ => (cols, rows),
        };
        let old = |x, y| (x < cols && y < rows).then_some(y * cols + x);
        let mut cells = vec![None; (nc * nr) as usize];
        for y in 0..rows.min(nr) { for x in 0..cols.min(nc) { cells[(y * nc + x) as usize] = old(x, y); } }
        match edit {
            Edit::Move { from, at, block, swap } => {
                if let Some(origin) = from.origin() {
                    for (x, y) in from.iter() { cells[(y * nc + x) as usize] = None; }
                    for j in 0..block.rows { for i in 0..block.cols {
                        if block.mask[(j * block.cols + i) as usize] {
                            let target = (at.0 + i, at.1 + j);
                            // An empty destination supplies the empty cell left by a move.
                            // Following that identity also makes a move back cancel the record.
                            if !swap && !from.contains(target)
                                && old(target.0, target.1).is_some_and(|id| !self.occupied_cell(id)) {
                                let source = (origin.0 + i, origin.1 + j);
                                let source_is_target = source.0 >= at.0 && source.1 >= at.1
                                    && source.0 - at.0 < block.cols && source.1 - at.1 < block.rows
                                    && block.mask[((source.1 - at.1) * block.cols + source.0 - at.0) as usize];
                                if !source_is_target { cells[(source.1 * nc + source.0) as usize] = old(target.0, target.1); }
                            }
                            cells[((at.1 + j) * nc + at.0 + i) as usize] = old(origin.0 + i, origin.1 + j);
                        }
                    } }
                    if *swap { for j in 0..block.rows { for i in 0..block.cols {
                        if block.mask[(j * block.cols + i) as usize] {
                            cells[((origin.1 + j) * nc + origin.0 + i) as usize] = old(at.0 + i, at.1 + j);
                        }
                    } } }
                }
            }
            Edit::Delete(sel) => { for (x, y) in sel.iter() { if x < nc && y < nr { cells[(y * nc + x) as usize] = None; } } }
            Edit::Grid(..) => cells.fill(None),
            _ => (),
        }
        let mut mapping = vec![None; (cols * rows) as usize];
        for (new, old) in cells.into_iter().enumerate() { if let Some(old) = old { mapping[old as usize] = Some(new as u32); } }
        mapping
    }

    fn request_edit(&mut self, ctx: &egui::Context, edit: Edit) -> bool {
        if self.map_ui.pending.is_some() { return false; }
        if matches!(&edit, Edit::Move {from, at, ..} if from.origin() == Some(*at)) { return false; }
        let mapping = self.edit_mapping(&edit);
        let changed = mapping.iter().enumerate().any(|(i, p)| *p != Some(i as u32));
        let recording = self.side.map_changes.as_ref().is_some_and(|r| r.enabled && r.complete);
        // A removal cannot be repaired by changing map references. Ask even while recording.
        let destroys_art = mapping.iter().enumerate().any(|(id, p)| p.is_none() && self.occupied_cell(id as u32));
        if !self.library && self.side.used_in_maps != Some(false) && changed
            && (!recording || destroys_art || matches!(edit, Edit::Grid(..))) {
            self.map_ui.pending = Some(edit);
            return false;
        }
        self.execute_edit(ctx, edit, mapping);
        true
    }

    fn occupied_cell(&self, id: u32) -> bool {
        if self.props.ids().contains(&id) { return true; }
        let (x0, y0, x1, y1) = self.cell_img_rect(id % self.cols(), id / self.cols());
        (y0..y1).any(|y| (x0..x1).any(|x| self.img.get_pixel(x, y).0[3] != 0))
    }

    fn execute_edit(&mut self, ctx: &egui::Context, edit: Edit, mapping: Vec<Option<u32>>) {
        match edit {
            Edit::Move { from, at, block, swap } => {
                if swap { self.swap_block_raw(ctx, &from, at, &block); }
                else { self.move_block_raw(ctx, &from, at, &block); }
            }
            Edit::Paste { at, block, blend } => {
                if let Err(e) = self.paste_mode(ctx, at, &block, blend) { self.map_ui.error = Some(e); return; }
            }
            Edit::Delete(sel) => { self.sel = sel; self.clear_selection_raw(); }
            Edit::Resize(cols, rows) => {
                self.snapshot();
                self.canvas_resize = Some(CanvasDrag {
                    dirty: self.dirty,
                    wh: (true, true), img: self.img.clone(), prov: self.prov.clone(), side: self.side.clone(),
                    props: self.props.clone(), cols: self.cols(),
                });
                self.set_size(ctx, cols, rows);
                self.canvas_resize = None;
            }
            Edit::Grid(tile, gap, offset) => { self.set_grid_raw(ctx, tile, gap, offset); }
        }
        if let Some(r) = &mut self.side.map_changes {
            if r.enabled && r.complete { r.apply(&mapping); }
            else if mapping.iter().enumerate().any(|(i, p)| *p != Some(i as u32)) { r.complete = false; }
        }
    }

    fn start_recording(&mut self) -> bool {
        if self.side.map_changes.as_ref().is_some_and(|r| !r.complete) {
            self.map_ui.error = Some("The earlier recording is incomplete. Clear it before recording again.".into());
            return false;
        }
        let count = self.cols() * self.rows();
        self.side.map_changes.get_or_insert_with(|| Recording::new(count)).enabled = true;
        self.side.used_in_maps = Some(true);
        self.dirty = true;
        true
    }

    pub fn map_controls(&mut self, ui: &mut egui::Ui) {
        ui.strong("Maps");
        let mut used = self.side.used_in_maps == Some(true);
        if ui.checkbox(&mut used, "Used in maps").changed() {
            self.side.used_in_maps = Some(used);
            if !used && let Some(r) = &mut self.side.map_changes { r.enabled = false; }
            self.dirty = true;
        }
        if !used && self.side.map_changes.is_none() {
            ui.weak("Enable this when maps start using the sheet.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let mut enabled = self.side.map_changes.as_ref().is_some_and(|r| r.enabled);
            if ui.toggle_value(&mut enabled, "Record").on_hover_text("Remember tile moves for a later map update").changed() {
                if enabled { self.start_recording(); }
                else if let Some(r) = &mut self.side.map_changes { r.enabled = false; self.dirty = true; }
            }
            if let Some(r) = &self.side.map_changes {
                if r.complete {
                    ui.label(format!("{} changes", r.changed())).on_hover_text(format!("{} removed tile references", r.deleted()));
                } else { ui.colored_label(ui.visuals().warn_fg_color, "Incomplete recording"); }
            }
        });
        if let Some(r) = &self.side.map_changes {
            let can_update = r.complete && self.map_ui.picking.is_none() && self.map_ui.updates.is_none() && !self.dirty;
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(can_update, egui::Button::new("Update maps...")).on_hover_text("Save the sheet first, then select maps to update").clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let paths = rfd::FileDialog::new().set_title("Update maps").add_filter("Tiled JSON maps", &["tmj", "json"]).pick_files();
                        let _ = tx.send(paths);
                        ctx.request_repaint();
                    });
                    self.map_ui.picking = Some(rx);
                }
                if ui.button("Clear").clicked() { self.map_ui.clear = true; }
            });
        }
    }

    pub fn map_dialogs(&mut self, ctx: &egui::Context) -> Option<String> {
        if let Some(rx) = &self.map_ui.picking {
            match rx.try_recv() {
                Ok(paths) => {
                    self.map_ui.picking = None;
                    if let Some(paths) = paths {
                        let prepared = paths.iter().map(|p| map_changes::prepare(p, &crate::tileset::path(&self.dir, &self.rel),
                            self.side.map_changes.as_ref().unwrap(), self.cols() * self.rows())).collect::<Result<Vec<_>, _>>();
                        match prepared { Ok(updates) => self.map_ui.updates = Some(updates), Err(e) => self.map_ui.error = Some(e) }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(_) => { self.map_ui.picking = None; self.map_ui.error = Some("The map picker stopped before returning a result.".into()); }
            }
        }
        if let Some(edit) = &self.map_ui.pending {
            let grid = matches!(edit, Edit::Grid(..));
            let mapping = self.edit_mapping(edit);
            let removed = mapping.iter().enumerate().any(|(id, p)| p.is_none() && self.occupied_cell(id as u32));
            let mut choice = None;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("map edit warning")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading("Existing maps");
                if grid { ui.label("Changing the grid can break existing maps. This change cannot be recorded."); }
                else if removed { ui.label("This edit removes existing tiles. Recording cannot repair maps that use those tiles."); }
                else { ui.label("This move can affect existing maps. Record it to update them later."); }
                crate::dialogs::footer(ui, |ui| {
                    if !grid && ui.button("Record").clicked() { choice = Some(true); }
                    if ui.button("No maps").on_hover_text("Turn off 'Used in maps' for this sheet and continue").clicked() { choice = Some(false); }
                    if ui.button("Cancel").clicked() { cancel = true; }
                });
            });
            if let Some(record) = choice {
                if record && !self.start_recording() { self.map_ui.pending = None; return None; }
                if !record {
                    self.side.used_in_maps = Some(false);
                    if let Some(r) = &mut self.side.map_changes { r.enabled = false; }
                    self.dirty = true;
                }
                let edit = self.map_ui.pending.take().unwrap();
                self.execute_edit(ctx, edit, mapping);
            } else if cancel || modal.should_close() {
                self.map_ui.pending = None;
            }
            return None;
        }
        if self.map_ui.clear {
            let mut clear = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("clear map recording")).show(ctx, |ui| {
                ui.heading("Clear recording?");
                ui.label("This discards the recorded changes. It does not undo your sheet edits or update any maps.");
                crate::dialogs::footer(ui, |ui| {
                    clear = ui.button("Clear").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if clear {
                self.side.map_changes = None;
                self.dirty = true;
            }
            if clear || cancel || modal.should_close() { self.map_ui.clear = false; }
        }
        if let Some(updates) = &self.map_ui.updates {
            let mut apply = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("update maps preview")).show(ctx, |ui| {
                ui.heading("Update maps?");
                ui.label("Newly selected maps must use the layout from when this recording began.");
                ui.label("Close these maps in Tiled first. Each changed map gets a backup beside it.");
                for update in updates {
                    ui.label(format!("{}: {} references", update.path.file_name().unwrap().to_string_lossy(), update.references));
                }
                crate::dialogs::footer(ui, |ui| {
                    apply = ui.button("Update").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if apply {
                let updates = self.map_ui.updates.take().unwrap();
                return Some(match self.apply_map_updates(&updates) {
                    Ok(n) => format!("Updated {n} maps. Backups are beside the map files."), Err(e) => { self.map_ui.error = Some(e.clone()); e }
                });
            }
            if cancel || modal.should_close() { self.map_ui.updates = None; }
        }
        if let Some(error) = self.map_ui.error.clone() {
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::new("map error")).show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.heading("Map changes");
                ui.label(error);
                close = ui.button("Close").clicked();
            });
            if close || modal.should_close() { self.map_ui.error = None; }
        }
        None
    }

    fn apply_map_updates(&mut self, updates: &[Update]) -> Result<usize, String> {
        if self.dirty { return Err("The sheet changed after the preview. Save it and select the maps again.".into()); }
        for u in updates { u.verify()?; }
        let mut written = 0;
        for u in updates {
            if u.references == 0 { continue; }
            let r = self.side.map_changes.as_mut().ok_or("The recording no longer exists.")?;
            r.updates.insert(u.key.clone(), u.receipt.clone());
            // Keep both hashes before writing the map, including when the write fails.
            crate::sidecar::store_layout(&self.dir, &self.rel, &self.side)?;
            u.write().map_err(|e| format!("{written} maps updated before the failure: {e}"))?;
            written += 1;
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::Folder;

    fn fixture() -> (egui::Context, Folder, Sheet) {
        let ctx = egui::Context::default();
        let dir = Folder::new();
        let mut sheet = Sheet::new_empty(&ctx, &dir.0, "tiles.png", [8, 8], 3, 2);
        sheet.img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        (ctx, dir, sheet)
    }

    #[test]
    fn unused_sheets_are_quiet_and_unknown_sheets_wait_before_mutating() {
        let (ctx, _dir, mut s) = fixture();
        let from = Sel::rect((0, 0), (0, 0));
        let block = s.copy_sel(&from).unwrap();
        s.move_block(&ctx, &from, (1, 0), &block);
        assert!(s.map_ui.pending.is_none());
        assert!(s.side.map_changes.is_none());
        s.undo(&ctx);
        s.side.used_in_maps = None;
        let image = s.img.clone();
        let undo = s.undo.len();
        s.move_block(&ctx, &from, (1, 0), &block);
        assert!(s.map_ui.pending.is_some());
        assert_eq!(s.img, image);
        assert_eq!(s.undo.len(), undo);
        s.map_ui.pending = None; // Cancel leaves the sheet alone.
        assert_eq!(s.img, image);
    }

    #[test]
    fn record_and_continue_persists_and_undo_tracks_final_positions() {
        let (ctx, dir, mut s) = fixture();
        s.side.used_in_maps = Some(true);
        let from = Sel::rect((0, 0), (0, 0));
        let block = s.copy_sel(&from).unwrap();
        s.move_block(&ctx, &from, (1, 0), &block);
        let edit = s.map_ui.pending.take().unwrap();
        assert!(s.start_recording());
        let mapping = s.edit_mapping(&edit);
        s.execute_edit(&ctx, edit, mapping);
        assert_eq!(s.side.map_changes.as_ref().unwrap().positions[..3], [Some(1), Some(0), Some(2)]);
        s.undo(&ctx);
        assert_eq!(s.side.map_changes.as_ref().unwrap().changed(), 0);
        s.redo(&ctx);
        assert_eq!(s.side.map_changes.as_ref().unwrap().changed(), 2);
        s.save().unwrap();
        let side = crate::sidecar::load_book(&dir.0).unwrap().sheets["tiles.png"].clone();
        let mut reopened = Sheet::open(&ctx, &dir.0, "tiles.png", [8, 8], side).unwrap();
        assert!(reopened.side.map_changes.as_ref().unwrap().enabled);
        let from = Sel::rect((1, 0), (1, 0));
        let block = reopened.copy_sel(&from).unwrap();
        reopened.move_block(&ctx, &from, (0, 0), &block);
        assert_eq!(reopened.side.map_changes.as_ref().unwrap().changed(), 0);
    }

    #[test]
    fn resize_paste_growth_and_trim_use_the_same_warning_and_mapping() {
        let (ctx, _dir, mut s) = fixture();
        s.side.used_in_maps = Some(true);
        s.resize_canvas(&ctx, 4, 2);
        assert_eq!(s.cols(), 3);
        assert!(s.map_ui.pending.take().is_some());
        s.start_recording();
        s.resize_canvas(&ctx, 4, 2);
        assert_eq!(s.side.map_changes.as_ref().unwrap().positions, [Some(0), Some(1), Some(2), Some(4), Some(5), Some(6)]);
        s.undo(&ctx);
        let block = s.copy_sel(&Sel::rect((0, 0), (0, 0))).unwrap();
        s.paste_overlay(&ctx, (3, 0), &block).unwrap();
        assert_eq!(s.side.map_changes.as_ref().unwrap().positions[3], Some(4));
        s.undo(&ctx);
        s.trim(&ctx);
        assert_eq!((s.cols(), s.rows()), (1, 1));
        assert_eq!(s.side.map_changes.as_ref().unwrap().deleted(), 5);
    }

    #[test]
    fn merge_is_quiet_but_deleting_art_warns_even_during_recording() {
        let (ctx, _dir, mut s) = fixture();
        s.start_recording();
        let block = s.copy_sel(&Sel::rect((0, 0), (0, 0))).unwrap();
        s.paste_overlay(&ctx, (1, 0), &block).unwrap();
        assert!(!s.map_dialog_open());
        assert_eq!(s.side.map_changes.as_ref().unwrap().changed(), 0);
        s.sel = Sel::rect((0, 0), (0, 0));
        s.clear_selection(&ctx);
        assert!(s.map_ui.pending.is_some());
        assert_eq!(s.img.get_pixel(0, 0).0[3], 255);
    }

    #[test]
    fn stopping_recording_keeps_it_and_no_maps_cannot_leave_a_false_valid_record() {
        let (ctx, _dir, mut s) = fixture();
        s.start_recording();
        s.side.map_changes.as_mut().unwrap().enabled = false;
        let from = Sel::rect((0, 0), (0, 0));
        let block = s.copy_sel(&from).unwrap();
        s.move_block(&ctx, &from, (1, 0), &block);
        assert!(s.map_ui.pending.is_some());
        let edit = s.map_ui.pending.take().unwrap();
        s.side.used_in_maps = Some(false);
        let mapping = s.edit_mapping(&edit);
        s.execute_edit(&ctx, edit, mapping);
        assert!(!s.side.map_changes.as_ref().unwrap().complete);
        s.undo(&ctx);
        assert!(s.side.map_changes.as_ref().unwrap().complete);
        assert_eq!(s.side.used_in_maps, Some(false));
    }

    #[test]
    fn undo_keeps_map_receipts_and_stops_at_recording_boundaries() {
        let (ctx, _dir, mut s) = fixture();
        let block = s.copy_sel(&Sel::rect((0, 0), (0, 0))).unwrap();
        s.paste(&ctx, (1, 0), &block).unwrap();
        s.start_recording();
        let from = Sel::rect((0, 0), (0, 0));
        s.swap_block(&ctx, &from, (1, 0), &block);
        let r = s.side.map_changes.as_mut().unwrap();
        r.updates.insert("example".into(), map_changes::Receipt {
            before_hash: "old".into(), after_hash: "new".into(), before: vec![], after: vec![],
        });
        s.undo(&ctx);
        assert_eq!(s.side.map_changes.as_ref().unwrap().updates.len(), 1);
        assert_eq!(s.side.map_changes.as_ref().unwrap().changed(), 0);
        let image = s.img.clone();
        s.undo(&ctx);
        assert_eq!(s.img, image);
        assert!(s.map_ui.error.is_some());
    }
}
