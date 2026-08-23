//! Tilepick: browse a large set of sheets, search them, and copy
//! cells into tilemaps of your own.
//!
//! Usage: `tilepick <source dir> <destination dir>`

mod index;
mod sheet;
mod sidecar;
mod tree;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, TextureHandle, Vec2};
use index::{Derived, Index, Names, Progress};
use sheet::{Block, Sel, Sheet};
use sidecar::Animation;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use tree::Node;

const NEW_COLS: u32 = 16;
const NEW_ROWS: u32 = 16;

#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Source,
    Mine,
}

/// A block on its way from one place to another, under the pointer.
struct Drag {
    block: Block,
    from: Panel,
    /// The selection the block was lifted from.
    origin: Sel,
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

struct App {
    drag: Option<Drag>,
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
    indexing: Option<(Arc<Progress>, Receiver<Derived>)>,
    status: String,
    /// Set when the query changes, so that the trees expand once to show the matches.
    open_trees: bool,
    /// Frames drawn so far; the split is set during the first few, once the window has its size.
    frames: u32,
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
    fn new(src_root: PathBuf, dst_root: PathBuf) -> Self {
        let src = Index::scan(&src_root);
        let mut dst = Index::scan(&dst_root);
        migrate_sidecars(&mut dst);

        let progress = Arc::new(Progress { done: 0.into(), total: 0.into() });
        let (tx, rx) = std::sync::mpsc::channel();
        let rels: Vec<String> = src.entries.iter().map(|e| e.rel.clone()).collect();
        let cache = index::cache_path(&src_root);
        let p = progress.clone();
        let root = src_root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(index::derive(root, rels, cache, p));
        });

        Self {
            drag: None,
            pending: None,
            src_tree: Node::build(&src.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>()),
            dst_tree: Node::build(&dst.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>()),
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
            indexing: Some((progress, rx)),
            open_trees: false,
            frames: 0,
            shown_status: String::new(),
            status_at: std::time::Instant::now(),
        }
    }

    fn poll_index(&mut self, ctx: &egui::Context) {
        let Some((progress, rx)) = &self.indexing else { return };
        match rx.try_recv() {
            Ok(derived) => {
                for e in &mut self.src.entries {
                    if let Some(names) = derived.get(&e.rel) {
                        e.names = names.clone();
                    }
                }
                self.status = format!("{} files, {} sheets with named cells", self.src.entries.len(), derived.len());
                self.indexing = None;
                if let (Some(sheet), Some(i)) = (&mut self.src_sheet, self.src_sel) {
                    sheet.names = self.src.entries[i].names.clone();
                }
                self.refresh_query();
            }
            Err(_) => {
                let (done, total) = (progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                self.status = format!("naming cells: {done}/{total} sheets");
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
    }

    fn refresh_query(&mut self) {
        self.qwords = index::query_words(&self.query);
        self.open_trees = !self.qwords.is_empty();
        self.src_visible = self.src.visible(&self.qwords);
        self.dst_visible = self.dst.visible(&self.qwords);
        if let Some(s) = &mut self.src_sheet {
            s.compute_hits(&self.qwords);
        }
        if let Some(s) = &mut self.dst_sheet {
            s.compute_hits(&self.qwords);
        }
    }

    fn open_source(&mut self, ctx: &egui::Context, i: usize) {
        let e = &self.src.entries[i];
        match Sheet::open(ctx, &self.src.root, &e.rel, e.side.clone(), e.names.clone(), e.words.clone()) {
            Ok(mut s) => {
                s.compute_hits(&self.qwords);
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
        match Sheet::open(ctx, &self.dst.root, &e.rel, e.side.clone(), Names::new(), e.words.clone()) {
            Ok(mut s) => {
                s.compute_hits(&self.qwords);
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
        let mut sheet = Sheet::new_empty(ctx, &self.dst.root, &rel, NEW_COLS, NEW_ROWS);
        if let Err(e) = sheet.save() {
            self.status = e;
            return;
        }
        self.new_name.clear();
        self.dst = Index::scan(&self.dst.root);
        self.dst_tree = Node::build(&self.dst.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>());
        if let Some(i) = self.dst.position(&rel) {
            self.open_mine(ctx, i);
        }
    }

    /// Keeps search in step with an edit. Saving is explicit: Ctrl+S.
    fn after_edit(&mut self) {
        let Some(sheet) = &mut self.dst_sheet else { return };
        sheet.compute_hits(&self.qwords);
        if let Some(i) = self.dst_sel {
            self.dst.entries[i].side = sheet.side.clone();
        }
        self.dst_visible = self.dst.visible(&self.qwords);
    }

    fn save(&mut self) {
        if let Some(sheet) = &mut self.dst_sheet {
            match sheet.save() {
                Ok(()) => self.status = format!("saved {}", sheet.rel),
                Err(e) => self.status = e,
            }
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
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| id == src_id() || id == dst_id()) {
            return;
        }
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let cmd = Modifiers::COMMAND;
        // The window layer turns Ctrl+C into a Copy event, and Ctrl+V into a Paste
        // event that only exists when the system clipboard holds text.
        let (copy, paste) = ctx.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let paste = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
            (copy, paste)
        });

        if copy || key(cmd, Key::C) {
            let from = match self.active {
                Panel::Source => &self.src_sheet,
                Panel::Mine => &self.dst_sheet,
            };
            if let Some(b) = from.as_ref().and_then(Sheet::copy) {
                self.status = format!("copied {}x{} cells", b.cols, b.rows);
                // A note in the system clipboard, so that Ctrl+V reaches us as a Paste event.
                ctx.copy_text(b.note());
                self.clip = Some(b);
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
        if key(cmd, Key::S) {
            self.save();
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

    fn start_drag(&mut self, ctx: &egui::Context, from: Panel, grab: (u32, u32)) {
        let sheet = match from {
            Panel::Source => self.src_sheet.as_ref(),
            Panel::Mine => self.dst_sheet.as_ref(),
        };
        let Some((block, origin)) = sheet.and_then(|s| Some((s.copy()?, s.sel.clone()))) else { return };
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [block.img.width() as usize, block.img.height() as usize],
            block.img.as_raw(),
        );
        let ghost = ctx.load_texture("drag ghost", image, egui::TextureOptions::NEAREST);
        self.drag = Some(Drag { block, from, origin, grab, ghost });
        self.active = from;
    }

    /// Draws the ghost under the pointer, and drops the block on release.
    fn update_drag(&mut self, ctx: &egui::Context) {
        let Some(drag) = &self.drag else { return };
        let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

        // Over the tilemap the ghost snaps to the grid; elsewhere it floats at the pointer.
        let target = self.dst_sheet.as_ref().and_then(|d| {
            let c = d.cell_at(p)?;
            Some((c.0.saturating_sub(drag.grab.0), c.1.saturating_sub(drag.grab.1)))
        });
        let (min, cell_px) = match (target, &self.dst_sheet) {
            (Some(t), Some(d)) => (d.screen.min + Vec2::new(t.0 as f32, t.1 as f32) * d.cell_px(), d.cell_px()),
            _ => {
                let z = match drag.from {
                    Panel::Source => self.src_sheet.as_ref().map_or(2.0, |s| s.cell_px()),
                    Panel::Mine => self.dst_sheet.as_ref().map_or(2.0, |s| s.cell_px()),
                };
                (p - Vec2::new(drag.grab.0 as f32 + 0.5, drag.grab.1 as f32 + 0.5) * z, z)
            }
        };
        let size = Vec2::new(drag.block.cols as f32, drag.block.rows as f32) * cell_px;
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
        let (Some(at), Some(sheet)) = (target, &mut self.dst_sheet) else { return };
        let copy = drag.from == Panel::Source || ctx.input(|i| i.modifiers.command);
        if copy {
            sheet.paste(ctx, at, &drag.block);
        } else if Some(at) != drag.origin.origin() {
            sheet.move_block(ctx, &drag.origin, at, &drag.block);
        } else {
            return;
        }
        self.active = Panel::Mine;
        self.after_edit();
    }

    fn sheet_header(ui: &mut egui::Ui, title: &str, active: bool, sheet: Option<&Sheet>) {
        ui.horizontal(|ui| {
            let color = if active { egui::Color32::from_rgb(80, 160, 255) } else { ui.visuals().weak_text_color() };
            ui.colored_label(color, egui::RichText::new(title).strong());
            let Some(s) = sheet else {
                ui.weak("nothing open");
                return;
            };
            ui.label(if s.dirty { format!("{} *", s.rel) } else { s.rel.clone() });
            ui.weak(format!("{}x{} cells", s.cols(), s.rows()));
            ui.weak(format!("{}x", s.zoom.level));
            if let Some(b) = s.sel.bounds() {
                ui.weak(format!("sel {} cells, {}x{} at {},{}", s.sel.len(), b.cols(), b.rows(), b.x0, b.y0));
            }
            if let Some((x, y)) = s.hover {
                let tags = s.cell_tags(x, y).join(" ");
                let from = s.side.get(x, y).and_then(|c| c.src.as_ref()).map(|p| format!(" <- {p}")).unwrap_or_default();
                ui.weak(format!("cell {x},{y}: {tags}{from}"));
            }
        });
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
        // The status line: zoom, and what the fields describe.
        ui.horizontal(|ui| {
            ui.weak(format!("{}x", sheet.preview_zoom.level));
            match &stored {
                Some(a) => ui.weak(format!("stored: {} frames of {}x{}", a.frames, a.w, a.h)),
                None if b.cols() % frames != 0 => {
                    ui.colored_label(egui::Color32::from_rgb(200, 60, 40), format!("{frames} does not divide {} columns", b.cols()))
                }
                None => ui.weak(format!("draft: {frames} frames of {}x{}", b.cols() / frames, b.rows())),
            };
        });
        let width = stored.as_ref().map_or(b.cols(), |a| a.w * a.frames);
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
        let anim = stored.clone().or_else(|| {
            let d = sheet.draft()?;
            (d.area.cols() % d.frames == 0).then(|| d.animation())
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
        let cell = 32.0 * sheet.preview_zoom.level;
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(a.w as f32, a.h as f32) * cell, egui::Sense::hover());
        sheet.preview_hovered = resp.hovered();
        if resp.hovered() {
            sheet.preview_zoom.wheel(ui);
        }
        ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(225));
        for cy in 0..a.h {
            for cx in 0..a.w {
                if let Some((tex, uv)) = sheet.cell_uv(a.x + frame * a.w + cx, a.y + cy) {
                    let r = Rect::from_min_size(rect.min + Vec2::new(cx as f32, cy as f32) * cell, Vec2::splat(cell));
                    ui.painter().image(tex, r, uv, egui::Color32::WHITE);
                }
            }
        }
        ui.weak(format!("frame {}/{}", frame + 1, a.frames));
        ui.ctx().request_repaint_after(Duration::from_millis(a.ms.max(16) as u64));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.poll_index(ctx);
        self.handle_keys(ctx);
        // The preview sets this flag while drawing; clear it first, so that
        // a closed preview does not keep it.
        for s in [&mut self.src_sheet, &mut self.dst_sheet].into_iter().flatten() {
            s.preview_hovered = false;
        }

        let mut open_src = None;
        let mut open_dst = None;
        let mut create = false;
        // The status line, across the whole window, text at the right. It
        // disappears ten seconds after it last changed.
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
        egui::Panel::left("left").resizable(true).default_size(340.0).size_range(240.0..=800.0).show(ui, |ui| {
            ui.add_space(4.0);
            let search = egui::TextEdit::singleline(&mut self.query).hint_text("search: rock wall").desired_width(f32::INFINITY);
            if ui.add(search).changed() {
                self.refresh_query();
            }
            ui.add_space(4.0);
            egui::Panel::top("source tree")
                .resizable(true)
                .default_size(ui.available_height() * 0.6)
                .show(ui, |ui| {
                    ui.strong("SOURCE");
                    egui::ScrollArea::vertical().id_salt("source scroll").auto_shrink([false, false]).show(ui, |ui| {
                        open_src = self.src_tree.show(ui, self.src_visible.as_deref(), self.src_sel, self.open_trees, "src");
                    });
                });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.strong("MY TILEMAPS");
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
                egui::Panel::bottom("legend").show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    ui.weak("click / drag: select   ctrl+a: select all   shift+click: rectangle from the last click   ctrl+click: add or remove a cell   ctrl+shift+click: add a rectangle   right click: clear   drag a selection: move (ctrl: copy)   ctrl+c / ctrl+v   delete   a: animation panel / store   ctrl+z   ctrl+s: save   ctrl+t: trim empty edges   drag the canvas edge: resize   ctrl+wheel or + / -: zoom the view under the pointer");
                });
                egui::ScrollArea::vertical().id_salt("mine scroll").auto_shrink([false, false]).show(ui, |ui| {
                    open_dst = self.dst_tree.show(ui, self.dst_visible.as_deref(), self.dst_sel, self.open_trees, "dst");
                });
            });
        });
        self.open_trees = false;
        if let Some(i) = open_src {
            self.open_source(ctx, i);
        }
        if let Some(i) = open_dst {
            self.request(ctx, Pending::Open(i));
        }
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
        let mut resized = false;
        // The window gets its final size after the first frames; put the
        // split in the middle once that is known.
        if self.frames < 5 {
            self.frames += 1;
            let h = ui.available_height() * 0.5;
            let rect = Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.available_width(), h));
            ctx.data_mut(|d| d.insert_persisted(Id::new("source panel"), egui::PanelState { outer_rect: rect }));
        }
        egui::Panel::top("source panel")
            .resizable(true)
            .show(ui, |ui| {
                Self::sheet_header(ui, "SOURCE", self.active == Panel::Source, self.src_sheet.as_ref());
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
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            Self::sheet_header(ui, "MINE", self.active == Panel::Mine, self.dst_sheet.as_ref());
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
            } else {
                ui.weak("Create or open a tilemap on the left. Then select cells in the source, Ctrl+C, click a cell here, Ctrl+V.");
            }
        });
        match anim_changed {
            Ok(true) => self.after_animation_edit(Panel::Mine),
            Ok(false) => {}
            Err(e) => self.status = e,
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

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: tilepick <source dir> <destination dir>");
        std::process::exit(2);
    }
    let src = PathBuf::from(&args[1]);
    let dst = PathBuf::from(&args[2]);
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
            Ok(Box::new(App::new(src, dst)))
        }),
    )
}
