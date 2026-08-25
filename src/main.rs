// SPDX-License-Identifier: GPL-3.0-only
//! Tilepicky: browse a large set of sheets, search them, and copy
//! cells into tilesheets of your own.
//!
//! Usage: `tilepicky [<library dir> [<project dir>]]`

mod index;
mod settings;
mod sheet;
mod sidecar;
mod tree;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, TextureHandle, Vec2};
use index::Index;
use sheet::{Block, Sel, Sheet};
use sidecar::{Animation, Pair};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tree::{Node, TreeAction};

/// A new tilesheet starts with this many cells.
/// The sizes the tile field steps through when dragged. Typing allows any
/// size, so the list stays short.
const TILE_SIZES: [u32; 12] = [4, 8, 10, 12, 16, 24, 32, 48, 64, 128, 256, 512];

/// A new tilesheet starts near this size, rounded to whole tiles.
const NEW_PX: u32 = 512;
/// The tile size a folder starts with when nothing has said otherwise.
const TILE: [u32; 2] = [32, 32];

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Library,
    Project,
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
    /// Files marked with Ctrl+click in MY TILESHEETS.
    marked: HashSet<usize>,
    /// The last plainly clicked file, for shift ranges.
    tree_anchor: Option<usize>,
    /// Where the arrow keys stand in MY TILESHEETS; the moving end of a range.
    tree_cursor: Option<usize>,
    /// What the tool remembers between runs: the two folders and their
    /// tile sizes.
    settings: settings::Settings,
    /// A folder dialog is open for this side; the answer arrives on the channel.
    picking: Option<(Panel, std::sync::mpsc::Receiver<Option<PathBuf>>)>,
    /// A drag across the files started here and marks a group while it lasts.
    sweep: Option<usize>,
    /// Files held in the air, waiting for a folder to land in.
    file_drag: Option<Vec<String>>,
    /// A file the arrow keys moved to, to bring into view next frame.
    library_scroll: Option<usize>,
    project_scroll: Option<usize>,
    /// Where the project pane sat last frame, for drops onto the empty pane.
    project_rect: Rect,
    /// A pending deletion, waiting for the user's yes.
    confirm: Option<(String, Vec<String>)>,
    /// An action that waits for the save dialog.
    pending: Option<Pending>,
    library: Index,
    project: Index,
    library_tree: Node,
    project_tree: Node,
    query: String,
    qwords: Vec<String>,
    library_visible: Option<Vec<bool>>,
    project_visible: Option<Vec<bool>>,
    library_sheet: Option<Sheet>,
    project_sheet: Option<Sheet>,
    library_sel: Option<usize>,
    project_sel: Option<usize>,
    active: Panel,
    clip: Option<Block>,
    new_name: String,
    status: String,
    /// Set when the query changes, so that the trees expand once to show the matches.
    open_trees: bool,
    /// Where the split between the library and the project sits, as a fraction of the height.
    split: f32,
    /// The status as last shown, and when it changed; it fades after a while.
    shown_status: String,
    status_at: std::time::Instant,
}

fn library_id() -> Id {
    Id::new("library sheet")
}
fn project_id() -> Id {
    Id::new("project sheet")
}

impl App {
    fn new(settings: settings::Settings) -> Self {
        let root = |s: &Option<PathBuf>| s.clone().unwrap_or_default();
        let library = Index::scan(&root(&settings.library.path), settings.library.tile.map_or(TILE, Pair::xy));
        let mut project = Index::scan(&root(&settings.project.path), settings.project.tile.map_or(TILE, Pair::xy));
        migrate_sidecars(&mut project);
        Self {
            settings,
            picking: None,
            drag: None,
            prompt: None,
            marked: HashSet::new(),
            tree_anchor: None,
            tree_cursor: None,
            sweep: None,
            file_drag: None,
            library_scroll: None,
            project_scroll: None,
            project_rect: Rect::NOTHING,
            confirm: None,
            pending: None,
            library_tree: Node::build(&library.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &library.dirs),
            project_tree: Node::build(&project.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &project.dirs),
            status: String::new(),
            library,
            project,
            query: String::new(),
            qwords: Vec::new(),
            library_visible: None,
            project_visible: None,
            library_sheet: None,
            project_sheet: None,
            library_sel: None,
            project_sel: None,
            active: Panel::Library,
            clip: None,
            new_name: String::new(),
            open_trees: false,
            split: 0.5,
            shown_status: String::new(),
            status_at: std::time::Instant::now(),
        }
    }

    /// Whether a side has a folder to work in.
    fn is_set(&self, panel: Panel) -> bool {
        !self.index(panel).root.as_os_str().is_empty()
    }

    fn index(&self, panel: Panel) -> &Index {
        match panel {
            Panel::Library => &self.library,
            Panel::Project => &self.project,
        }
    }

    /// Opens the folder dialog for one side. It runs on its own thread, so
    /// the window keeps drawing while the dialog is up.
    fn ask_folder(&mut self, panel: Panel) {
        if self.picking.is_some() {
            return;
        }
        let (title, at) = match panel {
            Panel::Library => ("Choose your library folder", self.settings.library.path.clone()),
            Panel::Project => ("Choose your project folder", self.settings.project.path.clone()),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = at.filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            let _ = tx.send(dialog.pick_folder());
        });
        self.picking = Some((panel, rx));
    }

    /// Takes the answer of an open folder dialog, once it comes.
    fn poll_folder(&mut self, ctx: &egui::Context) {
        let Some((panel, rx)) = &self.picking else {
            return;
        };
        let panel = *panel;
        match rx.try_recv() {
            Ok(answer) => {
                self.picking = None;
                if let Some(dir) = answer {
                    self.set_folder(ctx, panel, dir);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
        }
    }

    /// Points one side at a folder: reads it, drops what was open there, and
    /// remembers it for the next run.
    fn set_folder(&mut self, ctx: &egui::Context, panel: Panel, dir: PathBuf) {
        let _ = ctx;
        match panel {
            Panel::Library => {
                self.settings.library.path = Some(dir.clone());
                self.library.root = dir;
                self.library_sheet = None;
                self.library_sel = None;
                self.rescan_library();
            }
            Panel::Project => {
                self.settings.project.path = Some(dir.clone());
                self.project.root = dir;
                self.project_sheet = None;
                self.project_sel = None;
                self.marked.clear();
                self.rescan_project();
            }
        }
        self.settings.save();
        let name = match panel {
            Panel::Library => "library",
            Panel::Project => "project",
        };
        self.status = format!("{name}: {}", self.index(panel).root.display());
    }

    /// Remembers the tile size a side used last, in the folder's own book and
    /// in the settings, so that a new sheet there starts with it.
    fn remember_tile(&mut self, panel: Panel, tile: [u32; 2]) {
        if !self.is_set(panel) {
            return;
        }
        let root = self.index(panel).root.clone();
        let _ = sidecar::store_tile(&root, tile);
        match panel {
            Panel::Library => {
                self.library.tile = tile;
                self.settings.library.tile = Some(Pair::of(tile));
            }
            Panel::Project => {
                self.project.tile = tile;
                self.settings.project.tile = Some(Pair::of(tile));
            }
        }
        self.settings.save();
    }

    fn refresh_query(&mut self) {
        self.qwords = index::query_words(&self.query);
        self.open_trees = true;
        self.library_visible = self.library.visible(&self.qwords);
        self.project_visible = self.project.visible(&self.qwords);
    }

    /// The tile size to assume for a sheet whose entry names none: the sheet
    /// now open in the same panel, else the folder's default.
    fn inherited_tile(&self, panel: Panel) -> [u32; 2] {
        let (sheet, default) = match panel {
            Panel::Library => (&self.library_sheet, self.library.tile),
            Panel::Project => (&self.project_sheet, self.project.tile),
        };
        sheet.as_ref().map_or(default, |s| s.tile)
    }

    /// The arrow keys walk the file tree of the panel in use, and open what
    /// they reach. In MY TILESHEETS, Shift and the arrows grow the marked
    /// group instead, and Enter opens the file the group ends on.
    fn tree_keys(&mut self, ctx: &egui::Context, library_order: &[usize], project_order: &[usize]) {
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| id == library_id() || id == project_id()) {
            return;
        }
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k)) as i32;
        // Shift first: `consume_key` ignores an extra Shift, so the plain
        // arrows would eat the shifted ones.
        let grow = key(Modifiers::SHIFT, Key::ArrowDown) - key(Modifiers::SHIFT, Key::ArrowUp);
        let step = key(Modifiers::NONE, Key::ArrowDown) - key(Modifiers::NONE, Key::ArrowUp);
        let enter = key(Modifiers::NONE, Key::Enter) != 0;
        if step == 0 && grow == 0 && !enter {
            return;
        }
        match self.active {
            Panel::Library => {
                if let Some(i) = walk(library_order, self.library_sel, step + grow) {
                    self.open_library(ctx, i);
                    self.library_scroll = Some(i);
                }
            }
            Panel::Project => {
                let cursor = self.tree_cursor.or(self.project_sel);
                if grow != 0 {
                    // The group runs from the anchor to the new cursor.
                    let Some(i) = walk(project_order, cursor, grow) else {
                        return;
                    };
                    let a = self.tree_anchor.or(cursor).unwrap_or(i);
                    self.mark_range(project_order, a, i, false);
                    self.tree_anchor = Some(a);
                    self.tree_cursor = Some(i);
                    self.project_scroll = Some(i);
                } else if step != 0 {
                    let Some(i) = walk(project_order, cursor, step) else {
                        return;
                    };
                    self.marked.clear();
                    self.marked.insert(i);
                    self.tree_anchor = Some(i);
                    self.tree_cursor = Some(i);
                    self.project_scroll = Some(i);
                    self.request(ctx, Pending::Open(i));
                } else if let Some(i) = cursor {
                    self.request(ctx, Pending::Open(i));
                }
            }
        }
    }

    /// Marks every file from `a` to `i` in the order the tree shows them.
    /// `additive` keeps the files that are marked already.
    fn mark_range(&mut self, order: &[usize], a: usize, i: usize, additive: bool) {
        let (pa, pi) = (order.iter().position(|&x| x == a), order.iter().position(|&x| x == i));
        let (Some(pa), Some(pi)) = (pa, pi) else {
            return;
        };
        if !additive {
            self.marked.clear();
        }
        for &e in &order[pa.min(pi)..=pa.max(pi)] {
            self.marked.insert(e);
        }
    }

    fn open_library(&mut self, ctx: &egui::Context, i: usize) {
        let e = &self.library.entries[i];
        match Sheet::open(ctx, &self.library.root, &e.rel, self.inherited_tile(Panel::Library), e.side.clone()) {
            Ok(mut s) => {
                if let Some(prev) = &self.library_sheet {
                    s.zoom = prev.zoom;
                }
                self.library_sheet = Some(s);
                self.library_sel = Some(i);
                self.active = Panel::Library;
            }
            Err(err) => self.status = err,
        }
    }

    fn open_project(&mut self, ctx: &egui::Context, i: usize) {
        let e = &self.project.entries[i];
        match Sheet::open(ctx, &self.project.root, &e.rel, self.inherited_tile(Panel::Project), e.side.clone()) {
            Ok(mut s) => {
                if let Some(prev) = &self.project_sheet {
                    s.zoom = prev.zoom;
                }
                self.project_sheet = Some(s);
                self.project_sel = Some(i);
                self.active = Panel::Project;
            }
            Err(err) => self.status = err,
        }
    }

    fn create_project(&mut self, ctx: &egui::Context) {
        let name = self.new_name.trim().trim_end_matches(".png").to_string();
        if name.is_empty() {
            return;
        }
        let rel = format!("{name}.png");
        let tile = self.inherited_tile(Panel::Project);
        let cols = ((NEW_PX + tile[0] / 2) / tile[0]).max(1);
        let rows = ((NEW_PX + tile[1] / 2) / tile[1]).max(1);
        let mut sheet = Sheet::new_empty(ctx, &self.project.root, &rel, tile, cols, rows);
        if let Err(e) = sheet.save() {
            self.status = e;
            return;
        }
        self.new_name.clear();
        self.rescan_project();
        if let Some(i) = self.project.position(&rel) {
            self.open_project(ctx, i);
        }
    }

    fn rescan_library(&mut self) {
        self.library = Index::scan(&self.library.root, self.library.tile);
        self.library_tree = Node::build(&self.library.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &self.library.dirs);
        self.library_visible = self.library.visible(&self.qwords);
        self.library_sel = self.library_sheet.as_ref().and_then(|s| self.library.position(&s.rel));
        self.status = format!("{} files in the library", self.library.entries.len());
    }

    fn rescan_project(&mut self) {
        self.marked.clear();
        self.project = Index::scan(&self.project.root, self.project.tile);
        self.project_tree = Node::build(&self.project.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &self.project.dirs);
        self.project_visible = self.project.visible(&self.qwords);
        if let Some(rel) = self.project_sheet.as_ref().map(|s| s.rel.clone()) {
            self.project_sel = self.project.position(&rel);
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
        let Some(prompt) = &mut self.prompt else {
            return;
        };
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
        let root = self.project.root.clone();
        let mut book = sidecar::load_book(&root);
        for rel in rels {
            let path = root.join(rel);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
                book.sheets.retain(|k, _| k != rel && !k.starts_with(&format!("{rel}/")));
                if let Some(sheet) = &self.project_sheet {
                    if sheet.rel.starts_with(&format!("{rel}/")) {
                        self.project_sheet = None;
                    }
                }
            } else {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                book.sheets.remove(rel);
                if self.project_sheet.as_ref().is_some_and(|s| s.rel == *rel) {
                    self.project_sheet = None;
                }
            }
        }
        let json = serde_json::to_string_pretty(&book).map_err(|e| e.to_string())?;
        std::fs::write(root.join(sidecar::BOOK), json).map_err(|e| e.to_string())?;
        self.status = format!("deleted {}", rels.join(", "));
        self.rescan_project();
        Ok(())
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some((message, _)) = &self.confirm else {
            return;
        };
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

    /// Puts a file's path on the clipboard: the whole path, or the short
    /// form for the directory tilepicky runs in.
    fn copy_path(&mut self, ctx: &egui::Context, root: &Path, rel: &str, whole: bool) {
        let abs = file_path(root, rel);
        let text = if whole { home_path(&abs) } else { near_path(&abs) };
        ctx.copy_text(text.clone());
        self.status = format!("copied {text}");
    }

    /// Draws the files in the air and lands them when the button opens. The
    /// folder under the pointer takes them; Ctrl copies instead of moving.
    fn drop_files(&mut self, ctx: &egui::Context, hover_dir: Option<String>) {
        let Some(files) = &self.file_drag else { return };
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.file_drag = None;
            return;
        }
        let copying = ctx.input(|i| i.modifiers.command);
        if let Some(p) = ctx.pointer_latest_pos() {
            let what = match files.as_slice() {
                [one] => one.rsplit_once('/').map_or(one.clone(), |(_, n)| n.to_string()),
                many => format!("{} files", many.len()),
            };
            let text = if copying { format!("copy {what}") } else { what };
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("file drag")));
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
            let at = p + Vec2::new(14.0, 10.0);
            painter.rect_filled(Rect::from_min_size(at, galley.size()).expand(3.0), 3.0, Color32::from_black_alpha(190));
            painter.galley(at, galley, Color32::WHITE);
        }
        if !ctx.input(|i| i.pointer.primary_released()) {
            return;
        }
        let files = self.file_drag.take().unwrap();
        let Some(dir) = hover_dir else { return };
        let copy = ctx.input(|i| i.modifiers.command);
        for rel in &files {
            let name = rel.rsplit_once('/').map_or(rel.as_str(), |(_, n)| n);
            let new = if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };
            if let Err(e) = self.relocate(rel, &new, copy) {
                self.status = e;
            }
        }
        let what = if copy { "copied" } else { "moved" };
        let where_to = if dir.is_empty() { "the top".to_string() } else { dir.clone() };
        self.status = format!("{} {what} to {where_to}", files.len());
        self.marked.clear();
        self.rescan_project();
    }

    /// Moves or copies one file of MY TILESHEETS, with its book entry. The
    /// open sheet follows its own file.
    fn relocate(&mut self, old: &str, new: &str, copy: bool) -> Result<(), String> {
        let root = self.project.root.clone();
        if new == old {
            return Ok(());
        }
        if root.join(new).exists() {
            return Err(format!("{new} exists"));
        }
        if let Some(parent) = root.join(new).parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if copy {
            std::fs::copy(root.join(old), root.join(new)).map_err(|e| e.to_string())?;
        } else {
            std::fs::rename(root.join(old), root.join(new)).map_err(|e| e.to_string())?;
            if let Some(sheet) = &mut self.project_sheet {
                if sheet.rel == old {
                    sheet.rel = new.to_string();
                }
            }
        }
        sidecar::move_entry(&root, old, new, copy)
    }

    fn apply_name(&mut self, ctx: &egui::Context, what: &NameFor, name: &str) -> Result<(), String> {
        let root = self.project.root.clone();
        match what {
            NameFor::NewFolder(parent) => {
                let rel = Self::normalize_name(name, None).ok_or("that is not a usable name")?;
                let dir = if parent.is_empty() { rel } else { format!("{parent}/{rel}") };
                std::fs::create_dir_all(root.join(&dir)).map_err(|e| e.to_string())?;
                self.rescan_project();
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
                    if let Some(sheet) = &mut self.project_sheet {
                        if let Some(rest) = sheet.rel.strip_prefix(&format!("{old}/")) {
                            sheet.rel = format!("{new}/{rest}");
                        }
                    }
                    self.rescan_project();
                }
            }
            NameFor::SaveAs => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if root.join(&rel).exists() {
                    return Err(format!("{rel} exists"));
                }
                let Some(sheet) = &mut self.project_sheet else {
                    return Ok(());
                };
                if let Some(parent) = root.join(&rel).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                sheet.rel = rel.clone();
                sheet.save()?;
                self.status = format!("saved as {rel}");
                self.rescan_project();
            }
            NameFor::RenameFile(old) => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if rel != *old {
                    self.relocate(old, &rel, false)?;
                    self.rescan_project();
                }
            }
            NameFor::DuplicateFile(old) => {
                let rel = Self::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                self.relocate(old, &rel, true)?;
                self.rescan_project();
                if let Some(i) = self.project.position(&rel) {
                    self.open_project(ctx, i);
                }
            }
        }
        Ok(())
    }

    /// Keeps search in step with an edit. Saving is explicit: Ctrl+S.
    fn after_edit(&mut self) {
        let Some(sheet) = &mut self.project_sheet else {
            return;
        };
        if let Some(i) = self.project_sel {
            self.project.entries[i].side = sheet.side.clone();
        }
        self.project_visible = self.project.visible(&self.qwords);
    }

    fn save(&mut self) {
        let Some(sheet) = &mut self.project_sheet else {
            return;
        };
        if sheet.rel.is_empty() {
            self.prompt = Some(NamePrompt {
                title: "Save as".into(),
                value: String::new(),
                what: NameFor::SaveAs,
                focus: true,
            });
            return;
        }
        match sheet.save() {
            Ok(()) => self.status = format!("saved {}", sheet.rel),
            Err(e) => self.status = e,
        }
    }

    fn trim(&mut self, ctx: &egui::Context) {
        let Some(sheet) = &mut self.project_sheet else {
            return;
        };
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
        self.project_sheet.as_ref().is_some_and(|s| s.dirty)
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
            Pending::Open(i) => self.open_project(ctx, i),
            Pending::Create => self.create_project(ctx),
            Pending::Close => {
                self.project_sheet = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The dialog for unsaved changes: save, discard, or cancel.
    fn save_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending else { return };
        let name = self.project_sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
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
            if save && self.project_sheet.as_ref().is_some_and(|s| s.rel.is_empty()) {
                // No name yet: ask for one; the interrupted action is dropped.
                self.pending = None;
                self.save();
                return;
            }
            if save {
                self.save();
            }
            if let Some(sheet) = &mut self.project_sheet {
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
            if let Some(sheet) = &self.project_sheet {
                self.prompt = Some(NamePrompt {
                    title: "Save as".into(),
                    value: sheet.rel.clone(),
                    what: NameFor::SaveAs,
                    focus: true,
                });
            }
        }
        if key(cmd, Key::S) {
            self.save();
        }
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| id == library_id() || id == project_id()) {
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
                Panel::Library => &self.library_sheet,
                Panel::Project => &self.project_sheet,
            };
            if let Some(b) = from.as_ref().and_then(Sheet::copy) {
                self.status = format!("copied {}x{} cells", b.cols, b.rows);
                // A note in the system clipboard, so that Ctrl+V reaches us as a Paste event.
                ctx.copy_text(b.note());
                self.clip = Some(b);
                // A cut clears the cells; only your tilesheet is editable.
                if cut && self.active == Panel::Project {
                    if let Some(sheet) = &mut self.project_sheet {
                        sheet.clear_selection(ctx);
                        self.after_edit();
                    }
                }
            }
        }
        if paste || key(cmd, Key::V) {
            if let (Some(block), Some(sheet)) = (&self.clip, &mut self.project_sheet) {
                let at = sheet.sel.origin().unwrap_or((0, 0));
                sheet.paste(ctx, at, block);
                self.active = Panel::Project;
                self.after_edit();
            }
        }
        if key(cmd, Key::T) {
            self.trim(ctx);
        }
        if key(cmd, Key::Z) {
            if let Some(sheet) = &mut self.project_sheet {
                sheet.undo(ctx);
                self.after_edit();
            }
        }
        if self.active == Panel::Project {
            if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
                if let Some(sheet) = &mut self.project_sheet {
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
            Panel::Library => self.library_sheet.as_ref(),
            Panel::Project => self.project_sheet.as_ref(),
        };
        let Some(sheet_ref) = sheet else { return };
        let from_selection = sheet_ref.sel.contains(cell);
        let origin = if from_selection { sheet_ref.sel.clone() } else { Sel::rect(cell, cell) };
        let grab = origin.origin().map(|o| (cell.0 - o.0, cell.1 - o.1)).unwrap_or((0, 0));
        let Some(block) = sheet_ref.copy_sel(&origin) else {
            return;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([block.img.width() as usize, block.img.height() as usize], block.img.as_raw());
        let ghost = ctx.load_texture("drag ghost", image, egui::TextureOptions::NEAREST);
        self.drag = Some(Drag {
            block,
            from,
            origin,
            from_selection,
            grab,
            ghost,
        });
        self.active = from;
    }

    /// Draws the ghost under the pointer, and drops the block on release.
    fn update_drag(&mut self, ctx: &egui::Context) {
        let Some(drag) = &self.drag else { return };
        let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

        // Over the tilesheet the ghost snaps to the grid; elsewhere it floats at the pointer.
        let mut target = self.project_sheet.as_ref().and_then(|d| {
            let c = d.cell_at(p)?;
            Some((c.0.saturating_sub(drag.grab.0), c.1.saturating_sub(drag.grab.1)))
        });
        // The ghost is drawn at the zoom of the panel it is over, in pixels
        // of the block, since the tile sizes may differ.
        let block_px = Vec2::new(drag.block.img.width() as f32, drag.block.img.height() as f32);
        let library_cell = Vec2::new(drag.block.tile[0] as f32, drag.block.tile[1] as f32);
        let (min, zoom) = match (target, &self.project_sheet) {
            (Some(t), Some(d)) => {
                let c = d.cell_px();
                (d.screen.min + Vec2::new(t.0 as f32 * c.x, t.1 as f32 * c.y), d.zoom_px())
            }
            _ => {
                let z = match drag.from {
                    Panel::Library => self.library_sheet.as_ref().map_or(2.0, |s| s.zoom_px()),
                    Panel::Project => self.project_sheet.as_ref().map_or(2.0, |s| s.zoom_px()),
                };
                (
                    p - Vec2::new((drag.grab.0 as f32 + 0.5) * library_cell.x, (drag.grab.1 as f32 + 0.5) * library_cell.y) * z,
                    z,
                )
            }
        };
        let size = block_px * zoom;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("drag ghost")));
        let rect = Rect::from_min_size(min, size);
        painter.image(
            drag.ghost.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_white_alpha(160),
        );
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::from_rgb(80, 160, 255)), egui::StrokeKind::Inside);
        // What the drop will do, when a key changes it: a plus for a copy,
        // two arrows for a swap. The sign sits on the block itself, so that
        // only one thing follows the pointer. A block from the library can
        // only be copied, so it needs no sign.
        if drag.from == Panel::Project {
            let (ctrl, alt) = ctx.input(|i| (i.modifiers.command, i.modifiers.alt));
            if ctrl || alt {
                drop_sign(&painter, rect.min + Vec2::splat(3.0), ctrl);
            }
        }

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
            return;
        }
        if !ctx.input(|i| i.pointer.primary_released()) {
            return;
        }
        let drag = self.drag.take().unwrap();
        // A drop on the empty pane starts a fresh, unnamed tilesheet; its name
        // is asked for at the first save.
        if target.is_none() && self.project_sheet.is_none() && self.project_rect.contains(p) {
            // The new tilesheet takes the grid of the block that lands on it.
            let tile = drag.block.tile;
            let (cols, rows) = (((NEW_PX + tile[0] / 2) / tile[0]).max(1), ((NEW_PX + tile[1] / 2) / tile[1]).max(1));
            self.project_sheet = Some(Sheet::new_empty(ctx, &self.project.root, "", tile, cols, rows));
            self.project_sel = None;
            target = Some((0, 0));
        }
        let (Some(at), Some(sheet)) = (target, &mut self.project_sheet) else {
            return;
        };
        let copy = drag.from == Panel::Library || ctx.input(|i| i.modifiers.command);
        // Alt exchanges the two places. A library sheet never changes, so a
        // block from there can only be copied.
        let swap = !copy && ctx.input(|i| i.modifiers.alt);
        // A lone lifted tile leaves the selections as they were; only a
        // dragged selection keeps following its block.
        let keep = (!drag.from_selection || drag.from == Panel::Library).then(|| sheet.sel.clone());
        if copy {
            sheet.paste(ctx, at, &drag.block);
        } else if Some(at) == drag.origin.origin() {
            return;
        } else if swap {
            sheet.swap_block(ctx, &drag.origin, at, &drag.block);
        } else {
            sheet.move_block(ctx, &drag.origin, at, &drag.block);
        }
        if let Some(prev) = keep {
            sheet.sel = prev;
        }
        self.active = Panel::Project;
        self.after_edit();
    }

    /// Returns the new grid when the user finished editing a field:
    /// (tile, gap, offset).
    fn sheet_header(ui: &mut egui::Ui, title: &str, active: bool, library: bool, sheet: Option<&mut Sheet>) -> Option<([u32; 2], [u32; 2], [i32; 2])> {
        let mut new_grid = None;
        ui.horizontal(|ui| {
            // Both titles are blue; the panel that takes the keys wears the
            // strong one.
            let color = if active {
                egui::Color32::from_rgb(80, 160, 255)
            } else {
                egui::Color32::from_rgb(150, 190, 230)
            };
            ui.colored_label(color, egui::RichText::new(title).strong());
            let Some(s) = sheet else {
                ui.weak("nothing open");
                return;
            };
            ui.weak(format!("{}x{} cells", s.cols(), s.rows()));
            ui.label("tile");
            if let Some(t) = tile_field(library).ui(ui, s.tile) {
                new_grid = Some((t, s.gap, s.offset));
            }
            if library {
                // Sheets drawn with gaps between the tiles, and a border
                // before the first one.
                ui.label("gap");
                if let Some(g) = gap_field(library).ui(ui, s.gap) {
                    new_grid = Some((s.tile, g, s.offset));
                }
                ui.label("offset");
                if let Some(o) = offset_field(library).ui(ui, s.offset) {
                    new_grid = Some((s.tile, s.gap, o));
                }
            }
            ui.weak(format!("{}x", s.zoom.level));
            if let Some(b) = s.sel.bounds() {
                ui.weak(format!("sel {} cells, {}x{} at {},{}", s.sel.len(), b.cols(), b.rows(), b.x0, b.y0));
            }
            // The name and the origin of the hovered cell come last, and both
            // are truncated: a long path can never push the fields off screen.
            let name = if s.rel.is_empty() { "(unnamed)" } else { s.rel.as_str() };
            let name = if s.dirty { format!("{name} *") } else { name.to_string() };
            ui.add(egui::Label::new(name).truncate());
            if let Some((x, y)) = s.hover {
                let from = s.cell_source(x, y).map(|f| format!("<- {f}")).unwrap_or_default();
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
        let Some(sheet) = self.sheet_mut(panel) else {
            return;
        };
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
        let panel = if self.library_sheet.as_ref().is_some_and(hovered) {
            Panel::Library
        } else if self.project_sheet.as_ref().is_some_and(hovered) {
            Panel::Project
        } else {
            active
        };
        let s = self.sheet_mut(panel)?;
        Some(if s.preview_hovered { &mut s.preview_zoom } else { &mut s.zoom })
    }

    /// Applies a new grid (tile, gap, offset) to a sheet. Your tilesheet keeps
    /// it as an unsaved edit; a library sheet stores it at once.
    fn change_grid(&mut self, ctx: &egui::Context, panel: Panel, (t, gap, offset): ([u32; 2], [u32; 2], [i32; 2])) {
        let Some(sheet) = self.sheet_mut(panel) else {
            return;
        };
        if !sheet.set_grid(ctx, t, gap, offset) {
            return;
        }
        self.status = format!("grid: {} px tiles, {} px gap, {} px offset", show_tile(t), show_tile(gap), show_tile(offset));
        // The folder keeps the size, so the next sheet without an entry of
        // its own starts with it.
        self.remember_tile(panel, t);
        match panel {
            Panel::Project => self.after_edit(),
            Panel::Library => {
                if let Some(sheet) = &mut self.library_sheet {
                    if let Err(e) = sheet.save_entry() {
                        self.status = e;
                    }
                }
                if let (Some(i), Some(sheet)) = (self.library_sel, &self.library_sheet) {
                    self.library.entries[i].side = sheet.side.clone();
                }
            }
        }
    }

    fn sheet_mut(&mut self, panel: Panel) -> Option<&mut Sheet> {
        match panel {
            Panel::Library => self.library_sheet.as_mut(),
            Panel::Project => self.project_sheet.as_mut(),
        }
    }

    /// A tilesheet keeps the change until Ctrl+S. A library sheet has no pixel
    /// edits, so its book entry is written at once.
    fn after_animation_edit(&mut self, panel: Panel) {
        match panel {
            Panel::Project => self.after_edit(),
            Panel::Library => {
                let Some(sheet) = &mut self.library_sheet else {
                    return;
                };
                match sheet.save_entry() {
                    Ok(()) => self.status = format!("stored in {}", self.library.root.join(sidecar::BOOK).display()),
                    Err(e) => self.status = e,
                }
                if let Some(i) = self.library_sel {
                    self.library.entries[i].side = sheet.side.clone();
                }
            }
        }
    }

    /// The side panel of a sheet: the selection played as an animation, with
    /// fields for the frame grid and the frame time. A stored animation is
    /// edited in place; otherwise the fields shape a draft. Returns whether a
    /// stored animation changed, or the reason a change was refused.
    fn animation_panel(ui: &mut egui::Ui, sheet: &mut Sheet, library: bool) -> Result<bool, String> {
        ui.strong("Animation");
        let Some(b) = sheet.sel.bounds() else {
            ui.weak("Select cells to play them.");
            return Ok(false);
        };
        let stored = sheet.stored_animation();
        let (mut frames, mut ms) = match &stored {
            Some(a) => (a.grid(), a.ms),
            None => sheet.draft().map(|d| (d.frames, d.ms)).unwrap_or(([b.cols(), 1], 100)),
        };
        // The block the frames divide: the stored one, else the selection.
        let [bw, bh] = match &stored {
            Some(a) => {
                let [c, r] = a.grid();
                [a.frame[0] * c, a.frame[1] * r]
            }
            None => [b.cols() * sheet.tile[0], b.rows() * sheet.tile[1]],
        };
        let divides = frames[0] > 0 && frames[1] > 0 && bw % frames[0] == 0 && bh % frames[1] == 0;
        // The status line: zoom, and what the fields describe.
        ui.horizontal(|ui| {
            ui.weak(format!("{}x", sheet.preview_zoom.level));
            match &stored {
                Some(a) => ui.weak(format!("stored: {} frames of {}x{} px", show_frames(a.grid()), a.frame[0], a.frame[1])),
                None if !divides => ui.colored_label(
                    egui::Color32::from_rgb(200, 60, 40),
                    format!("{} does not divide {bw}x{bh} px", show_frames(frames)),
                ),
                None => ui.weak(format!("draft: {} frames of {}x{} px", show_frames(frames), bw / frames[0], bh / frames[1])),
            };
        });
        let mut changed = false;
        egui::Grid::new("animation fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("frames");
            if let Some(f) = frames_field(library).ui(ui, frames) {
                frames = f;
                changed = true;
            }
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
        // The animation to play: the stored one, or the draft when the frame
        // grid divides the selection.
        let tile = sheet.tile;
        let anim = stored.clone().or_else(|| {
            let d = sheet.draft()?;
            d.fits(tile).then(|| d.animation(tile))
        });
        if let Some(a) = anim {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt("animation preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Self::play(ui, sheet, &a);
                    });
            });
        }
        result
    }

    /// Draws the frame that is due now.
    fn play(ui: &mut egui::Ui, sheet: &mut Sheet, a: &Animation) {
        let t = ui.input(|i| i.time);
        let n = a.count().max(1);
        let frame = (((t * 1000.0) as u64 / a.ms.max(1) as u64) % n as u64) as u32;
        let zoom = sheet.preview_zoom_px();
        ui.add_space(6.0);
        let size = Vec2::new(a.frame[0] as f32, a.frame[1] as f32) * zoom;
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        let ppp = ui.ctx().pixels_per_point();
        let rect = egui::Rect::from_min_size(Pos2::new((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp), rect.size());
        sheet.preview_hovered = resp.hovered();
        if resp.hovered() {
            sheet.preview_zoom.wheel(ui);
        }
        ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(225));
        let [fx, fy] = a.frame_px(frame);
        let origin = Pos2::new(fx as f32, fy as f32);
        sheet.draw_px_rect(
            ui.painter(),
            Rect::from_min_size(origin, Vec2::new(a.frame[0] as f32, a.frame[1] as f32)),
            rect.min,
            zoom,
        );
        ui.weak(format!("frame {}/{}", frame + 1, n));
        ui.ctx().request_repaint_after(Duration::from_millis(a.ms.max(16) as u64));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // A right click first closes any open menu. egui closes a menu on any
        // click while it is open, and it does that after the same click has
        // opened the new menu; without this every second right click shows
        // nothing. The press comes a frame before the click that opens.
        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
            egui::Popup::close_all(ctx);
        }
        self.handle_keys(ctx);
        // The preview sets this flag while drawing; clear it first, so that
        // a closed preview does not keep it.
        for s in [&mut self.library_sheet, &mut self.project_sheet].into_iter().flatten() {
            s.preview_hovered = false;
        }

        let mut library_action = None;
        let mut project_action = None;
        // A click in an empty pane asks for that side's folder.
        let (library_set, project_set) = (self.is_set(Panel::Library), self.is_set(Panel::Project));
        let mut ask: Option<Panel> = None;
        let mut hover_dir: Option<String> = None;
        let mut library_order: Vec<usize> = Vec::new();
        let mut project_order: Vec<usize> = Vec::new();
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
                ui.weak("long click and drag: move   click / drag: select   shift+click: rectangle from the last click   ctrl+click: add or remove a cell   ctrl+shift+click: add a rectangle   ctrl+a: select all   right click: clear selection / inside it: delete content   (drop with ctrl held: copy, with alt: swap the two places)   ctrl+c / ctrl+x / ctrl+v   delete   a: animation panel / store   ctrl+z   ctrl+s: save   ctrl+shift+s: save as   ctrl+t: trim   drag the canvas edge: resize   ctrl+wheel or + / -: zoom the view");
            });
            egui::Panel::top("library tree")
                .resizable(true)
                .default_size(ui.available_height() * if project_set { 0.6 } else { 0.45 })
                .size_range(80.0..=f32::INFINITY)
                .show(ui, |ui| {
                    ui.strong("LIBRARY");
                    egui::ScrollArea::vertical().id_salt("library scroll").auto_shrink([false, false]).show(ui, |ui| {
                        // The whole visible area answers, before the tree
                        // draws: the files and folders lie on top of it, so
                        // every place that is not one of them is free space.
                        let bg = ui.interact(ui.clip_rect(), Id::new("library free space"), egui::Sense::click());
                        if !library_set {
                            ui.weak("No library folder yet.");
                            ui.add_space(4.0);
                            ui.weak("This is your library of tilesheets and packs. I'll help you browse and search them, and to transfer what you need into your own tilesheets. I'll track details about your assets in a tilepicky.json.");
                            ui.add_space(6.0);
                            ui.weak("Click here to choose it.");
                            if bg.clicked() {
                                ask = Some(Panel::Library);
                            }
                        }
                        let view = tree::View {
                            visible: self.library_visible.as_deref(),
                            selected: self.library_sel,
                            marked: None,
                            query: &self.qwords,
                            apply_query: self.open_trees,
                            menus: false,
                            scroll_to: self.library_scroll,
                            sweeping: false,
                            lifting: false,
                        };
                        library_action = self.library_tree.show(ui, &view, "", &mut Vec::new(), &mut library_order, &mut None);
                        bg.context_menu(|ui| {
                            let label = if library_set { "Change library folder…" } else { "Set library folder…" };
                            if ui.button(label).clicked() {
                                ask = Some(Panel::Library);
                                ui.close();
                            }
                            if library_set && ui.button("Refresh").clicked() {
                                library_action = Some(TreeAction::Refresh);
                                ui.close();
                            }
                        });
                    });
                });
            egui::CentralPanel::default().show(ui, |ui| {
                let heading = ui.add(egui::Label::new(egui::RichText::new("PROJECT").strong()).sense(egui::Sense::click()));
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
                if project_set {
                ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("New").clicked() {
                        create = true;
                    }
                    let field = egui::TextEdit::singleline(&mut self.new_name).hint_text("new tilesheet name").desired_width(ui.available_width());
                    let r = ui.add(field);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        create = true;
                    }
                });
                }
                egui::ScrollArea::vertical().id_salt("project scroll").auto_shrink([false, false]).show(ui, |ui| {
                    // The free space around the tree offers the folder menu.
                    let bg = ui.interact(ui.clip_rect(), Id::new("project free space"), egui::Sense::click());
                    if !project_set {
                        ui.weak("No project folder yet.");
                        ui.add_space(4.0);
                        ui.weak("Your tilesheets live here - I'll help you edit them and create new ones, tracking details in a tilepicky.json.");
                        ui.add_space(6.0);
                        ui.weak("Click here to choose it.");
                        if bg.clicked() {
                            ask = Some(Panel::Project);
                        }
                    }
                    let view = tree::View {
                        visible: self.project_visible.as_deref(),
                        selected: self.project_sel,
                        marked: Some(&self.marked),
                        query: &self.qwords,
                        apply_query: self.open_trees,
                        menus: true,
                        scroll_to: self.project_scroll,
                        sweeping: self.sweep.is_some(),
                        lifting: self.file_drag.is_some(),
                    };
                    // The tree area itself is the root folder; the tree names
                    // a folder inside it when the pointer is over one.
                    if self.file_drag.is_some() && bg.contains_pointer() {
                        hover_dir = Some(String::new());
                    }
                    project_action = self.project_tree.show(ui, &view, "", &mut Vec::new(), &mut project_order, &mut hover_dir);
                    bg.context_menu(|ui| {
                        let label = if project_set { "Change project folder…" } else { "Set project folder…" };
                        if ui.button(label).clicked() {
                            ask = Some(Panel::Project);
                            ui.close();
                        }
                        if project_set {
                            if ui.button("New folder…").clicked() {
                                self.prompt = Some(NamePrompt { title: "New folder".into(), value: String::new(), what: NameFor::NewFolder(String::new()), focus: true });
                                ui.close();
                            }
                            if ui.button("Refresh").clicked() {
                                project_action = Some(TreeAction::Refresh);
                                ui.close();
                            }
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
        self.library_scroll = None;
        self.project_scroll = None;
        self.tree_keys(ctx, &library_order, &project_order);
        match library_action {
            Some(TreeAction::Open(i)) => self.open_library(ctx, i),
            Some(TreeAction::Refresh) => self.rescan_library(),
            Some(TreeAction::Reveal(i)) => reveal(&file_path(&self.library.root, &self.library.entries[i].rel)),
            Some(TreeAction::CopyPath(i, whole)) => {
                let rel = self.library.entries[i].rel.clone();
                self.copy_path(ctx, &self.library.root.clone(), &rel, whole);
            }
            _ => {}
        }
        match project_action {
            Some(TreeAction::Open(i)) => {
                // The plainly clicked file is the start of any group.
                self.marked.clear();
                self.marked.insert(i);
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
                self.request(ctx, Pending::Open(i));
            }
            Some(TreeAction::Toggle(i)) => {
                if !self.marked.remove(&i) {
                    self.marked.insert(i);
                }
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
            }
            Some(TreeAction::Range(i, additive)) => {
                self.tree_cursor = Some(i);
                let a = self.tree_anchor.unwrap_or(i);
                self.mark_range(&project_order, a, i, additive);
            }
            Some(TreeAction::LiftFile(i)) => {
                let group = self.marked.len() > 1 && self.marked.contains(&i);
                self.file_drag = Some(if group {
                    let mut rels: Vec<String> = self.marked.iter().map(|&k| self.project.entries[k].rel.clone()).collect();
                    rels.sort();
                    rels
                } else {
                    self.marked.clear();
                    self.marked.insert(i);
                    vec![self.project.entries[i].rel.clone()]
                });
            }
            Some(TreeAction::SweepStart(i)) => {
                self.sweep = Some(i);
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
                self.marked.clear();
                self.marked.insert(i);
            }
            Some(TreeAction::Sweep(i)) => {
                let a = self.sweep.unwrap_or(i);
                self.tree_cursor = Some(i);
                self.mark_range(&project_order, a, i, false);
            }
            Some(TreeAction::DeleteFile(i)) => {
                let rel = self.project.entries[i].rel.clone();
                self.confirm = Some((format!("Delete {rel}? There is no undo."), vec![rel]));
            }
            Some(TreeAction::DeleteMarked) => {
                let rels: Vec<String> = self.marked.iter().map(|&i| self.project.entries[i].rel.clone()).collect();
                self.confirm = Some((format!("Delete {} files? There is no undo.", rels.len()), rels));
            }
            Some(TreeAction::Refresh) => self.rescan_project(),
            Some(TreeAction::Reveal(i)) => reveal(&file_path(&self.project.root, &self.project.entries[i].rel)),
            Some(TreeAction::CopyPath(i, whole)) => {
                let rel = self.project.entries[i].rel.clone();
                self.copy_path(ctx, &self.project.root.clone(), &rel, whole);
            }
            Some(TreeAction::DeleteFolder(dir)) => {
                self.confirm = Some((format!("Delete the folder {dir} and everything in it? There is no undo."), vec![dir]));
            }
            Some(TreeAction::RenameFile(i)) => {
                let rel = self.project.entries[i].rel.clone();
                self.prompt = Some(NamePrompt {
                    title: "Rename".into(),
                    value: rel.clone(),
                    what: NameFor::RenameFile(rel),
                    focus: true,
                });
            }
            Some(TreeAction::DuplicateFile(i)) => {
                let rel = self.project.entries[i].rel.clone();
                let suggestion = format!("{} copy", rel.trim_end_matches(".png"));
                self.prompt = Some(NamePrompt {
                    title: "Duplicate".into(),
                    value: suggestion,
                    what: NameFor::DuplicateFile(rel),
                    focus: true,
                });
            }
            Some(TreeAction::NewFolder(dir)) => {
                self.prompt = Some(NamePrompt {
                    title: "New folder".into(),
                    value: String::new(),
                    what: NameFor::NewFolder(dir),
                    focus: true,
                });
            }
            Some(TreeAction::RenameFolder(dir)) => {
                let name = dir.rsplit_once('/').map(|(_, n)| n).unwrap_or(&dir).to_string();
                self.prompt = Some(NamePrompt {
                    title: "Rename folder".into(),
                    value: name,
                    what: NameFor::RenameFolder(dir),
                    focus: true,
                });
            }
            None => {}
        }
        // The sweep ends with the button, after this frame's marks are in;
        // clearing it earlier would let the last step mark one file only.
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.sweep = None;
        }
        self.drop_files(ctx, hover_dir);
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
        let mut library_anim = Ok(false);
        let mut library_tile = None;
        let mut project_tile = None;
        let mut resized = false;
        // The split is kept as a fraction of the height, so that it stays in
        // place when the window changes size. The panel state is written from
        // it each frame and read back after the user drags the divider.
        let total = ui.available_height();
        let panel_id = Id::new("library panel");
        let rect = Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.available_width(), total * self.split));
        ctx.data_mut(|d| d.insert_persisted(panel_id, egui::PanelState { outer_rect: rect }));
        egui::Panel::top("library panel").resizable(true).show(ui, |ui| {
            let live = self.active == Panel::Library && self.library_sheet.is_some();
                library_tile = Self::sheet_header(ui, "LIBRARY", live, true, self.library_sheet.as_mut());
            if let Some(s) = &mut self.library_sheet {
                if s.show_anim_panel() {
                    egui::Panel::right("library animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        library_anim = Self::animation_panel(ui, s, true);
                    });
                }
                let ev = egui::CentralPanel::default().show(ui, |ui| s.view(ui, library_id(), dragging, false)).inner;
                if ev.interacted {
                    self.active = Panel::Library;
                }
                if let Some(grab) = ev.drag_block {
                    drag_from = Some((Panel::Library, grab));
                }
            } else {
                // Fill the panel, so that it keeps its height and can be dragged.
                egui::CentralPanel::default().show(ui, |ui| {
                    let hint = if library_set {
                        "Open a sheet on the left, or type a search."
                    } else {
                        "Click to open your asset library."
                    };
                    let r = ui.interact(ui.max_rect(), Id::new("library empty"), egui::Sense::click());
                    ui.weak(hint);
                    if !library_set && r.clicked() {
                        ask = Some(Panel::Library);
                    }
                });
            }
        });
        if let Some(state) = egui::PanelState::load(ctx, panel_id) {
            if total > 0.0 {
                self.split = (state.outer_rect.height() / total).clamp(0.1, 0.9);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.project_rect = ui.max_rect();
            let live = self.active == Panel::Project && self.project_sheet.is_some();
            project_tile = Self::sheet_header(ui, "PROJECT", live, false, self.project_sheet.as_mut());
            if let Some(s) = &mut self.project_sheet {
                if s.show_anim_panel() {
                    egui::Panel::right("my animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        anim_changed = Self::animation_panel(ui, s, false);
                    });
                }
                let ev = egui::CentralPanel::default().show(ui, |ui| s.view(ui, project_id(), dragging, true)).inner;
                if ev.interacted {
                    self.active = Panel::Project;
                }
                if ev.resized {
                    resized = true;
                }
                if let Some(grab) = ev.drag_block {
                    drag_from = Some((Panel::Project, grab));
                }
                if ev.delete {
                    s.clear_selection(ctx);
                    delete_in_mine = true;
                }
            } else {
                // The same frame as the library pane, so both hints sit alike.
                egui::CentralPanel::default().show(ui, |ui| {
                    let hint = if project_set {
                        "Create or open a tilesheet on the left. Then select cells in the library, Ctrl+C, click a cell here, Ctrl+V."
                    } else {
                        "Click to open your project folder."
                    };
                    let r = ui.interact(ui.max_rect(), Id::new("project empty"), egui::Sense::click());
                    ui.weak(hint);
                    if !project_set && r.clicked() {
                        ask = Some(Panel::Project);
                    }
                });
            }
        });
        match anim_changed {
            Ok(true) => self.after_animation_edit(Panel::Project),
            Ok(false) => {}
            Err(e) => self.status = e,
        }
        if let Some(panel) = ask {
            self.ask_folder(panel);
        }
        self.poll_folder(ctx);
        if let Some(g) = library_tile {
            self.change_grid(ctx, Panel::Library, g);
        }
        if let Some(g) = project_tile {
            self.change_grid(ctx, Panel::Project, g);
        }
        if delete_in_mine {
            self.after_edit();
        }
        if resized {
            if let Some(s) = &self.project_sheet {
                self.status = format!("resized to {}x{} cells", s.cols(), s.rows());
            }
            self.after_edit();
        }
        match library_anim {
            Ok(true) => self.after_animation_edit(Panel::Library),
            Ok(false) => {}
            Err(e) => self.status = e,
        }
        if let Some((from, grab)) = drag_from {
            self.start_drag(ctx, from, grab);
        }
        self.update_drag(ctx);
    }
}

/// Moves the old `name.json` files next to tilesheets into the book, once.
fn migrate_sidecars(project: &mut Index) {
    for e in &mut project.entries {
        let old = project.root.join(&e.rel).with_extension("json");
        if !e.side.is_empty() || !old.exists() {
            continue;
        }
        let Some(side) = std::fs::read_to_string(&old)
            .ok()
            .and_then(|s| serde_json::from_str::<sidecar::Sidecar>(&s).ok())
        else {
            continue;
        };
        if sidecar::store_entry(&project.root, &e.rel, &side).is_ok() {
            let _ = std::fs::remove_file(&old);
            e.side = side;
        }
    }
}

/// Height steps asked for by the wheel over a field this frame. Scrolling
/// down grows the number.
/// A small sign in the corner of the dragged block that tells what the drop
/// will do: a plus for a copy, two arrows for a swap.
fn drop_sign(painter: &egui::Painter, at: Pos2, copy: bool) {
    const SIDE: f32 = 18.0;
    let r = Rect::from_min_size(at, Vec2::splat(SIDE));
    painter.rect_filled(r, 3.0, Color32::from_black_alpha(200));
    painter.rect_stroke(r, 3.0, egui::Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
    let stroke = egui::Stroke::new(1.6, Color32::WHITE);
    let c = r.center();
    if copy {
        let arm = SIDE * 0.28;
        painter.line_segment([c - Vec2::new(arm, 0.0), c + Vec2::new(arm, 0.0)], stroke);
        painter.line_segment([c - Vec2::new(0.0, arm), c + Vec2::new(0.0, arm)], stroke);
        return;
    }
    // Two arrows, one over the other, pointing opposite ways.
    let (half, gap, head) = (SIDE * 0.28, SIDE * 0.16, SIDE * 0.13);
    for (dy, dir) in [(-gap, 1.0), (gap, -1.0)] {
        let y = c.y + dy;
        let (a, b) = (Pos2::new(c.x - half * dir, y), Pos2::new(c.x + half * dir, y));
        painter.line_segment([a, b], stroke);
        painter.line_segment([b, b + Vec2::new(-head * dir, -head)], stroke);
        painter.line_segment([b, b + Vec2::new(-head * dir, head)], stroke);
    }
}

/// Shows the file in the desktop's file manager, selected where the manager
/// can do that. The call waits on a thread: a manager that must start first
/// can take seconds to answer.
fn reveal(path: &Path) {
    let uri = file_uri(path);
    let dir = path.parent().unwrap_or(path).to_path_buf();
    std::thread::spawn(move || {
        let shown = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("['{uri}']"),
                "",
            ])
            .status()
            .is_ok_and(|s| s.success());
        // No such manager on the bus: open the folder and let the user look.
        if !shown {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    });
}

/// The whole path of a file in a tree. A root given on the command line may
/// be relative, and a URI or a file manager needs the whole path.
fn file_path(root: &Path, rel: &str) -> PathBuf {
    let p = root.join(rel);
    if p.is_absolute() {
        return p;
    }
    std::env::current_dir().map_or(p.clone(), |d| d.join(&p))
}

/// The whole path, with the home directory written as `~`.
fn home_path(abs: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|h| abs.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => abs.display().to_string(),
    }
}

/// The path as a person would type it in the directory tilepicky runs in.
/// A file somewhere else keeps its whole path.
fn near_path(abs: &Path) -> String {
    let here = std::env::current_dir().ok().and_then(|d| abs.strip_prefix(d).ok().map(Path::to_path_buf));
    match here {
        Some(rest) => rest.display().to_string(),
        None => home_path(abs),
    }
}

/// A `file://` URI for a local path. Every byte that is not plain becomes a
/// percent escape, so a space or an umlaut in a name cannot break the call.
fn file_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.as_os_str().as_encoded_bytes() {
        match b {
            b'/' | b'-' | b'_' | b'.' | b'~' => s.push(*b as char),
            b if b.is_ascii_alphanumeric() => s.push(*b as char),
            b => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// The file `dir` steps away from `from` in the order the tree shows them.
/// It stops at the ends, and starts at the first or the last file when the
/// tree has no cursor yet.
fn walk(order: &[usize], from: Option<usize>, dir: i32) -> Option<usize> {
    if order.is_empty() || dir == 0 {
        return None;
    }
    let at = from.and_then(|c| order.iter().position(|&x| x == c));
    let next = match at {
        Some(p) => (p as i32 + dir).clamp(0, order.len() as i32 - 1) as usize,
        None if dir > 0 => 0,
        None => order.len() - 1,
    };
    Some(order[next])
}

/// What a pair field keeps between frames: the drag distance collected since
/// the last step, and the text while the user types.
#[derive(Clone, Default)]
struct PairEdit {
    acc: f32,
    text: Option<String>,
    focus: bool,
}

/// A field that holds one number or two. A horizontal drag steps the first
/// number, the wheel steps the second, and a click turns the field into a
/// text box. Each step applies at once, so the drawing follows the pointer.
struct PairField<T = u32> {
    /// The same id in every frame; it holds the drag state.
    id: egui::Id,
    /// Pixels of drag that make one step.
    px_per_step: f32,
    /// What follows the numbers on the button.
    unit: &'static str,
    hover: &'static str,
    /// Moves one number by whole steps, inside its own limits.
    step: fn(T, i32) -> T,
    /// Writes the pair; one number when the field collapses it.
    show: fn([T; 2]) -> String,
    parse: fn(&str) -> Option<[T; 2]>,
    /// The drag moves both numbers when this holds.
    linked: fn([T; 2]) -> bool,
}

impl<T: Copy + PartialEq> PairField<T> {
    /// Draws the field. Returns a new value at each step of a drag, at each
    /// wheel step, and when the user accepts a typed value.
    fn ui(&self, ui: &mut egui::Ui, value: [T; 2]) -> Option<[T; 2]> {
        let mut edit: PairEdit = ui.data_mut(|d| d.get_temp(self.id)).unwrap_or_default();
        let out = self.field(ui, value, &mut edit);
        ui.data_mut(|d| d.insert_temp(self.id, edit));
        out
    }

    fn field(&self, ui: &mut egui::Ui, value: [T; 2], edit: &mut PairEdit) -> Option<[T; 2]> {
        if let Some(buf) = &mut edit.text {
            let r = ui.add(egui::TextEdit::singleline(buf).desired_width(56.0));
            if edit.focus {
                r.request_focus();
                edit.focus = false;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                edit.text = None;
                return None;
            }
            if r.lost_focus() {
                let v = (self.parse)(buf);
                edit.text = None;
                return v.filter(|n| *n != value);
            }
            return None;
        }
        let r = ui.add(
            egui::Button::new(format!("{}{}", (self.show)(value), self.unit))
                .small()
                .sense(egui::Sense::click_and_drag()),
        );
        let r = r.on_hover_text(self.hover);
        let mut new = value;
        if r.hovered() {
            let wheel = field_wheel(ui);
            if wheel != 0 {
                new[1] = (self.step)(new[1], wheel);
            }
        }
        if r.dragged() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            edit.acc += r.drag_delta().x;
            let steps = (edit.acc / self.px_per_step).trunc() as i32;
            edit.acc -= steps as f32 * self.px_per_step;
            if steps != 0 {
                // One number moves as one; two numbers move only the first.
                let both = (self.linked)(new);
                new[0] = (self.step)(new[0], steps);
                if both {
                    new[1] = new[0];
                }
            }
        } else {
            edit.acc = 0.0;
        }
        if r.clicked() {
            edit.text = Some((self.show)(value));
            edit.focus = true;
        }
        (new != value).then_some(new)
    }
}

/// The tile size: the usual sizes, and both numbers move while it is square.
fn tile_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("tile field", library)),
        px_per_step: 20.0,
        unit: " px",
        hover: "drag: step the size (only the width when not square)   scroll: the height   click: type, 48 or 32x48",
        step: |from, steps| {
            let last = TILE_SIZES.len() as i32 - 1;
            let idx = TILE_SIZES.iter().position(|&t| t >= from).unwrap_or(TILE_SIZES.len() - 1) as i32;
            TILE_SIZES[(idx + steps).clamp(0, last) as usize]
        },
        show: show_tile,
        parse: parse_tile,
        linked: |v| v[0] == v[1],
    }
}

/// The pixels between neighbouring tiles: single steps.
fn gap_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("gap field", library)),
        px_per_step: 12.0,
        unit: " px",
        hover: "drag: step the gap (only x when they differ)   scroll: the y gap   click: type, 1 or 1x2",
        step: |from, steps| (from as i32 + steps).clamp(0, 64) as u32,
        show: show_tile,
        parse: parse_px,
        linked: |v| v[0] == v[1],
    }
}

/// The pixels before the first tile: single steps, below zero as well. The
/// sheet stops a drag one pitch before its edge.
fn offset_field(library: bool) -> PairField<i32> {
    PairField {
        id: egui::Id::new(("offset field", library)),
        px_per_step: 12.0,
        unit: " px",
        hover: "drag: step the offset (only x when they differ)   scroll: the y offset   click: type, 4, 4x8, or -3",
        step: |from, steps| (from + steps).clamp(-1024, 64),
        show: show_tile,
        parse: parse_offset,
        linked: |v| v[0] == v[1],
    }
}

/// The frame grid of an animation: frames in a row, and rows. One row is the
/// usual case, so the rows never follow the drag.
fn frames_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("frames field", library)),
        px_per_step: 12.0,
        unit: "",
        hover: "drag: frames in a row   scroll: the number of rows   click: type, 6 or 4x2",
        step: |from, steps| (from as i32 + steps).clamp(1, 256) as u32,
        show: show_frames,
        parse: parse_frames,
        linked: |_| false,
    }
}

fn field_wheel(ui: &egui::Ui) -> i32 {
    // f32::signum maps 0.0 to +1, so a plain three-way sign it is.
    let sig = |v: f32| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let mut steps = 0;
    ui.input(|i| {
        for e in &i.events {
            if let egui::Event::MouseWheel { delta, modifiers, .. } = e {
                if !modifiers.ctrl {
                    steps -= sig(delta.y);
                }
            }
        }
    });
    steps
}

/// "4" or "4x8", in pixels, zero allowed. For gaps.
fn parse_px(text: &str) -> Option<[u32; 2]> {
    let text = text.trim().trim_end_matches("px").trim();
    let ok = |n: u32| n <= 1024;
    if let Some((x, y)) = text.split_once(['x', 'X']) {
        let (x, y) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
        (ok(x) && ok(y)).then_some([x, y])
    } else {
        let n = text.parse().ok()?;
        ok(n).then_some([n, n])
    }
}

/// "4", "4x8", or "-3", in pixels. For offsets, which may be negative.
fn parse_offset(text: &str) -> Option<[i32; 2]> {
    let text = text.trim().trim_end_matches("px").trim();
    let ok = |n: i32| (-1024..=1024).contains(&n);
    if let Some((x, y)) = text.split_once(['x', 'X']) {
        let (x, y) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
        (ok(x) && ok(y)).then_some([x, y])
    } else {
        let n = text.parse().ok()?;
        ok(n).then_some([n, n])
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
fn show_tile<T: std::fmt::Display + PartialEq>(t: [T; 2]) -> String {
    if t[0] == t[1] { format!("{}", t[0]) } else { format!("{}x{}", t[0], t[1]) }
}

/// "6" for a single row of frames, "4x2" for a block of them.
fn show_frames(f: [u32; 2]) -> String {
    if f[1] == 1 { format!("{}", f[0]) } else { format!("{}x{}", f[0], f[1]) }
}

/// "6" is six frames in one row; "4x2" is four in a row and two rows.
fn parse_frames(text: &str) -> Option<[u32; 2]> {
    let text = text.trim();
    let ok = |n: u32| (1..=1024).contains(&n);
    if let Some((c, r)) = text.split_once(['x', 'X']) {
        let (c, r) = (c.trim().parse().ok()?, r.trim().parse().ok()?);
        (ok(c) && ok(r)).then_some([c, r])
    } else {
        let n = text.parse().ok()?;
        ok(n).then_some([n, 1])
    }
}

fn main() -> eframe::Result {
    let mut dirs: Vec<String> = Vec::new();
    for a in std::env::args().skip(1) {
        if a == "--help" || a == "-h" {
            println!("usage: tilepicky [<library dir> [<project dir>]]");
            println!("Without a folder, the tool asks for one and remembers it.");
            return Ok(());
        }
        dirs.push(a);
    }
    if dirs.len() > 2 {
        eprintln!("usage: tilepicky [<library dir> [<project dir>]]");
        std::process::exit(2);
    }
    let mut settings = settings::Settings::load();
    // A folder named on the command line wins for this run, and is what the
    // tool offers next time.
    if let Some(d) = dirs.first() {
        settings.library.path = Some(PathBuf::from(d));
    }
    if let Some(d) = dirs.get(1) {
        let project = PathBuf::from(d);
        if let Err(e) = std::fs::create_dir_all(&project) {
            eprintln!("cannot create {}: {e}", project.display());
            std::process::exit(1);
        }
        settings.project.path = Some(project);
    }
    // A folder given here is what the tool offers next time, so it is
    // written before the window opens.
    settings.save();
    let icon = image::load_from_memory(include_bytes!("../icon.png")).expect("icon.png").to_rgba8();
    let icon = egui::IconData {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_title("Tilepicky")
            .with_icon(icon)
            .with_app_id("tilepicky"),
        ..Default::default()
    };
    eframe::run_native(
        "tilepicky",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(App::new(settings)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_escapes_what_a_name_may_hold() {
        let p = Path::new("/home/x/my tiles/yo+/a.png");
        assert_eq!(file_uri(p), "file:///home/x/my%20tiles/yo%2B/a.png");
    }

    #[test]
    fn the_home_directory_is_a_squiggle() {
        // SAFETY: the test runs alone in this process.
        unsafe { std::env::set_var("HOME", "/home/x") };
        assert_eq!(home_path(Path::new("/home/x/work/a.png")), "~/work/a.png");
        assert_eq!(home_path(Path::new("/opt/a.png")), "/opt/a.png");
    }
}
