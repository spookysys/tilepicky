//! Tilepick: browse a large set of sheets, search them, and copy
//! cells into tilemaps of your own.
//!
//! Usage: `tilepick <source dir> <destination dir>`

mod index;
mod sheet;
mod sidecar;
mod tree;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, TextureHandle, Vec2};
use index::Index;
use sheet::{Block, Sel, Sheet};
use sidecar::Animation;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use tree::{Node, TreeAction};

/// A new tilemap starts with this many cells.
/// The sizes the tile field steps through when dragged. Typing allows any
/// size, so the list stays short.
const TILE_SIZES: [u32; 12] = [4, 8, 10, 12, 16, 24, 32, 48, 64, 128, 256, 512];

/// A new tilemap starts near this size, rounded to whole tiles.
const NEW_PX: u32 = 512;

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Source,
    Mine,
}

/// A block on its way from one place to another, under the pointer.
struct Drag {
    block: Block,
    from: Panel,
    /// The cells the block was lifted from.
    origin: Sel,
    /// Whether the lift took the panel's own selection, which then follows
    /// the block; a lone tile leaves every selection untouched.
    from_selection: bool,
    /// The grabbed cell, relative to the block.
    grab: (u32, u32),
    ghost: TextureHandle,
}

/// What to do once the user has decided about unsaved changes.
#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Open(usize),
    Create,
    Close,
}

/// What the name prompt is for.
#[derive(Clone, PartialEq)]
enum NameFor {
    SaveAs,
    RenameFile(String),
    DuplicateFile(String),
    NewFolder(String),
    RenameFolder(String),
}

struct NamePrompt {
    title: String,
    value: String,
    what: NameFor,
    focus: bool,
}

struct App {
    drag: Option<Drag>,
    prompt: Option<NamePrompt>,
    /// Files marked with Ctrl+click in MY TILEMAPS.
    marked: HashSet<usize>,
    /// The last plainly clicked file, for shift ranges.
    tree_anchor: Option<usize>,
    /// Where the MINE pane sat last frame, for drops onto the empty pane.
    mine_rect: Rect,
    /// A pending deletion, waiting for the user's yes.
    confirm: Option<(String, Vec<String>)>,
    /// An action that waits for the save dialog.
    pending: Option<Pending>,
    src: Index,
    dst: Index,
    src_tree: Node,
    dst_tree: Node,
    query: String,
    qwords: Vec<String>,
    src_visible: Option<Vec<bool>>,
    dst_visible: Option<Vec<bool>>,
    src_sheet: Option<Sheet>,
    dst_sheet: Option<Sheet>,
    src_sel: Option<usize>,
    dst_sel: Option<usize>,
    active: Panel,
    clip: Option<Block>,
    new_name: String,
    status: String,
    /// Set when the query changes, so that the trees expand once to show the matches.
    open_trees: bool,
    /// Where the split between the source and your tilemap sits, as a fraction of the height.
    split: f32,
    /// The status as last shown, and when it changed; it fades after a while.
    shown_status: String,
    status_at: std::time::Instant,
}

fn src_id() -> Id {
    Id::new("source sheet")
}
fn dst_id() -> Id {
    Id::new("my sheet")
}

impl App {
    fn new(src_root: PathBuf, dst_root: PathBuf, tile: [u32; 2]) -> Self {
        let src = Index::scan(&src_root, tile);
        let mut dst = Index::scan(&dst_root, tile);
        migrate_sidecars(&mut dst);
        Self {
            drag: None,
            prompt: None,
            marked: HashSet::new(),
            tree_anchor: None,
            mine_rect: Rect::NOTHING,
            confirm: None,
            pending: None,
            src_tree: Node::build(&src.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &src.dirs),
            dst_tree: Node::build(&dst.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &dst.dirs),
            status: format!("{} files", src.entries.len()),
            src,
            dst,
            query: String::new(),
            qwords: Vec::new(),
            src_visible: None,
            dst_visible: None,
            src_sheet: None,
            dst_sheet: None,
            src_sel: None,
            dst_sel: None,
            active: Panel::Source,
            clip: None,
            new_name: String::new(),
            open_trees: false,
            split: 0.5,
            shown_status: String::new(),
            status_at: std::time::Instant::now(),
        }
    }

    fn refresh_query(&mut self) {
        self.qwords = index::query_words(&self.query);
        self.open_trees = true;
        self.src_visible = self.src.visible(&self.qwords);
        self.dst_visible = self.dst.visible(&self.qwords);
    }

    /// The grid to assume for a sheet whose entry names none: the sheet now
    /// open in the same panel, else the run's default.
    fn inherited_grid(&self, panel: Panel) -> ([u32; 2], u32, u32) {
        let (sheet, default) = match panel {
            Panel::Source => (&self.src_sheet, self.src.tile),
            Panel::Mine => (&self.dst_sheet, self.dst.tile),
        };
        sheet.as_ref().map_or((default, 0, 0), |s| (s.tile, s.gap, s.offset))
    }

    fn open_source(&mut self, ctx: &egui::Context, i: usize) {
        let e = &self.src.entries[i];
        match Sheet::open(ctx, &self.src.root, &e.rel, self.inherited_grid(Panel::Source), e.side.clone()) {
            Ok(mut s) => {
                if let Some(prev) = &self.src_sheet {
                    s.zoom = prev.zoom;
                }
                self.src_sheet = Some(s);
                self.src_sel = Some(i);
                self.active = Panel::Source;
            }
            Err(err) => self.status = err,
        }
    }

    fn open_mine(&mut self, ctx: &egui::Context, i: usize) {
        let e = &self.dst.entries[i];
        match Sheet::open(ctx, &self.dst.root, &e.rel, self.inherited_grid(Panel::Mine), e.side.clone()) {
            Ok(mut s) => {
                if let Some(prev) = &self.dst_sheet {
                    s.zoom = prev.zoom;
                }
                self.dst_sheet = Some(s);
                self.dst_sel = Some(i);
                self.active = Panel::Mine;
            }
            Err(err) => self.status = err,
        }
    }

    fn create_mine(&mut self, ctx: &egui::Context) {
        let name = self.new_name.trim().trim_end_matches(".png").to_string();
        if name.is_empty() {
            return;
        }
        let rel = format!("{name}.png");
        let tile = self.inherited_grid(Panel::Mine).0;
        let cols = ((NEW_PX + tile[0] / 2) / tile[0]).max(1);
        let rows = ((NEW_PX + tile[1] / 2) / tile[1]).max(1);
        let mut sheet = Sheet::new_empty(ctx, &self.dst.root, &rel, tile, cols, rows);
        if let Err(e) = sheet.save() {
            self.status = e;
            return;
        }
        self.new_name.clear();
        self.rescan_mine();
        if let Some(i) = self.dst.position(&rel) {
            self.open_mine(ctx, i);
        }
    }

    fn rescan_source(&mut self) {
        self.src = Index::scan(&self.src.root, self.src.tile);
        self.src_tree = Node::build(&self.src.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &self.src.dirs);
        self.src_visible = self.src.visible(&self.qwords);
        self.src_sel = self.src_sheet.as_ref().and_then(|s| self.src.position(&s.rel));
        self.status = format!("{} source files", self.src.entries.len());
    }

    fn rescan_mine(&mut self) {
        self.marked.clear();
        self.dst = Index::scan(&self.dst.root, self.dst.tile);
        self.dst_tree = Node::build(&self.dst.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &self.dst.dirs);
        self.dst_visible = self.dst.visible(&self.qwords);
        if let Some(rel) = self.dst_sheet.as_ref().map(|s| s.rel.clone()) {
            self.dst_sel = self.dst.position(&rel);
        }
    }

    /// A clean relative path from a typed name; `ext` is added when missing.
    fn normalize_name(name: &str, ext: Option<&str>) -> Option<String> {
        let name = name.trim().trim_matches('/');
        if name.is_empty() || name.split('/').any(|p| p.trim().is_empty() || p == "." || p == "..") {
            return None;
        }
        let mut rel = name.to_string();
        if let Some(ext) = ext {
            if !rel.to_ascii_lowercase().ends_with(ext) {
                rel.push_str(ext);
            }
        }
        Some(rel)
    }

    /// The name prompt for Save As, renames, duplicates, and new folders.
    fn name_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.prompt else { return };
        let mut apply = false;
        let mut cancel = false;
        egui::Modal::new(Id::new("name dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(&prompt.title);
            let r = ui.add(egui::TextEdit::singleline(&mut prompt.value).desired_width(f32::INFINITY));
            if prompt.focus {
                r.request_focus();
                prompt.focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                apply = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Ok").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.prompt = None;
        } else if apply {
            let p = self.prompt.take().unwrap();
            if let Err(e) = self.apply_name(ctx, &p.what, &p.value) {
                self.status = e;
            }
        }
    }

    /// Deletes files (and, through their paths, whole folders), with their
    /// book entries. Runs only after the confirm dialog.
    fn delete_paths(&mut self, rels: &[String]) -> Result<(), String> {
        let root = self.dst.root.clone();
        let mut book = sidecar::load_book(&root);
        for rel in rels {
            let path = root.join(rel);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
                book.retain(|k, _| k != rel && !k.starts_with(&format!("{rel}/")));
                if let Some(sheet) = &self.dst_sheet {
                    if sheet.rel.starts_with(&format!("{rel}/")) {
                        self.dst_sheet = None;
                    }
                }
            } else {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                book.remove(rel);
                if self.dst_sheet.as_ref().is_some_and(|s| s.rel == *rel) {
                    self.dst_sheet = None;
                }
            }
        }
        let json = serde_json::to_string_pretty(&book).map_err(|e| e.to_string())?;
        std::fs::write(root.join(sidecar::BOOK), json).map_err(|e| e.to_string())?;
        self.status = format!("deleted {}", rels.join(", "));
        self.rescan_mine();
        Ok(())
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some((message, _)) = &self.confirm else { return };
        let message = message.clone();
        let mut choice = None;
        egui::Modal::new(Id::new("confirm dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Delete");
            ui.label(&message);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                let (_, rels) = self.confirm.take().unwrap();
                if let Err(e) = self.delete_paths(&rels) {
                    self.status = e;
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    fn apply_name(&mut self, ctx: &egui::Context, what: &NameFor, name: &str) -> Result<(), String> {
        let root = self.dst.root.clone();
        match what {
            NameFor::NewFolder(parent) => {
                let rel = Self::normalize_name(name, None).ok_or("that is not a usable name")?;
                let dir = if parent.is_empty() { rel } else { format!("{parent}/{rel}") };
                std::fs::create_dir_all(root.join(&dir)).map_err(|e| e.to_string())?;
                self.rescan_mine();
            }
            NameFor::RenameFolder(old) => {
                let rel = Self::normalize_name(name, None).ok_or("that is not a usable name")?;
                let new = match old.rsplit_once('/') {
                    Some((parent, _)) => format!("{parent}/{rel}"),
                    None => rel,
                };
                if new != *old {
                    if root.join(&new).exists() {
                        return Err(format!("{new} exists"));
                    }
                    std::fs::rename(root.join(old), root.join(&new)).map_err(|e| e.to_string())?;
                    sidecar::move_prefix(&root, old, &new)?;
                    if let Some(sheet) = &mut self.dst_sheet {
                        if let Some(rest) = sheet.rel.strip_prefix(&format!("{old}/")) {
                            sheet.rel = format!("{new}/{rest}");
                        }
                    }
                    self.rescan_mine();
                }
            }
            NameFor::SaveAs => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if root.join(&rel).exists() {
                    return Err(format!("{rel} exists"));
                }
                let Some(sheet) = &mut self.dst_sheet else { return Ok(()) };
                if let Some(parent) = root.join(&rel).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                sheet.rel = rel.clone();
                sheet.save()?;
                self.status = format!("saved as {rel}");
                self.rescan_mine();
            }
            NameFor::RenameFile(old) => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if rel != *old {
                    if root.join(&rel).exists() {
                        return Err(format!("{rel} exists"));
                    }
                    if let Some(parent) = root.join(&rel).parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    std::fs::rename(root.join(old), root.join(&rel)).map_err(|e| e.to_string())?;
                    sidecar::move_entry(&root, old, &rel, false)?;
                    if let Some(sheet) = &mut self.dst_sheet {
                        if sheet.rel == *old {
                            sheet.rel = rel.clone();
                        }
                    }
                    self.rescan_mine();
                }
            }
            NameFor::DuplicateFile(old) => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if root.join(&rel).exists() {
                    return Err(format!("{rel} exists"));
                }
                if let Some(parent) = root.join(&rel).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::copy(root.join(old), root.join(&rel)).map_err(|e| e.to_string())?;
                sidecar::move_entry(&root, old, &rel, true)?;
                self.rescan_mine();
                if let Some(i) = self.dst.position(&rel) {
                    self.open_mine(ctx, i);
                }
            }
        }
        Ok(())
    }

    /// Keeps search in step with an edit. Saving is explicit: Ctrl+S.
    fn after_edit(&mut self) {
        let Some(sheet) = &mut self.dst_sheet else { return };
        if let Some(i) = self.dst_sel {
            self.dst.entries[i].side = sheet.side.clone();
        }
        self.dst_visible = self.dst.visible(&self.qwords);
    }

    fn save(&mut self) {
        let Some(sheet) = &mut self.dst_sheet else { return };
        if sheet.rel.is_empty() {
            self.prompt = Some(NamePrompt { title: "Save as".into(), value: String::new(), what: NameFor::SaveAs, focus: true });
            return;
        }
        match sheet.save() {
            Ok(()) => self.status = format!("saved {}", sheet.rel),
            Err(e) => self.status = e,
        }
    }

    fn trim(&mut self, ctx: &egui::Context) {
        let Some(sheet) = &mut self.dst_sheet else { return };
        let before = (sheet.cols(), sheet.rows());
        sheet.trim(ctx);
        self.status = if (sheet.cols(), sheet.rows()) == before {
            "nothing to trim".to_string()
        } else {
            format!("trimmed to {}x{} cells", sheet.cols(), sheet.rows())
        };
        self.after_edit();
    }

    fn has_unsaved(&self) -> bool {
        self.dst_sheet.as_ref().is_some_and(|s| s.dirty)
    }

    /// Runs the action, or asks about unsaved changes first.
    fn request(&mut self, ctx: &egui::Context, action: Pending) {
        if self.has_unsaved() {
            self.pending = Some(action);
        } else {
            self.run(ctx, action);
        }
    }

    fn run(&mut self, ctx: &egui::Context, action: Pending) {
        match action {
            Pending::Open(i) => self.open_mine(ctx, i),
            Pending::Create => self.create_mine(ctx),
            Pending::Close => {
                self.dst_sheet = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The dialog for unsaved changes: save, discard, or cancel.
    fn save_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending else { return };
        let name = self.dst_sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
        let mut choice = None;
        egui::Modal::new(Id::new("save dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Unsaved changes");
            ui.label(format!("{name} has changes that are not saved."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(true);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(false);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.pending = None;
                }
            });
        });
        if let Some(save) = choice {
            if save && self.dst_sheet.as_ref().is_some_and(|s| s.rel.is_empty()) {
                // No name yet: ask for one; the interrupted action is dropped.
                self.pending = None;
                self.save();
                return;
            }
            if save {
                self.save();
            }
            if let Some(sheet) = &mut self.dst_sheet {
                sheet.dirty = false;
            }
            self.pending = None;
            self.run(ctx, action);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let cmd = Modifiers::COMMAND;
        // Saving works no matter what has focus; a swallowed Ctrl+S loses work.
        if key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            if let Some(sheet) = &self.dst_sheet {
                self.prompt = Some(NamePrompt { title: "Save as".into(), value: sheet.rel.clone(), what: NameFor::SaveAs, focus: true });
            }
        }
        if key(cmd, Key::S) {
            self.save();
        }
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| id == src_id() || id == dst_id()) {
            return;
        }
        // The window layer turns Ctrl+C into a Copy event, and Ctrl+V into a Paste
        // event that only exists when the system clipboard holds text.
        let (copy, cut, paste) = ctx.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let cut = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
            let paste = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
            (copy, cut, paste)
        });
        let cut = cut || key(cmd, Key::X);

        if copy || cut || key(cmd, Key::C) {
            let from = match self.active {
                Panel::Source => &self.src_sheet,
                Panel::Mine => &self.dst_sheet,
            };
            if let Some(b) = from.as_ref().and_then(Sheet::copy) {
                self.status = format!("copied {}x{} cells", b.cols, b.rows);
                // A note in the system clipboard, so that Ctrl+V reaches us as a Paste event.
                ctx.copy_text(b.note());
                self.clip = Some(b);
                // A cut clears the cells; only your tilemap is editable.
                if cut && self.active == Panel::Mine {
                    if let Some(sheet) = &mut self.dst_sheet {
                        sheet.clear_selection(ctx);
                        self.after_edit();
                    }
                }
            }
        }
        if paste || key(cmd, Key::V) {
            if let (Some(block), Some(sheet)) = (&self.clip, &mut self.dst_sheet) {
                let at = sheet.sel.origin().unwrap_or((0, 0));
                sheet.paste(ctx, at, block);
                self.active = Panel::Mine;
                self.after_edit();
            }
        }
        if key(cmd, Key::T) {
            self.trim(ctx);
        }
        if key(cmd, Key::Z) {
            if let Some(sheet) = &mut self.dst_sheet {
                sheet.undo(ctx);
                self.after_edit();
            }
        }
        if self.active == Panel::Mine {
            if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
                if let Some(sheet) = &mut self.dst_sheet {
                    sheet.clear_selection(ctx);
                    self.after_edit();
                }
            }
        }
        if key(cmd, Key::A) {
            if let Some(s) = self.sheet_mut(self.active) {
                s.sel = Sel::rect((0, 0), (s.cols() - 1, s.rows() - 1));
            }
        }
        if key(Modifiers::NONE, Key::A) {
            self.press_a();
        }
        if key(Modifiers::NONE, Key::Escape) {
            if let Some(s) = self.sheet_mut(self.active) {
                s.sel = Sel::default();
            }
        }
        // + and - zoom the view under the pointer, else the active sheet.
        let dir = if key(Modifiers::NONE, Key::Plus) || key(Modifiers::NONE, Key::Equals) {
            1
        } else if key(Modifiers::NONE, Key::Minus) {
            -1
        } else {
            0
        };
        if dir != 0 {
            if let Some(z) = self.zoom_under_pointer() {
                z.step(dir);
            }
        }
    }

    fn start_drag(&mut self, ctx: &egui::Context, from: Panel, cell: (u32, u32)) {
        let sheet = match from {
            Panel::Source => self.src_sheet.as_ref(),
            Panel::Mine => self.dst_sheet.as_ref(),
        };
        let Some(sheet_ref) = sheet else { return };
        let from_selection = sheet_ref.sel.contains(cell);
        let origin = if from_selection { sheet_ref.sel.clone() } else { Sel::rect(cell, cell) };
        let grab = origin.origin().map(|o| (cell.0 - o.0, cell.1 - o.1)).unwrap_or((0, 0));
        let Some(block) = sheet_ref.copy_sel(&origin) else { return };
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [block.img.width() as usize, block.img.height() as usize],
            block.img.as_raw(),
        );
        let ghost = ctx.load_texture("drag ghost", image, egui::TextureOptions::NEAREST);
        self.drag = Some(Drag { block, from, origin, from_selection, grab, ghost });
        self.active = from;
    }

    /// Draws the ghost under the pointer, and drops the block on release.
    fn update_drag(&mut self, ctx: &egui::Context) {
        let Some(drag) = &self.drag else { return };
        let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

        // Over the tilemap the ghost snaps to the grid; elsewhere it floats at the pointer.
        let mut target = self.dst_sheet.as_ref().and_then(|d| {
            let c = d.cell_at(p)?;
            Some((c.0.saturating_sub(drag.grab.0), c.1.saturating_sub(drag.grab.1)))
        });
        // The ghost is drawn at the zoom of the panel it is over, in pixels
        // of the block, since the tile sizes may differ.
        let block_px = Vec2::new(drag.block.img.width() as f32, drag.block.img.height() as f32);
        let src_cell = Vec2::new(drag.block.tile[0] as f32, drag.block.tile[1] as f32);
        let (min, zoom) = match (target, &self.dst_sheet) {
            (Some(t), Some(d)) => {
                let c = d.cell_px();
                (d.screen.min + Vec2::new(t.0 as f32 * c.x, t.1 as f32 * c.y), d.zoom.level)
            }
            _ => {
                let z = match drag.from {
                    Panel::Source => self.src_sheet.as_ref().map_or(2.0, |s| s.zoom.level),
                    Panel::Mine => self.dst_sheet.as_ref().map_or(2.0, |s| s.zoom.level),
                };
                (p - Vec2::new((drag.grab.0 as f32 + 0.5) * src_cell.x, (drag.grab.1 as f32 + 0.5) * src_cell.y) * z, z)
            }
        };
        let size = block_px * zoom;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("drag ghost")));
        let rect = Rect::from_min_size(min, size);
        painter.image(drag.ghost.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_white_alpha(160));
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::from_rgb(80, 160, 255)), egui::StrokeKind::Inside);

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
            return;
        }
        if !ctx.input(|i| i.pointer.primary_released()) {
            return;
        }
        let drag = self.drag.take().unwrap();
        // A drop on the empty pane starts a fresh, unnamed tilemap; its name
        // is asked for at the first save.
        if target.is_none() && self.dst_sheet.is_none() && self.mine_rect.contains(p) {
            let tile = self.inherited_grid(Panel::Mine).0;
            let (cols, rows) = (((NEW_PX + tile[0] / 2) / tile[0]).max(1), ((NEW_PX + tile[1] / 2) / tile[1]).max(1));
            self.dst_sheet = Some(Sheet::new_empty(ctx, &self.dst.root, "", tile, cols, rows));
            self.dst_sel = None;
            target = Some((0, 0));
        }
        let (Some(at), Some(sheet)) = (target, &mut self.dst_sheet) else { return };
        let copy = drag.from == Panel::Source || ctx.input(|i| i.modifiers.command);
        // A lone lifted tile leaves the selections as they were; only a
        // dragged selection keeps following its block.
        let keep = (!drag.from_selection || drag.from == Panel::Source).then(|| sheet.sel.clone());
        if copy {
            sheet.paste(ctx, at, &drag.block);
        } else if Some(at) != drag.origin.origin() {
            sheet.move_block(ctx, &drag.origin, at, &drag.block);
        } else {
            return;
        }
        if let Some(prev) = keep {
            sheet.sel = prev;
        }
        self.active = Panel::Mine;
        self.after_edit();
    }

    /// The tile size field: dragging steps through the usual sizes and
    /// applies on release; a click turns it into a text field for any size.
    fn tile_field(ui: &mut egui::Ui, s: &mut Sheet) -> Option<[u32; 2]> {
        if let Some(buf) = &mut s.tile_text {
            let r = ui.add(egui::TextEdit::singleline(buf).desired_width(56.0));
            if s.tile_text_focus {
                r.request_focus();
                s.tile_text_focus = false;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                s.tile_text = None;
                return None;
            }
            if r.lost_focus() {
                let v = parse_tile(buf);
                s.tile_text = None;
                return v.filter(|t| *t != s.tile);
            }
            return None;
        }
        let r = ui.add(egui::Button::new(format!("{} px", show_tile(s.tile_edit))).small().sense(egui::Sense::click_and_drag()));
        let r = r.on_hover_text("drag: step through the usual square sizes   click: type any size, 48 or 32x48");
        if r.dragged() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            s.tile_acc += r.drag_delta().x;
            let steps = (s.tile_acc / 20.0).trunc() as i32;
            s.tile_acc -= steps as f32 * 20.0;
            if steps != 0 {
                let last = TILE_SIZES.len() as i32 - 1;
                let idx = TILE_SIZES.iter().position(|&t| t >= s.tile_edit[0]).unwrap_or(TILE_SIZES.len() - 1) as i32;
                let n = TILE_SIZES[(idx + steps).clamp(0, last) as usize];
                s.tile_edit = [n, n];
            }
        }
        if r.drag_stopped() {
            s.tile_acc = 0.0;
            if s.tile_edit != s.tile {
                return Some(s.tile_edit);
            }
        }
        if r.clicked() {
            s.tile_text = Some(show_tile(s.tile));
            s.tile_text_focus = true;
        }
        None
    }

    /// Returns the new grid when the user finished editing a field:
    /// (tile, gap, offset).
    fn sheet_header(ui: &mut egui::Ui, title: &str, active: bool, source: bool, sheet: Option<&mut Sheet>) -> Option<([u32; 2], u32, u32)> {
        let mut new_grid = None;
        ui.horizontal(|ui| {
            let color = if active { egui::Color32::from_rgb(80, 160, 255) } else { ui.visuals().weak_text_color() };
            ui.colored_label(color, egui::RichText::new(title).strong());
            let Some(s) = sheet else {
                ui.weak("nothing open");
                return;
            };
            let name = if s.rel.is_empty() { "(unnamed)" } else { s.rel.as_str() };
            ui.label(if s.dirty { format!("{name} *") } else { name.to_string() });
            ui.weak(format!("{}x{} cells", s.cols(), s.rows()));
            ui.label("tile");
            if let Some(t) = Self::tile_field(ui, s) {
                new_grid = Some((t, s.gap, s.offset));
            }
            if source {
                // Sheets drawn with gaps between the tiles, and a border
                // before the first one. The edit fields live on the sheet so
                // that a drag accumulates across frames.
                ui.label("gap");
                let g = ui.add(egui::DragValue::new(&mut s.gap_edit).range(0..=64).speed(0.05));
                ui.label("offset");
                let o = ui.add(egui::DragValue::new(&mut s.offset_edit).range(0..=64).speed(0.05));
                if (g.drag_stopped() || g.lost_focus() || o.drag_stopped() || o.lost_focus())
                    && (s.gap_edit, s.offset_edit) != (s.gap, s.offset)
                {
                    new_grid = Some((s.tile, s.gap_edit, s.offset_edit));
                }
            }
            ui.weak(format!("{}x", s.zoom.level));
            if let Some(b) = s.sel.bounds() {
                ui.weak(format!("sel {} cells, {}x{} at {},{}", s.sel.len(), b.cols(), b.rows(), b.x0, b.y0));
            }
            if let Some((x, y)) = s.hover {
                let from = s.cell_source(x, y).map(|f| format!("<- {f}")).unwrap_or_default();
                // Truncated, so a long path can never widen the row and push
                // the panes around.
                let text = egui::RichText::new(format!("cell {x},{y} {from}")).weak();
                ui.add(egui::Label::new(text).truncate());
            }
        });
        new_grid
    }

    /// `A` opens the animation panel for the selection; once it is open, `A`
    /// stores the draft, or removes the stored animation under the selection.
    fn press_a(&mut self) {
        let panel = self.active;
        let Some(sheet) = self.sheet_mut(panel) else { return };
        if !sheet.show_anim_panel() {
            sheet.open_anim_panel();
            return;
        }
        match sheet.toggle_animation() {
            Ok(()) => self.after_animation_edit(panel),
            Err(e) => self.status = e,
        }
    }

    /// The zoom of the view under the pointer: a preview, a sheet, or the
    /// active sheet as the fallback.
    fn zoom_under_pointer(&mut self) -> Option<&mut sheet::Zoom> {
        let active = self.active;
        let hovered = |s: &Sheet| s.preview_hovered || s.hover.is_some();
        let panel = if self.src_sheet.as_ref().is_some_and(hovered) {
            Panel::Source
        } else if self.dst_sheet.as_ref().is_some_and(hovered) {
            Panel::Mine
        } else {
            active
        };
        let s = self.sheet_mut(panel)?;
        Some(if s.preview_hovered { &mut s.preview_zoom } else { &mut s.zoom })
    }

    /// Applies a new grid (tile, gap, offset) to a sheet. Your tilemap keeps
    /// it as an unsaved edit; a source sheet stores it at once.
    fn change_grid(&mut self, ctx: &egui::Context, panel: Panel, (t, gap, offset): ([u32; 2], u32, u32)) {
        let Some(sheet) = self.sheet_mut(panel) else { return };
        sheet.set_tile(ctx, t);
        sheet.set_gap_offset(ctx, gap, offset);
        self.status = format!("grid: {} px tiles, {gap} px gap, {offset} px offset", show_tile(t));
        match panel {
            Panel::Mine => self.after_edit(),
            Panel::Source => {
                if let Some(sheet) = &mut self.src_sheet {
                    if let Err(e) = sheet.save_entry() {
                        self.status = e;
                    }
                }
                if let (Some(i), Some(sheet)) = (self.src_sel, &self.src_sheet) {
                    self.src.entries[i].side = sheet.side.clone();
                }
            }
        }
    }

    fn sheet_mut(&mut self, panel: Panel) -> Option<&mut Sheet> {
        match panel {
            Panel::Source => self.src_sheet.as_mut(),
            Panel::Mine => self.dst_sheet.as_mut(),
        }
    }

    /// A tilemap keeps the change until Ctrl+S. A source sheet has no pixel
    /// edits, so its book entry is written at once.
    fn after_animation_edit(&mut self, panel: Panel) {
        match panel {
            Panel::Mine => self.after_edit(),
            Panel::Source => {
                let Some(sheet) = &mut self.src_sheet else { return };
                match sheet.save_entry() {
                    Ok(()) => self.status = format!("stored in {}", self.src.root.join(sidecar::BOOK).display()),
                    Err(e) => self.status = e,
                }
                if let Some(i) = self.src_sel {
                    self.src.entries[i].side = sheet.side.clone();
                }
            }
        }
    }

    /// The side panel of a sheet: the selection played as a strip, with
    /// fields for the frame count and the frame time. A stored animation is
    /// edited in place; otherwise the fields shape a draft. Returns whether a
    /// stored animation changed, or the reason a change was refused.
    fn animation_panel(ui: &mut egui::Ui, sheet: &mut Sheet) -> Result<bool, String> {
        ui.strong("Animation");
        let Some(b) = sheet.sel.bounds() else {
            ui.weak("Select a strip of cells to play it.");
            return Ok(false);
        };
        let stored = sheet.stored_animation();
        let (mut frames, mut ms) = match &stored {
            Some(a) => (a.frames, a.ms),
            None => sheet.draft().map(|d| (d.frames, d.ms)).unwrap_or((b.cols(), 100)),
        };
        let sel_width_px = b.cols() * sheet.tile[0];
        // The status line: zoom, and what the fields describe.
        ui.horizontal(|ui| {
            ui.weak(format!("{}x", sheet.preview_zoom.level));
            match &stored {
                Some(a) => ui.weak(format!("stored: {} frames of {}x{} px", a.frames, a.frame[0], a.frame[1])),
                None if sel_width_px % frames != 0 => {
                    ui.colored_label(egui::Color32::from_rgb(200, 60, 40), format!("{frames} does not divide {sel_width_px} px"))
                }
                None => ui.weak(format!("draft: {frames} frames of {}x{} px", sel_width_px / frames, b.rows() * sheet.tile[1])),
            };
        });
        let width = stored.as_ref().map_or(sel_width_px, |a| a.frame[0] * a.frames);
        let mut changed = false;
        egui::Grid::new("animation fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("frames");
            changed |= ui.add(egui::DragValue::new(&mut frames).range(1..=width)).changed();
            ui.end_row();
            ui.label("ms");
            changed |= ui.add(egui::DragValue::new(&mut ms).range(1..=5000)).changed();
            ui.end_row();
        });
        let mut result = Ok(false);
        match &stored {
            Some(_) if changed => result = sheet.set_animation(frames, ms).map(|()| true),
            Some(_) => {}
            None => {
                if let Some(d) = sheet.draft() {
                    d.frames = frames;
                    d.ms = ms;
                }
            }
        }
        // Buttons at the bottom; the preview takes the space between.
        egui::Panel::bottom("animation buttons").show_separator_line(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = if stored.is_some() { "Unmark (A)" } else { "Store (A)" };
                if ui.button(label).clicked() {
                    result = sheet.toggle_animation().map(|()| true);
                }
                if stored.is_none() && ui.button("Hide").clicked() {
                    sheet.anim_panel = false;
                }
            });
        });
        // The strip to play: the stored one, or the draft when it divides.
        let tile = sheet.tile;
        let anim = stored.clone().or_else(|| {
            let d = sheet.draft()?;
            (d.area.cols() * tile[0] % d.frames == 0).then(|| d.animation(tile))
        });
        if let Some(a) = anim {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::both().id_salt("animation preview").auto_shrink([false, false]).show(ui, |ui| {
                    Self::play(ui, sheet, &a);
                });
            });
        }
        result
    }

    /// Draws the frame of the strip that is due now.
    fn play(ui: &mut egui::Ui, sheet: &mut Sheet, a: &Animation) {
        let t = ui.input(|i| i.time);
        let frame = (((t * 1000.0) as u64 / a.ms.max(1) as u64) % a.frames as u64) as u32;
        let zoom = sheet.preview_zoom.level;
        ui.add_space(6.0);
        let size = Vec2::new(a.frame[0] as f32, a.frame[1] as f32) * zoom;
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        sheet.preview_hovered = resp.hovered();
        if resp.hovered() {
            sheet.preview_zoom.wheel(ui);
        }
        ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(225));
        let origin = Pos2::new((a.px[0] + frame * a.frame[0]) as f32, a.px[1] as f32);
        sheet.draw_px_rect(ui.painter(), Rect::from_min_size(origin, Vec2::new(a.frame[0] as f32, a.frame[1] as f32)), rect.min, zoom);
        ui.weak(format!("frame {}/{}", frame + 1, a.frames));
        ui.ctx().request_repaint_after(Duration::from_millis(a.ms.max(16) as u64));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.handle_keys(ctx);
        // The preview sets this flag while drawing; clear it first, so that
        // a closed preview does not keep it.
        for s in [&mut self.src_sheet, &mut self.dst_sheet].into_iter().flatten() {
            s.preview_hovered = false;
        }

        let mut src_action = None;
        let mut dst_action = None;
        let mut dst_order: Vec<usize> = Vec::new();
        let mut delete_in_mine = false;
        let mut create = false;
        egui::Panel::left("left").resizable(true).default_size(340.0).size_range(240.0..=800.0).show(ui, |ui| {
            ui.add_space(4.0);
            let search = egui::TextEdit::singleline(&mut self.query).hint_text("search: rock wall").desired_width(f32::INFINITY);
            if ui.add(search).changed() {
                self.refresh_query();
            }
            ui.add_space(4.0);
            egui::Panel::bottom("legend").show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.weak("click / drag: select   long click and drag: move   ctrl+a: select all   shift+click: rectangle from the last click   ctrl+click: add or remove a cell   ctrl+shift+click: add a rectangle   right click: clear selection / inside it: delete content   (drop with ctrl held: copy)   ctrl+c / ctrl+x / ctrl+v   delete   a: animation panel / store   ctrl+z   ctrl+s: save   ctrl+shift+s: save as   ctrl+t: trim   drag the canvas edge: resize   ctrl+wheel or + / -: zoom the view under the pointer");
            });
            egui::Panel::top("source tree")
                .resizable(true)
                .default_size(ui.available_height() * 0.6)
                .size_range(80.0..=f32::INFINITY)
                .show(ui, |ui| {
                    ui.strong("SOURCE");
                    egui::ScrollArea::vertical().id_salt("source scroll").auto_shrink([false, false]).show(ui, |ui| {
                        src_action = self
                            .src_tree
                            .show(ui, self.src_visible.as_deref(), self.src_sel, None, &self.qwords, self.open_trees, false, "", &mut Vec::new(), &mut Vec::new());
                        let rest = ui.available_size_before_wrap();
                        let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(ui.available_width(), rest.y.max(24.0)), egui::Sense::hover());
                        let bg = ui.interact(rect, Id::new("source free space"), egui::Sense::click());
                        bg.context_menu(|ui| {
                            if ui.button("Refresh").clicked() {
                                src_action = Some(TreeAction::Refresh);
                                ui.close();
                            }
                        });
                    });
                });
            egui::CentralPanel::default().show(ui, |ui| {
                let heading = ui.add(egui::Label::new(egui::RichText::new("MY TILEMAPS").strong()).sense(egui::Sense::click()));
                heading.context_menu(|ui| {
                    if ui.button("New folder…").clicked() {
                        self.prompt = Some(NamePrompt {
                            title: "New folder".into(),
                            value: String::new(),
                            what: NameFor::NewFolder(String::new()),
                            focus: true,
                        });
                        ui.close();
                    }
                });
                // The button first, at the right; the field takes what is left. One row high.
                let row = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("New").clicked() {
                        create = true;
                    }
                    let field = egui::TextEdit::singleline(&mut self.new_name).hint_text("new tilemap name").desired_width(ui.available_width());
                    let r = ui.add(field);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        create = true;
                    }
                });
                egui::ScrollArea::vertical().id_salt("mine scroll").auto_shrink([false, false]).show(ui, |ui| {
                    dst_action = self.dst_tree.show(ui, self.dst_visible.as_deref(), self.dst_sel, Some(&self.marked), &self.qwords, self.open_trees, true, "", &mut Vec::new(), &mut dst_order);
                    // The empty space below the tree offers the folder menu too.
                    let rest = ui.available_size_before_wrap();
                    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(ui.available_width(), rest.y.max(24.0)), egui::Sense::hover());
                    let bg = ui.interact(rect, Id::new("tilemaps free space"), egui::Sense::click());
                    bg.context_menu(|ui| {
                        if ui.button("New folder…").clicked() {
                            self.prompt = Some(NamePrompt { title: "New folder".into(), value: String::new(), what: NameFor::NewFolder(String::new()), focus: true });
                            ui.close();
                        }
                        if ui.button("Refresh").clicked() {
                            dst_action = Some(TreeAction::Refresh);
                            ui.close();
                        }
                    });
                });
            });
        });
        // The status line, under the two sheet panels, text at the right.
        // It disappears ten seconds after it last changed.
        const STATUS_SECS: u64 = 10;
        if self.status != self.shown_status {
            self.shown_status = self.status.clone();
            self.status_at = std::time::Instant::now();
        }
        let age = self.status_at.elapsed();
        egui::Panel::bottom("status").show_separator_line(true).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if age.as_secs() < STATUS_SECS {
                    ui.colored_label(egui::Color32::from_rgb(190, 40, 30), egui::RichText::new(&self.status).strong());
                    ctx.request_repaint_after(Duration::from_secs(STATUS_SECS) - age);
                } else {
                    ui.label("");
                }
            });
        });
        self.open_trees = false;
        match src_action {
            Some(TreeAction::Open(i)) => self.open_source(ctx, i),
            Some(TreeAction::Refresh) => self.rescan_source(),
            _ => {}
        }
        match dst_action {
            Some(TreeAction::Open(i)) => {
                // The plainly clicked file is the start of any group.
                self.marked.clear();
                self.marked.insert(i);
                self.tree_anchor = Some(i);
                self.request(ctx, Pending::Open(i));
            }
            Some(TreeAction::Toggle(i)) => {
                if !self.marked.remove(&i) {
                    self.marked.insert(i);
                }
                self.tree_anchor = Some(i);
            }
            Some(TreeAction::Range(i, additive)) => {
                let a = self.tree_anchor.unwrap_or(i);
                let (pa, pi) = (dst_order.iter().position(|&x| x == a), dst_order.iter().position(|&x| x == i));
                if let (Some(pa), Some(pi)) = (pa, pi) {
                    if !additive {
                        self.marked.clear();
                    }
                    for &e in &dst_order[pa.min(pi)..=pa.max(pi)] {
                        self.marked.insert(e);
                    }
                }
            }
            Some(TreeAction::DeleteFile(i)) => {
                let rel = self.dst.entries[i].rel.clone();
                self.confirm = Some((format!("Delete {rel}? There is no undo."), vec![rel]));
            }
            Some(TreeAction::DeleteMarked) => {
                let rels: Vec<String> = self.marked.iter().map(|&i| self.dst.entries[i].rel.clone()).collect();
                self.confirm = Some((format!("Delete {} files? There is no undo.", rels.len()), rels));
            }
            Some(TreeAction::Refresh) => self.rescan_mine(),
            Some(TreeAction::DeleteFolder(dir)) => {
                self.confirm = Some((format!("Delete the folder {dir} and everything in it? There is no undo."), vec![dir]));
            }
            Some(TreeAction::RenameFile(i)) => {
                let rel = self.dst.entries[i].rel.clone();
                self.prompt = Some(NamePrompt { title: "Rename".into(), value: rel.clone(), what: NameFor::RenameFile(rel), focus: true });
            }
            Some(TreeAction::DuplicateFile(i)) => {
                let rel = self.dst.entries[i].rel.clone();
                let suggestion = format!("{} copy", rel.trim_end_matches(".png"));
                self.prompt = Some(NamePrompt { title: "Duplicate".into(), value: suggestion, what: NameFor::DuplicateFile(rel), focus: true });
            }
            Some(TreeAction::NewFolder(dir)) => {
                self.prompt = Some(NamePrompt { title: "New folder".into(), value: String::new(), what: NameFor::NewFolder(dir), focus: true });
            }
            Some(TreeAction::RenameFolder(dir)) => {
                let name = dir.rsplit_once('/').map(|(_, n)| n).unwrap_or(&dir).to_string();
                self.prompt = Some(NamePrompt { title: "Rename folder".into(), value: name, what: NameFor::RenameFolder(dir), focus: true });
            }
            None => {}
        }
        self.name_dialog(ctx);
        self.confirm_dialog(ctx);
        if create {
            self.request(ctx, Pending::Create);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.has_unsaved() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        self.save_dialog(ctx);

        let dragging = self.drag.is_some();
        let mut drag_from = None;
        let mut anim_changed = Ok(false);
        let mut src_anim = Ok(false);
        let mut src_tile = None;
        let mut dst_tile = None;
        let mut resized = false;
        // The split is kept as a fraction of the height, so that it stays in
        // place when the window changes size. The panel state is written from
        // it each frame and read back after the user drags the divider.
        let total = ui.available_height();
        let panel_id = Id::new("source panel");
        let rect = Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.available_width(), total * self.split));
        ctx.data_mut(|d| d.insert_persisted(panel_id, egui::PanelState { outer_rect: rect }));
        egui::Panel::top("source panel")
            .resizable(true)
            .show(ui, |ui| {
                src_tile = Self::sheet_header(ui, "SOURCE", self.active == Panel::Source, true, self.src_sheet.as_mut());
                if let Some(s) = &mut self.src_sheet {
                    if s.show_anim_panel() {
                        egui::Panel::right("source animation").resizable(true).default_size(220.0).show(ui, |ui| {
                            src_anim = Self::animation_panel(ui, s);
                        });
                    }
                    let ev = egui::CentralPanel::default().show(ui, |ui| s.view(ui, src_id(), dragging, false)).inner;
                    if ev.interacted {
                        self.active = Panel::Source;
                    }
                    if let Some(grab) = ev.drag_block {
                        drag_from = Some((Panel::Source, grab));
                    }
                } else {
                    // Fill the panel, so that it keeps its height and can be dragged.
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.weak("Open a sheet on the left, or type a search.");
                    });
                }
            });
        if let Some(state) = egui::PanelState::load(ctx, panel_id) {
            if total > 0.0 {
                self.split = (state.outer_rect.height() / total).clamp(0.1, 0.9);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.mine_rect = ui.max_rect();
            dst_tile = Self::sheet_header(ui, "MINE", self.active == Panel::Mine, false, self.dst_sheet.as_mut());
            if let Some(s) = &mut self.dst_sheet {
                if s.show_anim_panel() {
                    egui::Panel::right("my animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        anim_changed = Self::animation_panel(ui, s);
                    });
                }
                let ev = egui::CentralPanel::default().show(ui, |ui| s.view(ui, dst_id(), dragging, true)).inner;
                if ev.interacted {
                    self.active = Panel::Mine;
                }
                if ev.resized {
                    resized = true;
                }
                if let Some(grab) = ev.drag_block {
                    drag_from = Some((Panel::Mine, grab));
                }
                if ev.delete {
                    s.clear_selection(ctx);
                    delete_in_mine = true;
                }
            } else {
                ui.weak("Create or open a tilemap on the left. Then select cells in the source, Ctrl+C, click a cell here, Ctrl+V.");
            }
        });
        match anim_changed {
            Ok(true) => self.after_animation_edit(Panel::Mine),
            Ok(false) => {}
            Err(e) => self.status = e,
        }
        if let Some(g) = src_tile {
            self.change_grid(ctx, Panel::Source, g);
        }
        if let Some(g) = dst_tile {
            self.change_grid(ctx, Panel::Mine, g);
        }
        if delete_in_mine {
            self.after_edit();
        }
        if resized {
            if let Some(s) = &self.dst_sheet {
                self.status = format!("resized to {}x{} cells", s.cols(), s.rows());
            }
            self.after_edit();
        }
        match src_anim {
            Ok(true) => self.after_animation_edit(Panel::Source),
            Ok(false) => {}
            Err(e) => self.status = e,
        }
        if let Some((from, grab)) = drag_from {
            self.start_drag(ctx, from, grab);
        }
        self.update_drag(ctx);
    }
}

/// Moves the old `name.json` files next to tilemaps into the book, once.
fn migrate_sidecars(dst: &mut Index) {
    for e in &mut dst.entries {
        let old = dst.root.join(&e.rel).with_extension("json");
        if !e.side.is_empty() || !old.exists() {
            continue;
        }
        let Some(side) = std::fs::read_to_string(&old).ok().and_then(|s| serde_json::from_str::<sidecar::Sidecar>(&s).ok()) else {
            continue;
        };
        if sidecar::store_entry(&dst.root, &e.rel, &side).is_ok() {
            let _ = std::fs::remove_file(&old);
            e.side = side;
        }
    }
}

/// "32" or "32x48", in pixels.
fn parse_tile(text: &str) -> Option<[u32; 2]> {
    let text = text.trim().trim_end_matches("px").trim();
    let ok = |n: u32| (1..=1024).contains(&n);
    if let Some((w, h)) = text.split_once(['x', 'X']) {
        let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        (ok(w) && ok(h)).then_some([w, h])
    } else {
        let n = text.parse().ok()?;
        ok(n).then_some([n, n])
    }
}

/// "32" for square tiles, "32x48" otherwise.
fn show_tile(t: [u32; 2]) -> String {
    if t[0] == t[1] { format!("{}", t[0]) } else { format!("{}x{}", t[0], t[1]) }
}

fn main() -> eframe::Result {
    let mut tile: [u32; 2] = [32, 32];
    let mut dirs: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--tile" {
            tile = args.next().as_deref().and_then(parse_tile).unwrap_or_else(|| {
                eprintln!("--tile needs a pixel size like 32 or 32x48, between 1 and 1024");
                std::process::exit(2);
            });
        } else {
            dirs.push(a);
        }
    }
    if dirs.len() != 2 {
        eprintln!("usage: tilepick [--tile N|WxH] <source dir> <destination dir>");
        std::process::exit(2);
    }
    let src = PathBuf::from(&dirs[0]);
    let dst = PathBuf::from(&dirs[1]);
    if let Err(e) = std::fs::create_dir_all(&dst) {
        eprintln!("cannot create {}: {e}", dst.display());
        std::process::exit(1);
    }
    let icon = image::load_from_memory(include_bytes!("../icon.png")).expect("icon.png").to_rgba8();
    let icon = egui::IconData { width: icon.width(), height: icon.height(), rgba: icon.into_raw() };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_title("Tilepick")
            .with_icon(icon)
            .with_app_id("tilepick"),
        ..Default::default()
    };
    eframe::run_native(
        "tilepick",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(App::new(src, dst, tile)))
        }),
    )
}
