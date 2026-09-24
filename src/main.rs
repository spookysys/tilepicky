// SPDX-License-Identifier: GPL-3.0-only
//! Tilepicky: browse a large set of sheets, search them, and copy
//! cells into tilesheets of your own.
//!
//! Usage: `tilepicky [<library dir> [<project dir>]]`

// Windows opens a console beside a program that asks for one, and this one
// draws its own window. A debug build keeps the console, because that is
// where a panic and a `--help` go.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod detect;
mod dialogs;
mod index;
mod labels;
mod batch;
mod storage;
mod files;
mod half;
mod settings;
mod sheet;
mod sidecar;
mod tree;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, TextureHandle, Vec2};

/// The per-sheet labeling panel and local AI settings are available.
const AI_VISIBLE: bool = true;

/// A sheet's tile size, gap, and offset, as the header fields edit them.
type Grid = ([u32; 2], [u32; 2], [i32; 2]);
use index::Index;
use sheet::{Block, Sel, Sheet};
use sidecar::{Animation, Pair};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use dialogs::{Damaged, NameFor, NamePrompt, Pending};
use half::Half;
use tree::TreeAction;

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

struct App {
    /// Configuration files that could not be read, waiting for the user
    /// to agree that they are written over.
    damaged: Vec<Damaged>,
    drag: Option<Drag>,
    prompt: Option<NamePrompt>,
    /// Files marked with Ctrl+click in the PROJECT tree.
    marked: HashSet<usize>,
    /// The last plainly clicked file, for shift ranges.
    tree_anchor: Option<usize>,
    /// Where the arrow keys stand in the PROJECT tree; the moving end of a range.
    tree_cursor: Option<usize>,
    /// What the tool remembers between runs: the two folders and their
    /// tile sizes.
    settings: settings::Settings,
    /// The typed API keys, beside the settings.
    keys: ai::Keys,
    /// The AI assist panel of the library is open.
    ai_panel: bool,
    /// The Label with AI request in flight, and what the last one ended with.
    label_run: Option<labels::Run>,
    label_outcome: Option<String>,
    library_batch: batch::Panel,
    /// The sheet whose label waits for the user's yes to be removed.
    remove_label: Option<(PathBuf, String)>,
    /// The settings popup was open at the last frame; both files are
    /// written when it closes.
    config_open: bool,
    /// The legend asks whether to hide itself.
    legend_prompt: bool,
    /// The eye of the project panel: tooltips and islands, no editing. Off
    /// at each start; a thing to switch on for a moment.
    project_eye: bool,
    /// A folder dialog is open for this side; the answer arrives on the channel.
    picking: Option<(Panel, std::sync::mpsc::Receiver<Option<PathBuf>>)>,
    /// A drag across the files started here and marks a group while it lasts.
    sweep: Option<usize>,
    /// Files held in the air, waiting for a folder to land in.
    file_drag: Option<Vec<String>>,
    /// Where the project pane sat last frame, for drops onto the empty pane.
    project_rect: Rect,
    /// A pending deletion, waiting for the user's yes.
    confirm: Option<(String, Vec<String>)>,
    /// An action that waits for the save dialog.
    pending: Option<Pending>,
    library: Half,
    project: Half,
    query: String,
    qwords: Vec<String>,
    active: Panel,
    clip: Option<Block>,
    new_name: String,
    status: String,
    /// Set when the query changes, so that the trees expand once to show the matches.
    open_trees: bool,
    /// The pane of the last stop that held the keys. When nothing holds
    /// them, they go back to the body of this pane.
    pane: (Panel, Spot),
    /// The places that Tab stops at, in reading order, as the last frame
    /// drew them.
    stops: Vec<((Panel, Spot), Id)>,
    /// The Tab and arrow keys that `raw_input_hook` kept away from egui.
    /// They go back into the input at the start of the frame.
    nav: Vec<egui::Event>,

    /// Where the split between the library and the project sits, as a fraction of the height.
    split: f32,
    /// The status as last shown, and when it changed; it fades after a while.
    shown_status: String,
    status_at: std::time::Instant,
}

/// The panes of one half of the window.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Spot {
    Tree,
    Sheet,
    /// The side panel: the animation panel, or AI assist in the library.
    Side,
    /// The status bar along the foot of the window, with the gear on it. It
    /// belongs to neither half, so it rides with the project half and comes
    /// last, which is where it lies.
    Status,
}

/// The panes, one column of the window after the other, and in each column
/// the upper half before the lower half. Tab walks the stops in this order,
/// and Ctrl+Tab walks the panes. So the same pane in the other half is
/// always one step away: Ctrl+Tab goes down, Ctrl+Shift+Tab goes up.
const PANES: [(Panel, Spot); 7] = [
    (Panel::Library, Spot::Tree),
    (Panel::Project, Spot::Tree),
    (Panel::Library, Spot::Sheet),
    (Panel::Project, Spot::Sheet),
    (Panel::Library, Spot::Side),
    (Panel::Project, Spot::Side),
    (Panel::Project, Spot::Status),
];

/// Says in which pane the stops that draw next belong.
fn set_pane(ui: &egui::Ui, pane: (Panel, Spot)) {
    ui.data_mut(|d| d.insert_temp(Id::new("drawing pane"), pane));
}

/// Makes a widget a place that Tab stops at, in the pane that `set_pane`
/// named last. A widget that cannot take the focus now is left out: egui
/// drops the focus of a disabled widget at once.
fn stop(r: &egui::Response) {
    if !r.enabled() || !r.sense.is_focusable() {
        return;
    }
    stop_id(&r.ctx, r.id);
}

/// Makes a widget a stop, as `stop` does, and hands it back.
fn stopped(r: egui::Response) -> egui::Response {
    stop(&r);
    r
}

/// Makes the widget with this name a stop, as `stop` does. For a body that
/// draws deeper down, where the response is out of reach.
fn stop_id(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| {
        let pane = d.get_temp::<(Panel, Spot)>(Id::new("drawing pane")).unwrap_or((Panel::Library, Spot::Tree));
        d.get_temp_mut_or_default::<Vec<((Panel, Spot), Id)>>(Id::new("stops")).push((pane, id));
    });
}

/// The stop after `from` in the list, or before it when `step` is negative.
/// The walk wraps at both ends. A place that is not in the list starts the
/// walk at `home`.
fn next_stop(stops: &[Id], from: Option<Id>, home: Option<Id>, step: i32) -> Option<Id> {
    if stops.is_empty() {
        return None;
    }
    let n = stops.len() as i32;
    match from.and_then(|f| stops.iter().position(|s| *s == f)) {
        Some(i) => Some(stops[(i as i32 + step).rem_euclid(n) as usize]),
        None => home.filter(|h| stops.contains(h)).or(Some(stops[0])),
    }
}

/// What an arrow key does to the selection: grow it from a held corner,
/// step it, or walk the whole of it. `Ctrl` makes a step reach the next
/// block of filled cells.
#[derive(Clone, Copy, PartialEq)]
enum Move {
    Grow(bool),
    Step(bool),
    Whole,
}

/// Names a place for the trace below.
fn place_name(app: &App, id: Id) -> String {
    let named = [
        (library_tree_id(), "library tree"),
        (project_tree_id(), "project tree"),
        (library_id(), "library grid"),
        (project_id(), "project grid"),
        (search_id(), "search"),
        (new_name_id(), "new name"),
    ];
    let name = named.iter().find(|(k, _)| *k == id).map(|(_, n)| (*n).to_string());
    let pane = match app.pane_of(id) {
        Some((p, s)) => format!("{}/{s:?}", if p == Panel::Library { "library" } else { "project" }),
        None => "no pane".into(),
    };
    format!("{} [{pane}]", name.unwrap_or_else(|| format!("{id:?}")))
}

/// `TILEPICKY_KEYS=1` traces where the keys are and where they go.
fn trace(app: &App, ctx: &egui::Context, what: &str) {
    if std::env::var_os("TILEPICKY_KEYS").is_none() {
        return;
    }
    let here = match ctx.memory(|m| m.focused()) {
        Some(id) => place_name(app, id),
        None => "nothing".into(),
    };
    eprintln!("[keys] {what}: on {here}");
}

fn library_id() -> Id {
    Id::new("library sheet")
}
fn project_id() -> Id {
    Id::new("project sheet")
}
fn sheet_id(panel: Panel) -> Id {
    if panel == Panel::Library { library_id() } else { project_id() }
}

/// The keys of a popup that hangs from a button. While the popup is open,
/// the keys stay in it: when they are anywhere else, they go to `first`,
/// the first control of the popup. So they land there when the popup opens,
/// and a Tab past its last control comes back to the start. egui closes the
/// popup on Escape, and the keys then go back to the body of the pane.
fn popup_keys(ui: &egui::Ui, button: &egui::Response, first: &egui::Response) {
    let layer = ui.layer_id();
    let inside = |id: Id| id == button.id || ui.ctx().read_response(id).is_some_and(|r| r.layer_id == layer);
    if !ui.memory(|m| m.focused()).is_some_and(inside) {
        first.request_focus();
    }
}

/// A press anywhere in a tree gives its body the keys. The rows lie on top
/// of the body and take the click themselves, so the body never sees it.
fn claim_on_press(ui: &egui::Ui, id: Id) {
    if ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(ui.clip_rect()) {
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// The title of a pane. The colour says which pane holds the keys: deep
/// blue for it, faint grey for the rest.
fn title_text(title: &str, keys: bool) -> egui::RichText {
    let t = egui::RichText::new(format!(" {title} ")).strong();
    if keys { t.color(egui::Color32::from_rgb(20, 90, 190)) } else { t.color(egui::Color32::from_gray(180)) }
}

/// The search field, at the top of the left column.
fn search_id() -> Id {
    Id::new("search field")
}

/// The field that names a new tilesheet, above the PROJECT tree.
fn new_name_id() -> Id {
    Id::new("new tilesheet name")
}

/// The free space behind each file tree. It is a widget of its own, so it can
/// hold the keyboard focus: with it, the arrows walk the tree; with a sheet
/// focused, they move that sheet's selection.
fn library_tree_id() -> Id {
    Id::new("library free space")
}
fn project_tree_id() -> Id {
    Id::new("project free space")
}
fn tree_id(panel: Panel) -> Id {
    if panel == Panel::Library { library_tree_id() } else { project_tree_id() }
}
/// The half whose tree has this name.
fn tree_panel(id: Id) -> Option<Panel> {
    [Panel::Library, Panel::Project].into_iter().find(|p| tree_id(*p) == id)
}

impl App {
    /// `damaged` is why the settings file could not be read, if it could not.
    fn new(settings: settings::Settings, damaged: Option<String>) -> Self {
        let root = |s: &Option<PathBuf>| s.clone().unwrap_or_default();
        let library = Index::scan(&root(&settings.library.path), settings.library.tile.map_or(TILE, Pair::xy));
        let mut project = Index::scan(&root(&settings.project.path), settings.project.tile.map_or(TILE, Pair::xy));
        migrate_sidecars(&mut project);
        let (keys, damaged_keys) = ai::Keys::load();
        let damaged = damaged.map(Damaged::Settings).into_iter().chain(damaged_keys.map(Damaged::Keys)).collect();
        Self {
            damaged,
            settings,
            keys,
            ai_panel: false,
            label_run: None,
            label_outcome: None,
            library_batch: batch::Panel::default(),
            remove_label: None,
            config_open: false,
            legend_prompt: false,
            project_eye: false,
            picking: None,
            drag: None,
            prompt: None,
            marked: HashSet::new(),
            tree_anchor: None,
            tree_cursor: None,
            sweep: None,
            file_drag: None,
            project_rect: Rect::NOTHING,
            confirm: None,
            pending: None,
            status: library.error.clone().or_else(|| project.error.clone()).unwrap_or_default(),
            library: Half::new(library, true),
            project: Half::new(project, false),
            query: String::new(),
            qwords: Vec::new(),
            active: Panel::Library,
            pane: (Panel::Library, Spot::Tree),
            stops: Vec::new(),
            nav: Vec::new(),
            clip: None,
            new_name: String::new(),
            open_trees: false,
            split: 0.5,
            shown_status: String::new(),
            status_at: std::time::Instant::now(),
        }
    }

    fn half(&self, panel: Panel) -> &Half {
        match panel {
            Panel::Library => &self.library,
            Panel::Project => &self.project,
        }
    }

    fn half_mut(&mut self, panel: Panel) -> &mut Half {
        match panel {
            Panel::Library => &mut self.library,
            Panel::Project => &mut self.project,
        }
    }

    /// What the settings remember about one side.
    fn remembered(&mut self, panel: Panel) -> &mut settings::Side {
        match panel {
            Panel::Library => &mut self.settings.library,
            Panel::Project => &mut self.settings.project,
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
            // The dialog starts in the folder in use. The portal wants the
            // whole path; a relative one, as given on the command line, is
            // dropped by it.
            if let Some(dir) = at.and_then(|d| d.canonicalize().ok()).filter(|d| d.is_dir()) {
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
                    self.set_folder(panel, dir);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
        }
    }

    /// Points one side at a folder: reads it, drops what was open there, and
    /// remembers it for the next run.
    fn set_folder(&mut self, panel: Panel, dir: PathBuf) {
        let name = match panel {
            Panel::Library => "library",
            Panel::Project => {
                self.marked.clear();
                "project"
            }
        };
        self.remembered(panel).path = Some(dir.clone());
        let (query, search) = (self.qwords.clone(), self.settings.search);
        self.half_mut(panel).set_root(&dir, &query, search);
        self.status = self.half(panel).index.error.clone().unwrap_or_else(|| format!("{name}: {}", self.half(panel).index.root.display()));
        if let Err(e) = self.settings.save() { self.status = e; }
    }

    /// Remembers the tile size a side used last, in the folder's own book and
    /// in the settings, so that a new sheet there starts with it.
    fn remember_tile(&mut self, panel: Panel, tile: [u32; 2]) {
        if !self.half(panel).is_set() {
            return;
        }
        let root = self.half(panel).index.root.clone();
        if let Err(e) = sidecar::store_tile(&root, tile) { self.status = e; return; }
        self.half_mut(panel).index.tile = tile;
        self.remembered(panel).tile = Some(Pair::of(tile));
        if let Err(e) = self.settings.save() { self.status = e; }
    }

    fn refresh_query(&mut self) {
        self.qwords = index::words(&self.query);
        self.open_trees = true;
        self.refresh_visible();
    }

    /// Filters the trees again after the entries changed. The folders stay
    /// open or closed as they are.
    fn refresh_visible(&mut self) {
        self.library.refresh_visible(&self.qwords, self.settings.search);
        self.project.refresh_visible(&self.qwords, self.settings.search);
    }

    /// The arrow keys move a cursor over the folders and files of the tree
    /// that holds them. Nothing opens on the way: Enter, or Space, opens the
    /// file the cursor stands on, and Right and Left unfold and fold the
    /// folder it stands on. In the PROJECT tree, Shift and the arrows grow
    /// the marked group over the files.
    ///
    /// The keys go to a tree only while its body holds the focus.
    fn tree_keys(&mut self, ctx: &egui::Context, library_rows: &[tree::Row], project_rows: &[tree::Row], project_order: &[usize]) {
        let Some(panel) = ctx.memory(|m| m.focused()).and_then(tree_panel) else { return };
        let library = panel == Panel::Library;
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k)) as i32;
        // Shift first: `consume_key` ignores an extra Shift, so the plain
        // arrows would eat the shifted ones.
        let grow = key(Modifiers::SHIFT, Key::ArrowDown) - key(Modifiers::SHIFT, Key::ArrowUp);
        let step = key(Modifiers::NONE, Key::ArrowDown) - key(Modifiers::NONE, Key::ArrowUp);
        let fold = key(Modifiers::NONE, Key::ArrowRight) - key(Modifiers::NONE, Key::ArrowLeft);
        // Space says the same as Enter: take what the cursor stands on.
        let enter = key(Modifiers::NONE, Key::Enter) + key(Modifiers::NONE, Key::Space) != 0;
        if step == 0 && grow == 0 && fold == 0 && !enter {
            return;
        }
        // The tree has drawn already in this frame. Without a new frame the
        // screen shows the cursor one key late.
        ctx.request_repaint();
        let rows = if library { library_rows } else { project_rows };
        let half = self.half(panel);
        let open = if library { half.sel } else { self.tree_cursor.or(half.sel) };
        let at = half.at.clone().or(open.map(tree::Row::File));
        // Right and Left open and close the folder the keys stand on.
        if fold != 0 {
            if let Some(tree::Row::Dir(d)) = &at {
                self.half_mut(panel).open_dir = Some((d.clone(), fold > 0));
            }
            return;
        }
        // Shift walks the files only: a folder has nothing to mark.
        if grow != 0 && !library {
            let from = self.tree_cursor.or(self.project.sel);
            let Some(i) = walk(project_order, from, grow) else { return };
            let a = self.tree_anchor.or(from).unwrap_or(i);
            self.mark_range(project_order, a, i, false);
            self.tree_anchor = Some(a);
            self.tree_cursor = Some(i);
            self.project.at = Some(tree::Row::File(i));
            self.project.scroll = Some(tree::Row::File(i));
            return;
        }
        if enter && step == 0 && grow == 0 {
            if let Some(row) = &at {
                self.open_row(ctx, panel, row);
            }
            return;
        }
        let dir = step + grow;
        let Some(row) = walk_rows(rows, at.as_ref(), dir) else { return };
        self.stand_on(panel, row);
    }

    /// Puts the arrow keys on a row of a tree. Standing on a file does not
    /// open it: the cursor and the sheet on show are two different things,
    /// and Enter is what joins them.
    fn stand_on(&mut self, panel: Panel, row: tree::Row) {
        self.active = panel;
        let half = self.half_mut(panel);
        half.at = Some(row.clone());
        half.scroll = Some(row.clone());
        // A group grows from wherever the cursor stands, so the two agree.
        if panel == Panel::Project && let tree::Row::File(i) = row {
            self.tree_anchor = Some(i);
            self.tree_cursor = Some(i);
        }
    }

    /// Enter, or Space, in a tree: it opens the file the cursor stands on,
    /// or unfolds the folder it stands on.
    fn open_row(&mut self, ctx: &egui::Context, panel: Panel, row: &tree::Row) {
        match row {
            tree::Row::Dir(d) => self.half_mut(panel).open_dir = Some((d.clone(), true)),
            tree::Row::File(i) if panel == Panel::Library => self.open_library(ctx, *i),
            tree::Row::File(i) => self.request(ctx, Pending::Open(*i)),
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
        // The keys follow the file, however it was opened. A click that
        // left the cursor behind means the next arrow starts somewhere the
        // eye is not.
        match self.library.open(ctx, i) {
            Ok(()) => self.active = Panel::Library,
            Err(err) => self.status = err,
        }
    }

    fn open_project(&mut self, ctx: &egui::Context, i: usize) {
        match self.project.open(ctx, i) {
            Ok(()) => {
                self.tree_cursor = Some(i);
                self.active = Panel::Project;
            }
            Err(err) => self.status = err,
        }
    }

    /// Starts the fresh, unnamed tilesheet that an empty canvas grows: a
    /// block dropped on the panel starts one, and so does a Ctrl+Tab that
    /// crosses to it. Both name it at the first save.
    fn start_canvas(&mut self, ctx: &egui::Context, tile: [u32; 2]) {
        self.project.sheet = Some(self.new_canvas(ctx, "", tile));
        self.project.sel = None;
    }

    /// An empty tilesheet of about `NEW_PX` pixels a side.
    fn new_canvas(&self, ctx: &egui::Context, rel: &str, tile: [u32; 2]) -> Sheet {
        let cols = ((NEW_PX + tile[0] / 2) / tile[0]).max(1);
        let rows = ((NEW_PX + tile[1] / 2) / tile[1]).max(1);
        Sheet::new_empty(ctx, &self.project.index.root, rel, tile, cols, rows)
    }

    fn create_project(&mut self, ctx: &egui::Context) {
        if self.new_name.trim().is_empty() {
            return;
        }
        // The name stays inside the project, and a new sheet never takes
        // the place of one that is there.
        let Some(rel) = files::normalize_name(&self.new_name, Some(".png")) else {
            self.status = "that is not a usable name".into();
            return;
        };
        if self.project.index.root.join(&rel).exists() {
            self.status = format!("{rel} exists");
            return;
        }
        let mut sheet = self.new_canvas(ctx, &rel, self.project.inherited_tile());
        if let Err(e) = sheet.save() {
            self.status = e;
            return;
        }
        self.new_name.clear();
        self.rescan_project();
        if let Some(i) = self.project.index.position(&rel) {
            self.open_project(ctx, i);
        }
    }

    fn rescan_library(&mut self) {
        self.library.rescan(&self.qwords, self.settings.search);
        let index = &self.library.index;
        self.status = index.error.clone().unwrap_or_else(|| format!("{} files in the library", index.entries.len()));
    }

    fn rescan_project(&mut self) {
        self.marked.clear();
        self.project.rescan(&self.qwords, self.settings.search);
        if let Some(e) = &self.project.index.error { self.status = e.clone(); }
    }

    /// Deletes files (and, through their paths, whole folders), with their
    /// book entries. Runs only after the confirm dialog.
    fn delete_paths(&mut self, rels: &[String]) -> Result<(), String> {
        let root = self.project.index.root.clone();
        files::remove(&root, rels)?;
        if self.project.sheet.as_ref().is_some_and(|sheet| !root.join(&sheet.rel).exists()) { self.project.sheet = None; }
        self.status = format!("deleted {}", rels.join(", "));
        self.rescan_project();
        Ok(())
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
        let mut errors = Vec::new();
        for rel in &files {
            let name = rel.rsplit_once('/').map_or(rel.as_str(), |(_, n)| n);
            let new = if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };
            if let Err(e) = self.relocate(rel, &new, copy) {
                errors.push(e);
            }
        }
        let what = if copy { "copied" } else { "moved" };
        let where_to = if dir.is_empty() { "the top".to_string() } else { dir.clone() };
        self.status = format!("{} {what} to {where_to}", files.len() - errors.len());
        // A file that stayed says why.
        if !errors.is_empty() {
            self.status = format!("{}; {}", self.status, errors.join("; "));
        }
        self.marked.clear();
        self.rescan_project();
    }

    /// Moves or copies one file of the PROJECT tree, with its book entry. The
    /// open sheet follows its own file.
    fn relocate(&mut self, old: &str, new: &str, copy: bool) -> Result<(), String> {
        files::relocate(&self.project.index.root, old, new, copy)?;
        if !copy && let Some(sheet) = &mut self.project.sheet && sheet.rel == old { sheet.rel = new.to_string(); }
        Ok(())
    }

    fn apply_name(&mut self, ctx: &egui::Context, what: &NameFor, name: &str) -> Result<(), String> {
        let root = self.project.index.root.clone();
        match what {
            NameFor::NewFolder(parent) => {
                let rel = files::normalize_name(name, None).ok_or("that is not a usable name")?;
                let dir = if parent.is_empty() { rel } else { format!("{parent}/{rel}") };
                std::fs::create_dir_all(root.join(&dir)).map_err(|e| e.to_string())?;
                self.rescan_project();
            }
            NameFor::RenameFolder(old) => {
                let rel = files::normalize_name(name, None).ok_or("that is not a usable name")?;
                let new = match old.rsplit_once('/') {
                    Some((parent, _)) => format!("{parent}/{rel}"),
                    None => rel,
                };
                if new != *old {
                    files::relocate(&root, old, &new, false)?;
                    if let Some(sheet) = &mut self.project.sheet
                        && let Some(rest) = sheet.rel.strip_prefix(&format!("{old}/")) {
                        sheet.rel = format!("{new}/{rest}");
                    }
                    self.rescan_project();
                }
            }
            NameFor::SaveAs => {
                let rel = files::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if root.join(&rel).exists() {
                    return Err(format!("{rel} exists"));
                }
                let Some(sheet) = &mut self.project.sheet else {
                    return Ok(());
                };
                if let Some(parent) = root.join(&rel).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let old = std::mem::replace(&mut sheet.rel, rel.clone());
                if let Err(e) = sheet.save() {
                    sheet.rel = old;
                    return Err(e);
                }
                self.status = format!("saved as {rel}");
                self.rescan_project();
            }
            NameFor::RenameFile(old) => {
                let rel = files::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                if rel != *old {
                    self.relocate(old, &rel, false)?;
                    self.rescan_project();
                }
            }
            NameFor::DuplicateFile(old) => {
                let rel = files::normalize_name(name, Some(".png")).ok_or("that is not a usable name")?;
                self.relocate(old, &rel, true)?;
                self.rescan_project();
                // The copy opens as any file does: unsaved changes ask first.
                if let Some(i) = self.project.index.position(&rel) {
                    self.request(ctx, Pending::Open(i));
                }
            }
        }
        Ok(())
    }

    /// Keeps search in step with an edit. Saving is explicit: Ctrl+S.
    fn after_edit(&mut self) {
        self.project.sync_entry();
        self.project.refresh_visible(&self.qwords, self.settings.search);
    }

    fn save(&mut self) {
        let Some(sheet) = &mut self.project.sheet else {
            return;
        };
        // A sheet with no name asks for one. A sheet from a GIF or a JPEG
        // asks for a PNG name: those formats would lose frames or alpha.
        if sheet.rel.is_empty() || !sheet::is_png(&sheet.rel) {
            let value = if sheet.rel.is_empty() { String::new() } else { Path::new(&sheet.rel).with_extension("png").to_string_lossy().into() };
            self.prompt = Some(NamePrompt {
                title: "Save as".into(),
                value,
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
        let Some(sheet) = &mut self.project.sheet else {
            return;
        };
        let before = (sheet.cols(), sheet.rows());
        sheet.trim(ctx);
        self.status = if (sheet.cols(), sheet.rows()) == before {
            "nothing to trim".to_string()
        } else {
            format!("trimmed to {}x{} tiles", sheet.cols(), sheet.rows())
        };
        self.after_edit();
    }

    fn has_unsaved(&self) -> bool {
        self.project.sheet.as_ref().is_some_and(|s| s.dirty)
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
                self.project.sheet = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The status line: the settings gear at the right, then the text, which
    /// disappears ten seconds after it last changed. With the legend hidden
    /// it runs under the whole window; with the legend shown it stays under
    /// the sheet panels, so the trees keep their height.
    fn status_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        const STATUS_SECS: u64 = 10;
        if self.status != self.shown_status {
            self.shown_status = self.status.clone();
            self.status_at = std::time::Instant::now();
        }
        let age = self.status_at.elapsed();
        egui::Panel::bottom("status").show_separator_line(true).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let gear = ui.small_button("⚙").on_hover_text("settings (Ctrl+,)");
                set_pane(ui, (Panel::Project, Spot::Status));
                stop(&gear);
                self.settings_popup(&gear, ui);
                if age.as_secs() < STATUS_SECS {
                    ui.colored_label(egui::Color32::from_rgb(190, 40, 30), egui::RichText::new(&self.status).strong());
                    ctx.request_repaint_after(Duration::from_secs(STATUS_SECS) - age);
                } else {
                    ui.label("");
                }
            });
        });
    }

    /// The settings, in a popup above the gear, opening up and to the left:
    /// the legend, and the AI providers, models, and defaults. Both files
    /// are written when the popup closes.
    fn settings_popup(&mut self, gear: &egui::Response, ui: &egui::Ui) {
        let id = Id::new("settings popup");
        egui::Popup::new(id, ui.ctx().clone(), gear, ui.layer_id())
            .open_memory(gear.clicked().then_some(egui::SetOpenCommand::Toggle))
            .align(egui::RectAlign::TOP_END)
            .show(|ui| {
                // A maximum, not a fixed width: the popup shrinks to its
                // content, and wraps a long line instead of growing wide
                // enough to keep it on one.
                ui.set_max_width(480.0);
                ui.strong("Settings");
                let mut legend = !self.settings.hide_legend;
                let first = ui.checkbox(&mut legend, "Show keyboard shortcuts");
                if first.changed() {
                    self.settings.hide_legend = !legend;
                }
                popup_keys(ui, gear, &first);
                if AI_VISIBLE {
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                        ai::settings_ui(ui, &mut self.settings.ai, &mut self.keys);
                    });
                }
            });
        let open = egui::Popup::is_id_open(ui.ctx(), id);
        if self.config_open && !open {
            if let Err(e) = self.settings.save() { self.status = e; }
            if let Err(e) = self.keys.save() { self.status = e; }
        }
        self.config_open = open;
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // A dialog or a popup takes the keys as egui gives them, and no
        // command of the window behind it fires.
        if self.dialog_open(ctx) {
            return;
        }
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let cmd = Modifiers::COMMAND;
        let pressed = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Key { key: Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Tab, pressed: true, .. }))
        });
        if pressed {
            trace(self, ctx, "key");
        }
        // A run of Shift and the arrows keeps the corner that walks. It ends
        // when Shift comes up, so nothing invisible outlasts the gesture.
        if !ctx.input(|i| i.modifiers.shift) {
            for s in [self.library.sheet.as_mut(), self.project.sheet.as_mut()].into_iter().flatten() {
                s.end_run();
            }
        }
        // Saving works no matter what has focus; a swallowed Ctrl+S loses work.
        if key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) && let Some(sheet) = &self.project.sheet {
            self.prompt = Some(NamePrompt {
                title: "Save as".into(),
                value: sheet.rel.clone(),
                what: NameFor::SaveAs,
                focus: true,
            });
        }
        if key(cmd, Key::S) {
            self.save();
        }
        // The search field, from anywhere. It is the one place the panes do
        // not walk to in a step or two, because it sits above them all.
        if key(cmd, Key::F) {
            self.go(ctx, search_id());
        }
        if key(cmd, Key::Comma) {
            egui::Popup::toggle_id(ctx, Id::new("settings popup"));
        }
        // Tab walks the stops, and Ctrl+Tab walks the panes. They work from
        // a text field too. The most modifiers go first: `consume_key` lets
        // a plain Tab eat a shifted one.
        if key(cmd | Modifiers::SHIFT, Key::Tab) {
            self.press_pane(ctx, -1);
        } else if key(cmd, Key::Tab) {
            self.press_pane(ctx, 1);
        } else if key(Modifiers::SHIFT, Key::Tab) {
            self.press_tab(ctx, -1);
        } else if key(Modifiers::NONE, Key::Tab) {
            self.press_tab(ctx, 1);
        }
        // Typing beats every shortcut below.
        if ctx.text_edit_focused() {
            return;
        }
        // A frame in which nothing holds the keys, such as the frame in
        // which a text field lets go of them, fires no command.
        let focus = ctx.memory(|m| m.focused());
        if focus.is_none_or(|id| self.pane_of(id).is_none()) {
            return;
        }
        // The status bar lies in neither half. A command that asks which
        // panel it means has no answer while the keys are there, so it does
        // nothing at all. Saving, undo and the settings are not of that kind
        // and go on working from anywhere.
        let in_half = self.pane.1 != Spot::Status;
        // The eye is for looking: a panel with it on takes no edit and no
        // selection. Copying, zooming, and the panels stay.
        let project_eye = self.project_eye;
        let eye = project_eye && self.active == Panel::Project;
        // The window layer turns Ctrl+C into a Copy event, and Ctrl+V into a Paste
        // event that only exists when the system clipboard holds text.
        let (copy, cut, paste) = ctx.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let cut = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
            let paste = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
            (copy, cut, paste)
        });
        let cut = !project_eye && (cut || key(cmd, Key::X));

        if in_half && (copy || cut || key(cmd, Key::C)) {
            let from = &self.half(self.active).sheet;
            if let Some(b) = from.as_ref().and_then(Sheet::copy) {
                self.status = format!("copied {}x{} tiles", b.cols, b.rows);
                // A note in the system clipboard, so that Ctrl+V reaches us as a Paste event.
                ctx.copy_text(b.note());
                self.clip = Some(b);
                // A cut clears the cells; only your tilesheet is editable.
                if cut && self.active == Panel::Project && let Some(sheet) = &mut self.project.sheet {
                    sheet.clear_selection();
                    self.after_edit();
                }
            }
        }
        if in_half && !project_eye && (paste || key(cmd, Key::V)) && let (Some(block), Some(sheet)) = (&self.clip, &mut self.project.sheet) {
            let at = sheet.sel.origin().unwrap_or((0, 0));
            sheet.paste(ctx, at, block);
            self.active = Panel::Project;
            self.after_edit();
        }
        if !project_eye && key(cmd, Key::T) {
            self.trim(ctx);
        }
        // A step back, and a step forward again. They belong to the sheet
        // you are in, so the library's grid and animation changes take them
        // as well. Shift goes first, or the plain one eats it.
        if in_half && !eye {
            let panel = self.active;
            let again = key(cmd | Modifiers::SHIFT, Key::Z) || key(cmd, Key::Y);
            let back = !again && key(cmd, Key::Z);
            if again || back {
                if let Some(sheet) = self.sheet_mut(panel) {
                    if again {
                        sheet.redo(ctx);
                    } else {
                        sheet.undo(ctx);
                    }
                }
                self.after_animation_edit(panel);
            }
        }
        if in_half && self.active == Panel::Project && !project_eye
            && (key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace))
            && let Some(sheet) = &mut self.project.sheet {
            sheet.clear_selection();
            self.after_edit();
        }
        if in_half && !eye && key(cmd, Key::A) && let Some(s) = self.sheet_mut(self.active) {
            s.sel = Sel::rect((0, 0), (s.cols() - 1, s.rows() - 1));
        }
        if in_half && !eye && key(Modifiers::NONE, Key::A) {
            self.press_a();
        }
        if in_half && !eye && key(Modifiers::NONE, Key::M) {
            self.press_m();
        }
        if in_half && AI_VISIBLE && key(Modifiers::NONE, Key::I) {
            self.ai_panel = !self.ai_panel;
        }
        if in_half && self.active == Panel::Project && key(Modifiers::NONE, Key::E) {
            self.project_eye = !self.project_eye;
        }
        // The arrows move the selection of the sheet that holds the focus.
        // Shift walks one corner and keeps the other; Ctrl steps from one
        // block of filled cells to the next; Alt walks the whole selection.
        // The most modifiers go first: `consume_key` lets a plain arrow eat
        // a shifted one.
        let sheet_keys = focus
            == Some(match self.active {
                Panel::Library => library_id(),
                Panel::Project => project_id(),
            });
        // An arrow never leaves the body it is in. On any other stop the
        // arrows do nothing.
        if !eye && sheet_keys {
            const DIRS: [(Key, (i32, i32)); 4] = [
                (Key::ArrowRight, (1, 0)),
                (Key::ArrowLeft, (-1, 0)),
                (Key::ArrowDown, (0, 1)),
                (Key::ArrowUp, (0, -1)),
            ];
            let combos = [
                (cmd | Modifiers::SHIFT, Move::Grow(true)),
                (Modifiers::SHIFT, Move::Grow(false)),
                (cmd, Move::Step(true)),
                (Modifiers::ALT, Move::Whole),
                (Modifiers::NONE, Move::Step(false)),
            ];
            let hit = combos.into_iter().find_map(|(m, what)| DIRS.into_iter().find(|(k, _)| key(m, *k)).map(|(_, d)| (d, what)));
            if let Some((d, what)) = hit && let Some(s) = self.sheet_mut(self.active) {
                match what {
                    Move::Grow(ctrl) => s.arrow(d, true, ctrl),
                    Move::Step(ctrl) => s.arrow(d, false, ctrl),
                    Move::Whole => s.nudge(d),
                }
            }
        }
        if in_half && key(Modifiers::NONE, Key::Escape) && let Some(s) = self.sheet_mut(self.active) {
            s.sel = Sel::default();
        }
        // + and - zoom the view under the pointer, else the active sheet.
        let dir = if key(Modifiers::NONE, Key::Plus) || key(Modifiers::NONE, Key::Equals) {
            1
        } else if key(Modifiers::NONE, Key::Minus) {
            -1
        } else {
            0
        };
        if dir != 0 && let Some(z) = self.zoom_under_pointer() {
            z.step(dir);
        }
    }

    fn start_drag(&mut self, ctx: &egui::Context, from: Panel, cell: (u32, u32)) {
        let sheet = self.half(from).sheet.as_ref();
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
        let mut target = self.project.sheet.as_ref().and_then(|d| {
            let c = d.cell_at(p)?;
            Some((c.0.saturating_sub(drag.grab.0), c.1.saturating_sub(drag.grab.1)))
        });
        // The ghost is drawn at the zoom of the panel it is over, in pixels
        // of the block, since the tile sizes may differ.
        let block_px = Vec2::new(drag.block.img.width() as f32, drag.block.img.height() as f32);
        let library_cell = Vec2::new(drag.block.tile[0] as f32, drag.block.tile[1] as f32);
        let (min, zoom) = match (target, &self.project.sheet) {
            (Some(t), Some(d)) => {
                let c = d.cell_px();
                (d.screen.min + Vec2::new(t.0 as f32 * c.x, t.1 as f32 * c.y), d.zoom_px())
            }
            _ => {
                let z = self.half(drag.from).sheet.as_ref().map_or(2.0, |s| s.zoom_px());
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
        if self.project_eye {
            self.status = "the eye is on in your tilesheet; switch it off to drop".into();
            return;
        }
        // A drop on the empty pane starts a fresh, unnamed tilesheet; its name
        // is asked for at the first save.
        if target.is_none() && self.project.sheet.is_none() && self.project_rect.contains(p) {
            // The new tilesheet takes the grid of the block that lands on it.
            self.start_canvas(ctx, drag.block.tile);
            target = Some((0, 0));
        }
        let (Some(at), Some(sheet)) = (target, &mut self.project.sheet) else {
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

    /// Returns the new grid when the user finished editing a field
    /// (tile, gap, offset), and whether a header button was clicked: the
    /// caller makes this panel active on a click, so a key like `A` or `E`
    /// next acts on the panel whose button was just pressed.
    fn sheet_header(
        ui: &mut egui::Ui,
        title: &str,
        keys: bool,
        library: bool,
        sheet: Option<&mut Sheet>,
        ai: Option<&mut bool>,
        eye: Option<&mut bool>,
    ) -> (Option<Grid>, bool) {
        let mut new_grid = None;
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.label(title_text(title, keys));
            let Some(s) = sheet else {
                ui.weak("nothing open");
                clicked = Self::header_tail(ui, None, ai, eye, String::new(), None);
                return;
            };
            ui.weak(format!("{}x{} tiles", s.cols(), s.rows()));
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
                ui.weak(format!("sel {} tiles, {}x{} at {},{}", s.sel.len(), b.cols(), b.rows(), b.x0, b.y0));
            }
            let name = if s.rel.is_empty() { "(unnamed)" } else { s.rel.as_str() };
            let name = if s.dirty { format!("{name} *") } else { name.to_string() };
            let cell = s.hover.map(|(x, y)| format!("tile {x},{y}"));
            clicked = Self::header_tail(ui, Some(s), ai, eye, name, cell);
        });
        (new_grid, clicked)
    }

    /// The right end of a header: the buttons that open the side panels,
    /// then the name of the sheet and the cell under the pointer. The two
    /// texts are truncated: a long name can never push the fields off screen.
    /// Returns whether one of the three buttons was clicked.
    fn header_tail(ui: &mut egui::Ui, sheet: Option<&mut Sheet>, ai: Option<&mut bool>, eye: Option<&mut bool>, name: String, cell: Option<String>) -> bool {
        let mut clicked = false;
        // The buttons draw from the right, so they are kept here and become
        // stops from the left, in the order you read them.
        let mut here: Vec<egui::Response> = Vec::new();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The buttons of the sheet wait, greyed out, for a sheet.
            let open = sheet.is_some();
            if let Some(ai) = ai {
                let r = ui.add(egui::Button::new("✨").small().selected(*ai)).on_hover_text("AI assist panel (I)");
                if r.clicked() {
                    *ai = !*ai;
                    clicked = true;
                }
                here.push(r);
            }
            let anim = sheet.as_ref().is_some_and(|s| s.anim_panel);
            let r = ui.add_enabled(open, egui::Button::new("🎬").small().selected(anim)).on_hover_text("animation panel (A)");
            if r.clicked() {
                clicked = true;
                if let Some(s) = sheet {
                    if s.anim_panel {
                        s.anim_panel = false;
                    } else {
                        s.open_anim_panel();
                    }
                }
            }
            here.push(r);
            if let Some(eye) = eye {
                let r = ui.add_enabled(open, egui::Button::new("👁").small().selected(*eye))
                    .on_hover_text("view information about the sheet, no editing (E)");
                if r.clicked() {
                    *eye = !*eye;
                    clicked = true;
                }
                here.push(r);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(name).truncate());
                if let Some(cell) = cell {
                    ui.add(egui::Label::new(egui::RichText::new(cell).weak()).truncate());
                }
            });
        });
        for r in here.iter().rev() {
            stop(r);
        }
        clicked
    }

    /// Configuration belongs in Settings; this panel shows the label of the open sheet.
    fn assist_panel(
        ui: &mut egui::Ui, ai: &ai::Ai, keys: &ai::Keys, sheet: Option<&Sheet>, run: Option<&labels::Run>, outcome: Option<&str>,
    ) -> Option<labels::Action> {
        ui.strong("AI label");
        let ready = match ai.chosen(ai::Mode::Instant) {
            Some((p, m)) if p.kind == ai::Kind::OpenAi && p.key_source(keys) != ai::KeySource::None => {
                ui.weak(format!("Ready: {}", m.id));
                true
            }
            _ => { ui.weak("Set an instant image model and key in Settings."); false }
        };
        let mut cancel = None;
        if let Some(run) = run {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Waiting for the model: {}s of 60s.", run.started.elapsed().as_secs()));
            });
            ui.weak(run.path.file_name().unwrap_or_default().to_string_lossy()).on_hover_text(run.path.display().to_string());
            cancel = Some(stopped(ui.button("Cancel").on_hover_text("Stops waiting. The provider may still bill the request.")));
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        } else if let Some(outcome) = outcome {
            ui.label(outcome);
        }
        match sheet.map(|s| &s.side.label) {
            Some(Some(label)) => label.show(ui),
            Some(None) => { ui.weak("No AI label yet."); }
            None => { ui.weak("Select a library sheet."); }
        }
        let label = ui.add_enabled(ready && sheet.is_some() && run.is_none(), egui::Button::new("Label with AI"))
            .on_hover_text("Sends this sheet to the configured instant model.");
        let remove = ui.add_enabled(sheet.is_some_and(|s| s.side.label.is_some()) && run.is_none(), egui::Button::new("Remove AI label..."));
        stop(&label);
        stop(&remove);
        if label.clicked() { Some(labels::Action::Label) } else if remove.clicked() { Some(labels::Action::Remove) }
            else if cancel.as_ref().is_some_and(egui::Response::clicked) { Some(labels::Action::Cancel) } else { None }
    }

    /// Every UI entry point reaches this operation after it opens the target sheet.
    fn label_action(&mut self, ctx: &egui::Context, action: labels::Action) {
        self.ai_panel = true;
        if action == labels::Action::Cancel {
            self.label_run = None;
            self.label_outcome = Some("Cancelled. The provider may still bill the request.".into());
            return;
        }
        if self.label_run.is_some() {
            self.status = "Wait for the current label, or cancel it.".into();
            return;
        }
        let Some(sheet) = &self.library.sheet else { return };
        if action == labels::Action::Remove {
            self.remove_label = Some((sheet.dir.clone(), sheet.rel.clone()));
            ctx.request_repaint();
            return;
        }
        let result = (|| {
            let (provider, model) = self.settings.ai.chosen(ai::Mode::Instant).ok_or("Choose an instant model in Settings.")?;
            if provider.kind != ai::Kind::OpenAi { return Err("Choose an OpenAI-compatible provider in Settings.".into()); }
            let key = provider.key(&self.keys).ok_or("Set the provider key in Settings.")?;
            let endpoint = labels::Endpoint::new(&provider.url, key)?;
            let ctx = ctx.clone();
            labels::Run::start(sheet.label_input(), provider.name.clone(), model.id.clone(), move |body| endpoint.send(body), move || ctx.request_repaint())
        })();
        self.label_outcome = None;
        match result {
            Ok(run) => { self.label_run = Some(run); ctx.request_repaint(); }
            Err(error) => { self.status = error.clone(); self.label_outcome = Some(error); }
        }
    }

    fn receive_label(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let Some(run) = &self.label_run else { return };
        let result = match run.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The labeling operation stopped unexpectedly.".into()),
        };
        let run = self.label_run.take().unwrap();
        let result = result.and_then(|label| {
            let labels = [(run.rel.clone(), Some(label))];
            sidecar::store_labels(&run.dir, labels.iter().map(|(rel, label)| (rel.as_str(), label.clone())))
                .map_err(|e| format!("Could not save the label: {e}"))?;
            self.apply_labels(&run.dir, &labels);
            Ok(())
        });
        let notice = match result {
            Ok(()) => "Label saved.".to_string(),
            Err(error) => format!("Labeling failed: {error}"),
        };
        self.status = format!("{}: {notice}", run.path.file_name().unwrap_or_default().to_string_lossy());
        self.label_outcome = Some(self.status.clone());
    }

    /// Puts labels that the book now holds into the open sheets and the search.
    fn apply_labels(&mut self, dir: &Path, labels: &[(String, Option<sidecar::Label>)]) {
        if labels.is_empty() { return; }
        self.library.apply_labels(dir, labels);
        self.project.apply_labels(dir, labels);
        self.refresh_visible();
    }

    /// `A` opens or closes the animation panel of the active panel. Storing
    /// or unmarking the animation is a separate action; see `press_m`.
    fn press_a(&mut self) {
        let Some(sheet) = self.sheet_mut(self.active) else {
            return;
        };
        if sheet.anim_panel {
            sheet.anim_panel = false;
        } else {
            sheet.open_anim_panel();
        }
    }

    /// `M` stores the draft as an animation, or removes the stored one under
    /// the selection, in the active panel. It acts only while that panel's
    /// animation panel is open, like the Store and Unmark buttons in it.
    fn press_m(&mut self) {
        let panel = self.active;
        let Some(sheet) = self.sheet_mut(panel) else {
            return;
        };
        if !sheet.anim_panel {
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
        let panel = if self.library.sheet.as_ref().is_some_and(hovered) {
            Panel::Library
        } else if self.project.sheet.as_ref().is_some_and(hovered) {
            Panel::Project
        } else {
            active
        };
        let s = self.sheet_mut(panel)?;
        Some(if s.preview_hovered { &mut s.preview_zoom } else { &mut s.zoom })
    }

    /// Applies a new grid (tile, gap, offset) to a sheet. Your tilesheet keeps
    /// it as an unsaved edit; a library sheet stores it at once.
    fn change_grid(&mut self, ctx: &egui::Context, panel: Panel, (t, gap, offset): Grid) {
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
                if let Err(e) = self.store_library_entry() {
                    self.status = e;
                }
            }
        }
    }

    /// The pane of a stop. The bodies have fixed names, so they answer even
    /// on the frame before they first draw. Every other stop answers from
    /// the list that the last frame drew.
    fn pane_of(&self, id: Id) -> Option<(Panel, Spot)> {
        let fixed = [
            (library_tree_id(), (Panel::Library, Spot::Tree)),
            (project_tree_id(), (Panel::Project, Spot::Tree)),
            (library_id(), (Panel::Library, Spot::Sheet)),
            (project_id(), (Panel::Project, Spot::Sheet)),
        ];
        if let Some((_, p)) = fixed.iter().find(|(i, _)| *i == id) {
            return Some(*p);
        }
        self.stops.iter().find(|(_, i)| *i == id).map(|(p, _)| *p)
    }

    /// Where the keys land in a pane: the rows of a tree, the grid of a
    /// sheet, else the first stop of the pane.
    fn body(&self, pane: (Panel, Spot)) -> Option<Id> {
        match pane.1 {
            Spot::Tree => return Some(tree_id(pane.0)),
            Spot::Sheet if self.half(pane.0).sheet.is_some() => return Some(sheet_id(pane.0)),
            _ => {}
        }
        self.stops.iter().find(|(p, _)| *p == pane).map(|(_, id)| *id)
    }

    /// Gives the keys to a stop, and makes its pane the pane in use.
    fn go(&mut self, ctx: &egui::Context, id: Id) {
        if std::env::var_os("TILEPICKY_KEYS").is_some() {
            eprintln!("[keys]   -> {}", place_name(self, id));
        }
        ctx.memory_mut(|m| m.request_focus(id));
        if let Some(at) = self.pane_of(id) {
            self.pane = at;
            // The status bar belongs to neither half, so it leaves the panel
            // in use as it was: `A`, `M` and copying still mean that panel.
            if at.1 != Spot::Status {
                self.active = at.0;
            }
        }
        // A tree with no row under the keys, or a grid with nothing
        // selected, shows nothing at all. Arriving puts that right at once.
        self.enter_tree(id);
        if let Some(panel) = [Panel::Library, Panel::Project].into_iter().find(|p| sheet_id(*p) == id)
            && let Some(sheet) = self.sheet_mut(panel)
        {
            sheet.start();
        }
    }

    /// Puts the keys on a row when they arrive in a tree with none: the file
    /// on show, else the first row.
    fn enter_tree(&mut self, id: Id) {
        let Some(panel) = tree_panel(id) else { return };
        let half = self.half(panel);
        // A cursor that is still on a row of this tree stays where it is.
        if half.at.as_ref().is_some_and(|r| half.rows.contains(r)) {
            return;
        }
        let rows = &half.rows;
        let row = half.sel.map(tree::Row::File).filter(|r| rows.contains(r)).or_else(|| rows.first().cloned());
        if let Some(row) = row {
            self.stand_on(panel, row);
        }
    }

    /// Tab: the next stop in reading order, or the one before it. The walk
    /// wraps at both ends.
    fn press_tab(&mut self, ctx: &egui::Context, step: i32) {
        let ids: Vec<Id> = self.stops.iter().map(|(_, id)| *id).collect();
        let from = ctx.memory(|m| m.focused());
        if let Some(to) = next_stop(&ids, from, self.body(self.pane), step) {
            self.go(ctx, to);
        }
    }

    /// Ctrl+Tab: the body of the next pane, or of the one before it. A pane
    /// with nothing to stop at is passed over.
    fn press_pane(&mut self, ctx: &egui::Context, step: i32) {
        let here = PANES.iter().position(|p| *p == self.pane).unwrap_or(0);
        // The canvas is the one pane you can make on the way. A step down
        // from the source sheet starts a tilesheet there, as a dropped block
        // does, at the tile size of the sheet you come from.
        if step > 0 && self.pane == (Panel::Library, Spot::Sheet) && self.project.sheet.is_none() && self.project.is_set() {
            let tile = self.library.inherited_tile();
            self.start_canvas(ctx, tile);
        }
        let n = PANES.len() as i32;
        for k in 1..n {
            let pane = PANES[(here as i32 + k * step).rem_euclid(n) as usize];
            if let Some(id) = self.body(pane) {
                self.go(ctx, id);
                if let Some(s) = self.sheet_mut(pane.0) {
                    s.end_run();
                }
                return;
            }
        }
    }

    fn sheet_mut(&mut self, panel: Panel) -> Option<&mut Sheet> {
        self.half_mut(panel).sheet.as_mut()
    }

    /// A tilesheet keeps the change until Ctrl+S. A library sheet has no pixel
    /// edits, so its book entry is written at once.
    fn after_animation_edit(&mut self, panel: Panel) {
        match panel {
            Panel::Project => self.after_edit(),
            Panel::Library => match self.store_library_entry() {
                Ok(()) => self.status = format!("stored in {}", self.library.index.root.join(sidecar::BOOK).display()),
                Err(e) => self.status = e,
            },
        }
    }

    /// Writes the book entry of the library sheet, and gives search the
    /// same entry.
    fn store_library_entry(&mut self) -> Result<(), String> {
        let result = self.library.sheet.as_mut().map_or(Ok(()), Sheet::save_entry);
        self.library.sync_entry();
        result
    }

    /// The side panel of a sheet: the selection played as an animation, with
    /// fields for the frame size and the frame time. A stored animation is
    /// edited in place; otherwise the fields shape a draft. Returns whether a
    /// stored animation changed, or the reason a change was refused.
    fn animation_panel(ui: &mut egui::Ui, sheet: &mut Sheet, library: bool, keys: bool) -> Result<bool, String> {
        // The content claims the panel's width: egui stores the width the
        // content took, and a narrower content would shrink the panel on
        // its second frame.
        ui.set_min_width(ui.available_width());
        // Hovering anywhere in this panel, not only the preview image,
        // zooms the preview: this panel is not the sheet, so the sheet's
        // own zoom must stay out of reach here.
        sheet.preview_hovered = ui.rect_contains_pointer(ui.max_rect());
        if sheet.preview_hovered {
            sheet.preview_zoom.wheel(ui);
        }
        ui.label(title_text("Animation", keys));
        let Some(b) = sheet.sel.bounds() else {
            ui.weak("Select tiles to play them.");
            return Ok(false);
        };

        let tile = sheet.tile;
        let stored = sheet.stored_animation();
        // The frame size in cells: the one the stored animation has, else the
        // draft's. A stored frame smaller than a cell reads as one cell, and
        // becomes one as soon as the user changes the field.
        // The cell of a stored animation is its frame size read in tiles.
        // That only works when the tiles divide it: a sheet whose tile size
        // changed after the animation was stored has frames that no whole
        // number of tiles describes. The field then reads pixels, which is
        // always the truth, rather than a rounded number that is not.
        let in_tiles = stored.as_ref().is_none_or(|a| a.frame[0] % tile[0] == 0 && a.frame[1] % tile[1] == 0);
        let (mut cell, mut ms) = match &stored {
            Some(a) if in_tiles => ([a.frame[0] / tile[0], a.frame[1] / tile[1]], a.ms),
            Some(a) => (a.frame, a.ms),
            None => sheet.draft().map(|d| (d.frame, d.ms)).unwrap_or(([1, 1], 100)),
        };
        let mut changed = false;
        egui::Grid::new("animation fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("cell");
            let field = if in_tiles { cell_field(library) } else { frame_px_field(library) };
            if let Some(f) = field.ui(ui, cell) {
                cell = f;
                changed = true;
            }
            ui.end_row();
            ui.label("ms");
            if let Some(v) = ms_field(library).ui(ui, [ms, ms]) {
                ms = v[0];
                changed = true;
            }
            ui.end_row();
        });
        // The fields apply before the panel reads what they made, so a number
        // just typed or dragged shows in the same repaint, not the next one.
        let mut result = Ok(false);
        match &stored {
            // The sheet works in pixels; the field says how many that is.
            Some(_) if changed => {
                let frame = if in_tiles { [cell[0] * tile[0], cell[1] * tile[1]] } else { cell };
                result = sheet.set_animation(frame, ms).map(|()| true);
            }
            Some(_) => {}
            None => {
                if let Some(d) = sheet.draft() {
                    d.frame = cell;
                    d.ms = ms;
                }
            }
        }
        // What plays: the stored animation, else the whole frames the
        // selection holds. The cells they miss are spare.
        let anim = sheet.stored_animation().or_else(|| sheet.draft().and_then(|d| d.animation(tile)));
        let spare = match &stored {
            Some(_) => 0,
            None => b.cols() * b.rows() - (b.cols() / cell[0]) * cell[0] * (b.rows() / cell[1]) * cell[1],
        };
        // What the fields made of the selection.
        ui.horizontal(|ui| {
            match &anim {
                Some(a) => ui.weak(format!("{} frames of {}x{} px", show_frames(a.grid()), a.frame[0], a.frame[1])),
                None => ui.colored_label(sheet::SPARE, "the cell is larger than the selection"),
            };
        });
        if spare > 0 && anim.is_some() {
            let cells = if spare == 1 { "1 tile".to_string() } else { format!("{spare} tiles") };
            ui.colored_label(sheet::SPARE, format!("{cells} spare"));
        }
        // The button at the bottom; the preview takes the space above it. The
        // panel itself is opened and closed by the header's 🎬 button.
        egui::Panel::bottom("animation buttons").show_separator_line(false).show(ui, |ui| {
            // A single frame is a picture, not an animation; unmarking a
            // stored one stays enabled no matter its frame count.
            let can_store = stored.is_some() || anim.as_ref().is_some_and(|a| a.count() > 1);
            let label = if stored.is_some() { "Unmark (M)" } else { "Store (M)" };
            let r = ui.add_enabled(can_store, egui::Button::new(label));
            stop(&r);
            if !can_store {
                r.on_disabled_hover_text("an animation needs more than one frame");
            } else if r.clicked() {
                result = sheet.toggle_animation().map(|()| true);
            }
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
        let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        let ppp = ui.ctx().pixels_per_point();
        let rect = egui::Rect::from_min_size(Pos2::new((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp), rect.size());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(225));
        let [fx, fy] = a.frame_px(frame);
        let origin = Pos2::new(fx as f32, fy as f32);
        sheet.draw_px_rect(
            ui.painter(),
            Rect::from_min_size(origin, Vec2::new(a.frame[0] as f32, a.frame[1] as f32)),
            rect.min,
            zoom,
        );
        ui.weak(format!("{}x - frame {}/{}", sheet.preview_zoom.level, frame + 1, n));
        ui.ctx().request_repaint_after(Duration::from_millis(a.ms.max(16) as u64));
    }

    /// What a click or a menu in the LIBRARY tree asks for.
    fn library_tree_action(&mut self, ctx: &egui::Context, action: TreeAction) {
        match action {
            TreeAction::Open(i) => self.open_library(ctx, i),
            TreeAction::Labels(i, action) => {
                let rel = self.library.index.entries[i].rel.clone();
                if self.library.sheet.as_ref().is_none_or(|s| s.rel != rel) { self.open_library(ctx, i); }
                if self.library.sheet.as_ref().is_some_and(|s| s.rel == rel) { self.label_action(ctx, action); }
            }
            TreeAction::Refresh => self.rescan_library(),
            TreeAction::Reveal(i) => reveal(&file_path(&self.library.index.root, &self.library.index.entries[i].rel)),
            TreeAction::RevealDir(dir) => reveal(&file_path(&self.library.index.root, &dir)),
            TreeAction::CopyPath(i, whole) => {
                let rel = self.library.index.entries[i].rel.clone();
                self.copy_path(ctx, &self.library.index.root.clone(), &rel, whole);
            }
            TreeAction::CopyDirPath(dir, whole) => self.copy_path(ctx, &self.library.index.root.clone(), &dir, whole),
            _ => {}
        }
    }

    /// What a click, a drag, or a menu in the PROJECT tree asks for.
    /// `project_order` is the files in the order the tree shows them.
    fn project_tree_action(&mut self, ctx: &egui::Context, action: TreeAction, project_order: &[usize]) {
        match action {
            TreeAction::Open(i) => {
                // The plainly clicked file is the start of any group.
                self.marked.clear();
                self.marked.insert(i);
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
                self.request(ctx, Pending::Open(i));
            }
            TreeAction::Toggle(i) => {
                if !self.marked.remove(&i) {
                    self.marked.insert(i);
                }
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
            }
            TreeAction::Range(i, additive) => {
                self.tree_cursor = Some(i);
                let a = self.tree_anchor.unwrap_or(i);
                self.mark_range(project_order, a, i, additive);
            }
            TreeAction::LiftFile(i) => {
                let group = self.marked.len() > 1 && self.marked.contains(&i);
                self.file_drag = Some(if group {
                    let mut rels: Vec<String> = self.marked.iter().map(|&k| self.project.index.entries[k].rel.clone()).collect();
                    rels.sort();
                    rels
                } else {
                    self.marked.clear();
                    self.marked.insert(i);
                    vec![self.project.index.entries[i].rel.clone()]
                });
            }
            TreeAction::SweepStart(i) => {
                self.sweep = Some(i);
                self.tree_anchor = Some(i);
                self.tree_cursor = Some(i);
                self.marked.clear();
                self.marked.insert(i);
            }
            TreeAction::Sweep(i) => {
                let a = self.sweep.unwrap_or(i);
                self.tree_cursor = Some(i);
                self.mark_range(project_order, a, i, false);
            }
            TreeAction::DeleteFile(i) => {
                let rel = self.project.index.entries[i].rel.clone();
                self.confirm = Some((format!("Delete {rel}? There is no undo."), vec![rel]));
            }
            TreeAction::DeleteMarked => {
                let rels: Vec<String> = self.marked.iter().map(|&i| self.project.index.entries[i].rel.clone()).collect();
                self.confirm = Some((format!("Delete {} files? There is no undo.", rels.len()), rels));
            }
            TreeAction::Refresh => self.rescan_project(),
            TreeAction::Reveal(i) => reveal(&file_path(&self.project.index.root, &self.project.index.entries[i].rel)),
            TreeAction::RevealDir(dir) => reveal(&file_path(&self.project.index.root, &dir)),
            TreeAction::CopyPath(i, whole) => {
                let rel = self.project.index.entries[i].rel.clone();
                self.copy_path(ctx, &self.project.index.root.clone(), &rel, whole);
            }
            TreeAction::CopyDirPath(dir, whole) => self.copy_path(ctx, &self.project.index.root.clone(), &dir, whole),
            TreeAction::DeleteFolder(dir) => {
                self.confirm = Some((format!("Delete the folder {dir} and everything in it? There is no undo."), vec![dir]));
            }
            TreeAction::RenameFile(i) => {
                let rel = self.project.index.entries[i].rel.clone();
                self.prompt = Some(NamePrompt {
                    title: "Rename".into(),
                    value: rel.clone(),
                    what: NameFor::RenameFile(rel),
                    focus: true,
                });
            }
            TreeAction::DuplicateFile(i) => {
                let rel = self.project.index.entries[i].rel.clone();
                let suggestion = format!("{} copy", rel.trim_end_matches(".png"));
                self.prompt = Some(NamePrompt {
                    title: "Duplicate".into(),
                    value: suggestion,
                    what: NameFor::DuplicateFile(rel),
                    focus: true,
                });
            }
            TreeAction::NewFolder(dir) => {
                self.prompt = Some(NamePrompt {
                    title: "New folder".into(),
                    value: String::new(),
                    what: NameFor::NewFolder(dir),
                    focus: true,
                });
            }
            TreeAction::RenameFolder(dir) => {
                let name = dir.rsplit_once('/').map(|(_, n)| n).unwrap_or(&dir).to_string();
                self.prompt = Some(NamePrompt {
                    title: "Rename folder".into(),
                    value: name,
                    what: NameFor::RenameFolder(dir),
                    focus: true,
                });
            }
            TreeAction::Labels(_, _) => {}
        }
    }
}

impl eframe::App for App {
    /// Keeps the Tab and arrow keys away from egui, which would walk the
    /// focus with them on its own, and Escape, which would drop the focus.
    /// `ui` puts them back into the input, for the handlers of the app. A
    /// dialog or a popup gets them as egui gives them. A text field keeps
    /// Left and Right for its cursor, and Escape to stop typing. It loses Up
    /// and Down, which have nothing to do in one line.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.dialog_open(ctx) {
            return;
        }
        let typing = ctx.text_edit_focused();
        let mut nav = Vec::new();
        raw_input.events.retain(|e| {
            let egui::Event::Key { key, modifiers, .. } = e else { return true };
            match key {
                Key::Tab => nav.push(e.clone()),
                Key::ArrowUp | Key::ArrowDown if typing && modifiers.is_none() => {}
                Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Escape if !typing => nav.push(e.clone()),
                _ => return true,
            }
            false
        });
        self.nav.extend(nav);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.receive_label();
        if self.library_batch.tick(ctx, &self.library.index.root, &self.keys) {
            let root = self.library_batch.root.clone();
            let labels = self.library_batch.import();
            self.apply_labels(&root, &labels);
        }
        // A right click first closes any open menu. egui closes a menu on any
        // click while it is open, and it does that after the same click has
        // opened the new menu; without this every second right click shows
        // nothing. The press comes a frame before the click that opens.
        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
            egui::Popup::close_all(ctx);
        }
        // The keys that `raw_input_hook` kept from egui go back into the
        // input, for the handlers below. Stops from the last frame are in
        // `self.stops`; this frame collects them anew.
        let nav = std::mem::take(&mut self.nav);
        ctx.input_mut(|i| i.events.extend(nav));
        ctx.data_mut(|d| d.remove_temp::<Vec<((Panel, Spot), Id)>>(Id::new("stops")));
        self.handle_keys(ctx);
        // The preview sets this flag while drawing; clear it first, so that
        // a closed preview does not keep it.
        for s in [&mut self.library.sheet, &mut self.project.sheet].into_iter().flatten() {
            s.preview_hovered = false;
        }

        // Which pane holds the keys. Read once, after the keys of this frame
        // moved them, so that every title agrees.
        if let Some(at) = ctx.memory(|m| m.focused()).and_then(|id| self.pane_of(id)) {
            self.pane = at;
            if at.1 != Spot::Status {
                self.active = at.0;
            }
        }
        let keys = self.pane;
        // A tree draws its cursor only while the keys are on its rows.
        let on_rows = (ctx.memory(|m| m.has_focus(library_tree_id())), ctx.memory(|m| m.has_focus(project_tree_id())));
        let mut library_action = None;
        let mut project_action = None;
        // A click in an empty pane asks for that side's folder.
        let (library_set, project_set) = (self.library.is_set(), self.project.is_set());
        let mut ask: Option<Panel> = None;
        let mut hover_dir: Option<String> = None;
        let mut library_rows: Vec<tree::Row> = Vec::new();
        let mut project_rows: Vec<tree::Row> = Vec::new();
        let mut delete_in_mine = false;
        let mut create = false;
        if self.settings.hide_legend {
            self.status_bar(ctx, ui);
        }
        egui::Panel::left("left").resizable(true).default_size(340.0).size_range(240.0..=800.0).show(ui, |ui| {
            ui.add_space(4.0);
            // One row: the filter button at the left, the box in the rest.
            set_pane(ui, (Panel::Library, Spot::Tree));
            ui.horizontal(|ui| {
                // What the search matches on, in a popup under the button.
                let r = ui.small_button("☰").on_hover_text("search in…");
                stop(&r);
                egui::Popup::from_toggle_button_response(&r).show(|ui| {
                    ui.set_min_width(220.0);
                    ui.strong("Search in");
                    let first = ui.checkbox(&mut self.settings.search.folders, "folder names");
                    let second = ui.checkbox(&mut self.settings.search.files, "file names");
                    let third = ui.checkbox(&mut self.settings.search.captions, "captions");
                    let fourth = ui.checkbox(&mut self.settings.search.tags, "tags");
                    let changed = [&first, &second, &third, &fourth].iter().any(|r| r.changed());
                    popup_keys(ui, &r, &first);
                    if changed {
                        self.refresh_query();
                        if let Err(e) = self.settings.save() { self.status = e; }
                    }
                    ui.weak("Words match by prefix. All words must match.\nCaptions and tags come from Label with AI.");
                });
                let r = egui::TextEdit::singleline(&mut self.query)
                    .id(search_id())
                    .hint_text("search: rock wall")
                    .desired_width(ui.available_width())
                    .show(ui)
                    .response;
                stop(&r);
                if r.changed() {
                    self.refresh_query();
                }
            });
            ui.add_space(4.0);
            if !self.settings.hide_legend {
                egui::Panel::bottom("legend").show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    let ai_key = if AI_VISIBLE { "i: AI assist panel | " } else { "" };
                    // The only manual anyone reads, and read once. It holds
                    // what a person cannot guess and needs at once, in the
                    // order they need it. Everything a tooltip already says
                    // stays out of it, and so does every key that can wait.
                    let legend = format!(
                        "click and drag: select tiles | click and hold: lift and move (ctrl: copy) | \
                         ctrl+c, ctrl+v: copy/paste | drag an edge of the selection: resize it | right click: clear it, or delete inside it | \
                         ctrl+tab: next panel | tab: next field | arrows: move the selection, shift: extend | \
                         {ai_key}ctrl+f: search | ctrl+wheel: zoom | ctrl+z, ctrl+y: undo, redo | ctrl+s: save"
                    );
                    let text = egui::RichText::new(legend).weak();
                    if ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text("click: hide the legend").clicked() {
                        self.legend_prompt = true;
                    }
                });
            }
            egui::Panel::top("library tree")
                .resizable(true)
                .default_size(ui.available_height() * if project_set { 0.6 } else { 0.45 })
                .size_range(80.0..=f32::INFINITY)
                .show(ui, |ui| {
                    ui.label(title_text("LIBRARY", keys == (Panel::Library, Spot::Tree)));
                    egui::ScrollArea::vertical().id_salt("library scroll").auto_shrink([false, false]).show(ui, |ui| {
                        // The whole visible area answers, before the tree
                        // draws: the files and folders lie on top of it, so
                        // every place that is not one of them is free space.
                        let bg = ui.interact(ui.clip_rect(), library_tree_id(), egui::Sense::click());
                        stop(&bg);
                        if !library_set {
                            ui.weak("No library folder yet.");
                            ui.add_space(4.0);
                            ui.weak(
                                "This is your library of tilesheets and packs. I'll help you browse and search them, and to transfer what you \
                                 need into your own tilesheets. I'll track details about your assets in a tilepicky.json.",
                            );
                            ui.add_space(6.0);
                            ui.weak("Click here to choose it.");
                            if bg.clicked() {
                                ask = Some(Panel::Library);
                            }
                        }
                        let view = tree::View {
                            visible: self.library.visible.as_deref(),
                            selected: self.library.sel,
                            marked: None,
                            query: &self.qwords,
                            apply_query: self.open_trees,
                            menus: false,
                            scroll_to: self.library.scroll.as_ref(),
                            cursor: on_rows.0.then_some(self.library.at.as_ref()).flatten(),
                            open_dir: self.library.open_dir.as_ref().map(|(d, o)| (d.as_str(), *o)),
                            sweeping: false,
                            lifting: false,
                            entries: &self.library.index.entries,
                        };
                        library_action = self.library.tree.show(ui, &view, "", &mut Vec::new(), &mut library_rows, &mut None);
                        claim_on_press(ui, library_tree_id());
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
                set_pane(ui, (Panel::Project, Spot::Tree));
                let title = title_text("PROJECT", keys == (Panel::Project, Spot::Tree));
                let heading = ui.add(egui::Label::new(title).sense(egui::Sense::click()));
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
                    let new = ui.button("New");
                    if new.clicked() {
                        create = true;
                    }
                    let r = egui::TextEdit::singleline(&mut self.new_name)
                        .id(new_name_id())
                        .hint_text("new tilesheet name")
                        .desired_width(ui.available_width())
                        .show(ui)
                        .response;
                    // The button draws first, at the right; the field comes
                    // first when you read.
                    stop(&r);
                    stop(&new);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        create = true;
                    }
                });
                }
                egui::ScrollArea::vertical().id_salt("project scroll").auto_shrink([false, false]).show(ui, |ui| {
                    // The free space around the tree offers the folder menu.
                    let bg = ui.interact(ui.clip_rect(), project_tree_id(), egui::Sense::click());
                    stop(&bg);
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
                        visible: self.project.visible.as_deref(),
                        selected: self.project.sel,
                        marked: Some(&self.marked),
                        query: &self.qwords,
                        apply_query: self.open_trees,
                        menus: true,
                        scroll_to: self.project.scroll.as_ref(),
                        cursor: on_rows.1.then_some(self.project.at.as_ref()).flatten(),
                        open_dir: self.project.open_dir.as_ref().map(|(d, o)| (d.as_str(), *o)),
                        sweeping: self.sweep.is_some(),
                        lifting: self.file_drag.is_some(),
                        entries: &self.project.index.entries,
                    };
                    // The tree area itself is the root folder; the tree names
                    // a folder inside it when the pointer is over one.
                    if self.file_drag.is_some() && bg.contains_pointer() {
                        hover_dir = Some(String::new());
                    }
                    project_action = self.project.tree.show(ui, &view, "", &mut Vec::new(), &mut project_rows, &mut hover_dir);
                    claim_on_press(ui, project_tree_id());
                    bg.context_menu(|ui| {
                        let label = if project_set { "Change project folder…" } else { "Set project folder…" };
                        if ui.button(label).clicked() {
                            ask = Some(Panel::Project);
                            ui.close();
                        }
                        if project_set {
                            if ui.button("New folder…").clicked() {
                                self.prompt =
                                    Some(NamePrompt { title: "New folder".into(), value: String::new(), what: NameFor::NewFolder(String::new()), focus: true });
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
        if !self.settings.hide_legend {
            self.status_bar(ctx, ui);
        }
        self.open_trees = false;
        self.library.scroll = None;
        self.project.scroll = None;
        self.library.open_dir = None;
        self.project.open_dir = None;
        // The files of the PROJECT tree, in the order it shows them: what a
        // marked group runs over.
        let project_order: Vec<usize> = project_rows.iter().filter_map(|r| if let tree::Row::File(i) = r { Some(*i) } else { None }).collect();
        self.tree_keys(ctx, &library_rows, &project_rows, &project_order);
        if let Some(action) = library_action {
            self.library_tree_action(ctx, action);
        }
        if let Some(action) = project_action {
            self.project_tree_action(ctx, action, &project_order);
        }
        // The sweep ends with the button, after this frame's marks are in;
        // clearing it earlier would let the last step mark one file only.
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.sweep = None;
        }
        self.drop_files(ctx, hover_dir);
        if create {
            self.request(ctx, Pending::Create);
        }
        self.dialogs(ctx);

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
            set_pane(ui, (Panel::Library, Spot::Sheet));
            ui.horizontal(|ui| {
                let live = keys == (Panel::Library, Spot::Sheet);
                let ai = AI_VISIBLE.then_some(&mut self.ai_panel);
                let clicked;
                (library_tile, clicked) = Self::sheet_header(ui, "Source", live, true, self.library.sheet.as_mut(), ai, None);
                // A header button does not keep the keys: they go to the grid.
                if clicked {
                    self.active = Panel::Library;
                    if self.library.sheet.is_some() { ctx.memory_mut(|m| m.request_focus(library_id())); }
                }
            });
            if self.ai_panel {
                let label = egui::Panel::right("library assist").resizable(true).default_size(260.0).show(ui, |ui| {
                    set_pane(ui, (Panel::Library, Spot::Side));
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let out = Self::assist_panel(ui, &self.settings.ai, &self.keys, self.library.sheet.as_ref(),
                            self.label_run.as_ref(), self.label_outcome.as_deref());
                        self.library_batch.ui(ui, &self.library.index, &self.settings.ai, &self.keys);
                        out
                    }).inner
                });
                if let Some(action) = label.inner { self.label_action(ctx, action); }
            }
            if let Some(s) = &mut self.library.sheet {
                if s.anim_panel {
                    egui::Panel::right("library animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        set_pane(ui, (Panel::Library, Spot::Side));
                        library_anim = Self::animation_panel(ui, s, true, keys == (Panel::Library, Spot::Side));
                    });
                }
                set_pane(ui, (Panel::Library, Spot::Sheet));
                stop_id(ui.ctx(), library_id());
                let out = egui::CentralPanel::default().show(ui, |ui| s.view(ui, library_id(), dragging, false, false, keys == (Panel::Library, Spot::Sheet)));
                let ev = out.inner;
                if let Some(action) = ev.labels { self.label_action(ctx, action); }
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
                        "Open a sheet on the left, or press Ctrl+F to search."
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
        if let Some(state) = egui::PanelState::load(ctx, panel_id) && total > 0.0 {
            self.split = (state.outer_rect.height() / total).clamp(0.1, 0.9);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.project_rect = ui.max_rect();
            set_pane(ui, (Panel::Project, Spot::Sheet));
            let live = keys == (Panel::Project, Spot::Sheet);
            let clicked;
            (project_tile, clicked) = Self::sheet_header(ui, "Canvas", live, false, self.project.sheet.as_mut(), None, Some(&mut self.project_eye));
            if clicked {
                self.active = Panel::Project;
                if self.project.sheet.is_some() { ctx.memory_mut(|m| m.request_focus(project_id())); }
            }
            let eye = self.project_eye;
            if let Some(s) = &mut self.project.sheet {
                if s.anim_panel {
                    egui::Panel::right("my animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        set_pane(ui, (Panel::Project, Spot::Side));
                        anim_changed = Self::animation_panel(ui, s, false, keys == (Panel::Project, Spot::Side));
                    });
                }
                set_pane(ui, (Panel::Project, Spot::Sheet));
                stop_id(ui.ctx(), project_id());
                let out = egui::CentralPanel::default().show(ui, |ui| s.view(ui, project_id(), dragging, true, eye, keys == (Panel::Project, Spot::Sheet)));
                let ev = out.inner;
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
                    s.clear_selection();
                    delete_in_mine = true;
                }
            } else {
                // The same frame as the library pane, so both hints sit alike.
                egui::CentralPanel::default().show(ui, |ui| {
                    let hint = if project_set {
                        "Create or open a tilesheet on the left. Then select tiles in the library, Ctrl+C, click a tile here, Ctrl+V."
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
            if let Some(s) = &self.project.sheet {
                self.status = format!("resized to {}x{} tiles", s.cols(), s.rows());
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
        // The stops of this frame, in reading order: pane by pane, and in
        // each pane in the order they drew. The next frame's Tab walks them.
        let mut stops = ctx.data_mut(|d| d.remove_temp::<Vec<((Panel, Spot), Id)>>(Id::new("stops"))).unwrap_or_default();
        stops.sort_by_key(|(p, _)| PANES.iter().position(|q| q == p));
        let mut seen = HashSet::new();
        stops.retain(|(_, id)| seen.insert(*id));
        self.stops = stops;
        self.library.rows = library_rows;
        self.project.rows = project_rows;
        // When nothing holds the keys, they go back to the body of the pane
        // in use: after Escape or Enter in a text field, and after a click
        // on something that does not take the keys itself. This waits for
        // the end of the frame, so that the field sees its Escape first.
        if !self.dialog_open(ctx) && ctx.memory(|m| m.focused()).is_none() && let Some(id) = self.body(self.pane) {
            ctx.memory_mut(|m| m.request_focus(id));
            ctx.request_repaint();
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
/// The row `dir` steps away from `from`, over folders and files alike.
fn walk_rows(rows: &[tree::Row], from: Option<&tree::Row>, dir: i32) -> Option<tree::Row> {
    if rows.is_empty() || dir == 0 {
        return None;
    }
    let at = from.and_then(|c| rows.iter().position(|x| x == c));
    let next = match at {
        Some(p) => (p as i32 + dir).clamp(0, rows.len() as i32 - 1) as usize,
        None if dir > 0 => 0,
        None => rows.len() - 1,
    };
    Some(rows[next].clone())
}

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
    /// The field holds one number, kept in both halves of the pair. The
    /// wheel moves it as the drag does, instead of the second number.
    single: bool,
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
            // The text box wears the button's own name. Without that the
            // button stops drawing the moment you type in it, egui drops the
            // focus of a widget that has gone, and the keys go back to the
            // body of the pane. With one name, Tab also walks on from the
            // text box as it does from the button.
            let mut box_ = egui::TextEdit::singleline(buf).desired_width(56.0);
            if let Some((id, _)) = ui.data(|d| d.get_temp::<(egui::Id, egui::Rect)>(self.id.with("widget"))) {
                box_ = box_.id(id);
            }
            let r = ui.add(box_);
            stop(&r);
            if edit.focus {
                r.request_focus();
                edit.focus = false;
            }
            // The button comes back in the next frame, so ask for one.
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                edit.text = None;
                shut_text_box(ui, r.id);
                ui.ctx().request_repaint();
                return None;
            }
            if r.lost_focus() {
                let v = (self.parse)(buf);
                edit.text = None;
                shut_text_box(ui, r.id);
                ui.ctx().request_repaint();
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
        stop(&r);
        // The text box takes this name when it opens.
        ui.data_mut(|d| d.insert_temp(self.id.with("widget"), (r.id, r.rect)));
        let mut new = value;
        if r.hovered() {
            let wheel = field_wheel(ui);
            if wheel != 0 {
                let axis = usize::from(!self.single);
                new[axis] = (self.step)(new[axis], wheel);
                if self.single {
                    new[1] = new[0];
                }
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
        single: false,
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
        single: false,
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
        single: false,
    }
}

/// Forgets that a name once belonged to a text box. The button and its text
/// box share one name, so that the keys never fall through the gap between
/// them, and egui answers "a text box holds the keys" for as long as a text
/// box was ever opened under that name. The app then hands every key to
/// typing that is no longer happening.
fn shut_text_box(ui: &egui::Ui, id: Id) {
    ui.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(id));
}

/// The time one frame is on screen. One number, so the drag and the wheel
/// move the same thing. It is a field like the others on purpose: a
/// `DragValue` turns into a text box the moment it holds the keys, which
/// swallows every shortcut and leaves nowhere to walk to.
fn ms_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("ms field", library)),
        px_per_step: 2.0,
        unit: " ms",
        hover: "drag or scroll: the time one frame is on screen   click: type",
        step: |from, steps| (from as i32 + steps * 5).clamp(1, 5000) as u32,
        show: |v| format!("{}", v[0]),
        parse: parse_ms,
        linked: |_| true,
        single: true,
    }
}

/// "100", or "100 ms".
fn parse_ms(text: &str) -> Option<[u32; 2]> {
    let n: u32 = text.trim().trim_end_matches("ms").trim().parse().ok()?;
    (1..=5000).contains(&n).then_some([n, n])
}

/// The size of one cell of an animation, in pixels. It stands in for the
/// field below when the frames are not a whole number of tiles, so that the
/// panel never shows a rounded number as if it were the truth. It wears the
/// same name, because it is the same place to the keyboard.
fn frame_px_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("cell field", library)),
        px_per_step: 3.0,
        unit: " px",
        hover: "drag: the width in pixels   scroll: the height   click: type, 32 or 32x48",
        step: |from, steps| (from as i32 + steps).clamp(1, 1024) as u32,
        show: show_cells,
        parse: parse_tile,
        linked: |_| false,
        single: false,
    }
}

/// The size of one frame of an animation, in cells. Both sides always show,
/// because a frame that is not square is as usual as one that is.
fn cell_field(library: bool) -> PairField {
    PairField {
        id: egui::Id::new(("cell field", library)),
        px_per_step: 12.0,
        unit: " tiles",
        hover: "drag: the width in tiles   scroll: the height   click: type, 2 or 2x1",
        step: |from, steps| (from as i32 + steps).clamp(1, 256) as u32,
        show: show_cells,
        parse: parse_cells,
        linked: |_| false,
        single: false,
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
            if let egui::Event::MouseWheel { delta, modifiers, .. } = e && !modifiers.ctrl {
                steps -= sig(delta.y);
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

/// "2x1": a frame size keeps both sides, so that nothing has to be guessed.
fn show_cells(c: [u32; 2]) -> String {
    format!("{}x{}", c[0], c[1])
}

/// "2" is a cell of two tiles by two; "2x1" is two tiles wide and one high.
fn parse_cells(text: &str) -> Option<[u32; 2]> {
    let text = text.trim().trim_end_matches("tiles").trim();
    let ok = |n: u32| (1..=256).contains(&n);
    if let Some((w, h)) = text.split_once(['x', 'X']) {
        let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        (ok(w) && ok(h)).then_some([w, h])
    } else {
        let n = text.parse().ok()?;
        ok(n).then_some([n, n])
    }
}

/// The version as a screenshot should carry it: the first two numbers, and
/// the word of a pre-release when there is one. A picture of the window says
/// `0.2rc` while 0.2 is still on its way, and `0.2` once it has arrived,
/// without anyone editing this.
fn short_version() -> String {
    let v = env!("CARGO_PKG_VERSION");
    let (number, pre) = v.split_once('-').map_or((v, ""), |(n, p)| (n, p));
    let mut parts = number.split('.');
    let short = match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        _ => number.to_string(),
    };
    let tag: String = pre.chars().take_while(char::is_ascii_alphabetic).collect();
    format!("{short}{tag}")
}

/// Whether this build can draw with wgpu as well as with OpenGL. Only
/// OpenGL is built by default: wgpu is half the compile time of the whole
/// tool, and a person installing with `cargo install` waits for it.
const WGPU: bool = cfg!(feature = "wgpu");

fn main() -> eframe::Result {
    let mut dirs: Vec<String> = Vec::new();
    let mut renderer = eframe::Renderer::default();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--help" | "-h" => {
                println!("usage: tilepicky [--glow{}] [<library dir> [<project dir>]]", if WGPU { " | --wgpu" } else { "" });
                println!("Without a folder, the tool asks for one and remembers it.");
                if WGPU {
                    println!("--glow draws with OpenGL, --wgpu with wgpu.");
                }
                return Ok(());
            }
            "--glow" => renderer = eframe::Renderer::Glow,
            #[cfg(feature = "wgpu")]
            "--wgpu" => renderer = eframe::Renderer::Wgpu,
            _ => dirs.push(a),
        }
    }
    if dirs.len() > 2 {
        eprintln!("usage: tilepicky [--glow{}] [<library dir> [<project dir>]]", if WGPU { " | --wgpu" } else { "" });
        std::process::exit(2);
    }
    let (mut settings, damaged) = settings::Settings::load();
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
    if let Err(e) = settings.save() { eprintln!("{e}"); }
    let icon = image::load_from_memory(include_bytes!("../icon.png")).expect("icon.png").to_rgba8();
    let icon = egui::IconData {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
    };
    let options = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_title(format!("Tilepicky {}", short_version()))
            .with_icon(icon)
            .with_app_id("tilepicky"),
        ..Default::default()
    };
    eframe::run_native(
        "tilepicky",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(App::new(settings, damaged)))
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

    use crate::storage::tests::Folder;
    use image::{Rgba, RgbaImage};

    /// A library and a project on disk, and the app that shows them. No
    /// test here writes the settings, which live in the user's own folder.
    struct Bench {
        ctx: egui::Context,
        app: App,
        library: Folder,
        project: Folder,
    }

    fn sheet_file(dir: &Path, rel: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        RgbaImage::from_pixel(16, 16, Rgba([90, 120, 30, 255])).save(path).unwrap();
    }

    fn bench(library: &[&str], project: &[&str]) -> Bench {
        let (lib, proj) = (Folder::new(), Folder::new());
        library.iter().for_each(|rel| sheet_file(&lib.0, rel));
        project.iter().for_each(|rel| sheet_file(&proj.0, rel));
        let mut settings = settings::Settings::default();
        settings.library.path = Some(lib.0.clone());
        settings.project.path = Some(proj.0.clone());
        Bench { ctx: egui::Context::default(), app: App::new(settings, None), library: lib, project: proj }
    }

    impl Bench {
        fn project_rel(&self) -> Option<&str> {
            self.app.project.sheet.as_ref().map(|s| s.rel.as_str())
        }
        /// The index entry the project side says is open.
        fn project_sel_rel(&self) -> Option<&str> {
            self.app.project.sel.map(|i| self.app.project.index.entries[i].rel.as_str())
        }
        fn open_project(&mut self, rel: &str) {
            let i = self.app.project.index.position(rel).unwrap();
            self.app.open_project(&self.ctx, i);
        }
    }

    #[test]
    fn a_damaged_settings_file_waits_for_the_user() {
        let app = App::new(settings::Settings::default(), Some("Cannot read settings.json".into()));
        assert!(matches!(app.damaged.first(), Some(Damaged::Settings(e)) if e == "Cannot read settings.json"));
        assert!(app.dialog_open(&egui::Context::default()));
    }

    #[test]
    fn the_open_sheet_keeps_its_place_through_a_rescan() {
        let mut b = bench(&["a.png", "c.png"], &[]);
        let i = b.app.library.index.position("c.png").unwrap();
        b.app.open_library(&b.ctx, i);
        assert_eq!(b.app.library.sel, Some(1));
        assert!(b.app.library.sheet.as_ref().unwrap().library, "a library sheet is a source");
        assert_eq!(b.app.library.at, Some(tree::Row::File(1)));
        assert!(b.app.active == Panel::Library);
        sheet_file(&b.library.0, "b.png");
        b.app.rescan_library();
        assert_eq!(b.app.library.sel, Some(2));
        assert_eq!(b.app.library.sheet.as_ref().unwrap().rel, "c.png");
    }

    #[test]
    fn opening_a_project_file_moves_the_cursor_to_it() {
        let mut b = bench(&[], &["a.png", "b.png"]);
        b.open_project("b.png");
        assert_eq!(b.project_rel(), Some("b.png"));
        assert!(!b.app.project.sheet.as_ref().unwrap().library, "a tilesheet is no source");
        assert_eq!((b.app.project.sel, b.app.tree_cursor), (Some(1), Some(1)));
        assert_eq!(b.app.project.at, Some(tree::Row::File(1)));
        assert!(b.app.active == Panel::Project);
    }

    #[test]
    fn renames_take_the_open_sheet_along() {
        let mut b = bench(&[], &["pack/tree.png", "rock.png"]);
        b.open_project("pack/tree.png");
        b.app.apply_name(&b.ctx, &NameFor::RenameFolder("pack".into()), "plants").unwrap();
        assert_eq!(b.project_rel(), Some("plants/tree.png"));
        assert_eq!(b.project_sel_rel(), Some("plants/tree.png"));
        b.app.apply_name(&b.ctx, &NameFor::RenameFile("plants/tree.png".into()), "plants/oak").unwrap();
        assert_eq!(b.project_rel(), Some("plants/oak.png"));
        assert_eq!(b.project_sel_rel(), Some("plants/oak.png"));
        assert!(b.project.0.join("plants/oak.png").is_file());
    }

    #[test]
    fn deleting_the_open_file_closes_it() {
        let mut b = bench(&[], &["a.png", "b.png"]);
        b.open_project("b.png");
        b.app.delete_paths(&["a.png".into()]).unwrap();
        assert_eq!(b.project_sel_rel(), Some("b.png"));
        b.app.delete_paths(&["b.png".into()]).unwrap();
        assert!(b.app.project.sheet.is_none());
        assert!(b.app.project.index.entries.is_empty());
    }

    #[test]
    fn a_new_sheet_never_replaces_one_or_leaves_the_project() {
        let mut b = bench(&[], &["tree.png"]);
        let before = std::fs::read(b.project.0.join("tree.png")).unwrap();
        for name in ["tree", "tree.png", "../outside"] {
            b.app.new_name = name.into();
            b.app.create_project(&b.ctx);
            assert!(b.app.project.sheet.is_none(), "{name}");
        }
        assert_eq!(std::fs::read(b.project.0.join("tree.png")).unwrap(), before);
        assert!(!b.project.0.parent().unwrap().join("outside.png").exists());
        b.app.new_name = "pack/fresh".into();
        b.app.create_project(&b.ctx);
        assert_eq!(b.project_rel(), Some("pack/fresh.png"));
        assert_eq!(b.project_sel_rel(), Some("pack/fresh.png"));
        assert!(b.app.new_name.is_empty());
    }

    #[test]
    fn unsaved_changes_hold_an_open_until_asked() {
        let mut b = bench(&[], &["a.png", "b.png", "c.png"]);
        b.open_project("a.png");
        b.app.project.sheet.as_mut().unwrap().dirty = true;
        b.app.request(&b.ctx, Pending::Open(1));
        assert!(b.app.pending == Some(Pending::Open(1)));
        assert_eq!(b.project_rel(), Some("a.png"));
        // A duplicate opens as any file does: it waits too.
        b.app.pending = None;
        b.app.apply_name(&b.ctx, &NameFor::DuplicateFile("c.png".into()), "d").unwrap();
        assert!(b.project.0.join("d.png").is_file());
        assert_eq!(b.project_rel(), Some("a.png"));
        assert!(b.app.pending.is_some());
        // Without changes, it opens at once.
        b.app.project.sheet.as_mut().unwrap().dirty = false;
        b.app.pending = None;
        b.app.apply_name(&b.ctx, &NameFor::DuplicateFile("c.png".into()), "e").unwrap();
        assert_eq!(b.project_rel(), Some("e.png"));
    }

    /// A GIF would keep one frame and a JPEG no alpha, so a sheet from
    /// another format saves as a PNG beside it.
    #[test]
    fn a_sheet_that_is_no_png_saves_as_one() {
        let mut b = bench(&[], &["walk.gif"]);
        let before = std::fs::read(b.project.0.join("walk.gif")).unwrap();
        b.open_project("walk.gif");
        b.app.project.sheet.as_mut().unwrap().dirty = true;
        b.app.save();
        let prompt = b.app.prompt.take().unwrap();
        assert!(prompt.what == NameFor::SaveAs);
        assert_eq!(prompt.value, "walk.png");
        assert_eq!(std::fs::read(b.project.0.join("walk.gif")).unwrap(), before);
        assert!(b.app.project.sheet.as_mut().unwrap().save().is_err());
        b.app.apply_name(&b.ctx, &prompt.what, &prompt.value).unwrap();
        assert_eq!(b.project_rel(), Some("walk.png"));
        assert!(b.project.0.join("walk.png").is_file());
        assert_eq!(std::fs::read(b.project.0.join("walk.gif")).unwrap(), before);
    }

    #[test]
    fn labels_reach_the_index_the_open_sheet_and_the_search() {
        let mut b = bench(&["a.png", "b.png"], &[]);
        let i = b.app.library.index.position("b.png").unwrap();
        b.app.open_library(&b.ctx, i);
        b.app.query = "mossy".into();
        b.app.refresh_query();
        assert_eq!(b.app.library.visible, Some(vec![false, false]));
        let label = sidecar::Label { provider: "p".into(), model: "m".into(), status: sidecar::Status::Labeled,
            caption: "Mossy stones".into(), tags: vec![] };
        let root = b.library.0.clone();
        b.app.apply_labels(&root, &[("b.png".into(), Some(label.clone()))]);
        assert_eq!(b.app.library.index.entries[1].side.label.as_ref(), Some(&label));
        assert_eq!(b.app.library.sheet.as_ref().unwrap().side.label.as_ref(), Some(&label));
        assert_eq!(b.app.library.visible, Some(vec![false, true]));
    }

    #[test]
    fn tab_walks_the_stops_and_wraps() {
        let [a, b, c] = [Id::new("a"), Id::new("b"), Id::new("c")];
        let stops = [a, b, c];
        assert_eq!(next_stop(&stops, Some(a), None, 1), Some(b));
        assert_eq!(next_stop(&stops, Some(c), None, 1), Some(a));
        assert_eq!(next_stop(&stops, Some(a), None, -1), Some(c));
        // A place that is not a stop starts the walk at the body of the pane.
        assert_eq!(next_stop(&stops, Some(Id::new("x")), Some(b), 1), Some(b));
        assert_eq!(next_stop(&stops, None, None, 1), Some(a));
        assert_eq!(next_stop(&[], Some(a), None, 1), None);
    }
}
