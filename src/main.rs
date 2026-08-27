// SPDX-License-Identifier: GPL-3.0-only
//! Tilepicky: browse a large set of sheets, search them, and copy
//! cells into tilesheets of your own.
//!
//! Usage: `tilepicky [<library dir> [<project dir>]]`

mod ai;
mod index;
mod settings;
mod sheet;
mod sidecar;
mod tree;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, TextureHandle, Vec2};

/// The AI features show in the UI only when this is `true`. The code
/// (`src/ai.rs`, the assist panel, the batch popup) stays either way; this
/// flag only hides the buttons and the settings section for a release
/// before the features are finished.
const AI_VISIBLE: bool = false;
/// The library panel's eye shows only the whole-sheet tooltip today, since
/// a library sheet carries no provenance; that is little enough to hide it
/// for a release. The project panel's eye, which shows provenance, stays.
const LIBRARY_EYE_VISIBLE: bool = false;

/// A sheet's tile size, gap, and offset, as the header fields edit them.
type Grid = ([u32; 2], [u32; 2], [i32; 2]);
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
    /// The settings popup was open at the last frame; both files are
    /// written when it closes.
    config_open: bool,
    /// The legend asks whether to hide itself.
    legend_prompt: bool,
    /// The eye of each sheet panel: tooltips and islands, no editing. Off
    /// at each start; a thing to switch on for a moment.
    library_eye: bool,
    project_eye: bool,
    /// A folder dialog is open for this side; the answer arrives on the channel.
    picking: Option<(Panel, std::sync::mpsc::Receiver<Option<PathBuf>>)>,
    /// A drag across the files started here and marks a group while it lasts.
    sweep: Option<usize>,
    /// Files held in the air, waiting for a folder to land in.
    file_drag: Option<Vec<String>>,
    /// A file the arrow keys moved to, to bring into view next frame.
    library_scroll: Option<tree::Row>,
    project_scroll: Option<tree::Row>,
    /// The row the arrow keys stand on in each tree, folders included.
    library_at: Option<tree::Row>,
    project_at: Option<tree::Row>,
    /// The rows each tree showed when it last drew, in reading order.
    library_rows: Vec<tree::Row>,
    project_rows: Vec<tree::Row>,
    /// A folder the arrow keys opened or closed, applied on the next draw.
    library_open_dir: Option<(String, bool)>,
    project_open_dir: Option<(String, bool)>,
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
    /// Which pane holds the keyboard. The panes own the keyboard, so this
    /// always names one of them, and a focus that goes astray comes back
    /// here.
    pane: (Panel, Spot),
    /// Where each pane lay when it last drew. A focused widget inside one of
    /// these belongs to that pane.
    pane_rects: Vec<((Panel, Spot), egui::Rect)>,
    /// Every place the arrow keys can stand, with the pane it belongs to and
    /// where it lay when it last drew. The arrows walk these, and only ever
    /// within one pane: leaving a pane is Tab's work, never an arrow's.
    stops: Vec<((Panel, Spot), Id, egui::Rect)>,
    /// Where the keys last stood inside each pane, so that Tab gives a pane
    /// back as you left it.
    inner: Vec<((Panel, Spot), Id)>,

    split: f32,
    /// The status as last shown, and when it changed; it fades after a while.
    shown_status: String,
    status_at: std::time::Instant,
}

/// The panes of one half of the window, from left to right. Tab walks them
/// and wraps; Shift+Tab swaps the halves and stays in the same pane.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Spot {
    Tree,
    Sheet,
    /// The animation panel, which is there only while it is open.
    Side,
    /// The status bar along the foot of the window, with the gear on it. It
    /// belongs to neither half, so it rides with the project half and comes
    /// last in the walk, which is where it lies.
    Status,
    /// A dialog standing in front of the window. While one is open it is the
    /// whole world of the arrow keys, and Tab does nothing at all. Only one
    /// can be open at a time, so one name is enough for all of them.
    Dialog,
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
fn place_name(app: &App, ctx: &egui::Context, id: Id) -> String {
    let named = [
        (library_tree_id(), "library tree"),
        (project_tree_id(), "project tree"),
        (library_id(), "library grid"),
        (project_id(), "project grid"),
        (tree_heading_id(true), "LIBRARY title"),
        (tree_heading_id(false), "PROJECT title"),
        (heading_id(true), "Source title"),
        (heading_id(false), "Canvas title"),
        (search_id(), "search"),
        (new_name_id(), "new name"),
    ];
    let name = named.iter().find(|(k, _)| *k == id).map(|(_, n)| (*n).to_string());
    let pane = match app.spot_of(ctx, id) {
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
        Some(id) => place_name(app, ctx, id),
        None => "nothing".into(),
    };
    eprintln!("[keys] {what}: on {here}");
}

/// Takes a Tab away from a text field, and says which way it walks: forward,
/// back, or across to the other half. `handle_keys` never sees these, because
/// typing beats every shortcut while a text field holds the keys.
fn taken_tab(ui: &egui::Ui) -> Option<i32> {
    let take = |m, k| ui.input_mut(|i| i.consume_key(m, k));
    if take(Modifiers::COMMAND, Key::Tab) {
        Some(0)
    } else if take(Modifiers::SHIFT, Key::Tab) {
        Some(-1)
    } else if take(Modifiers::NONE, Key::Tab) {
        Some(1)
    } else {
        None
    }
}

fn library_id() -> Id {
    Id::new("library sheet")
}
fn project_id() -> Id {
    Id::new("project sheet")
}
/// The free space behind each file tree. It is a widget of its own, so it can
/// hold the keyboard focus: with it, the arrows walk the tree; with a sheet
/// focused, they move that sheet's selection.
/// The places of an open dialog: everything in it, and the button it hangs
/// from. While it is open these are the whole world of the arrow keys, which
/// cannot leave it, and Tab does nothing at all. The button belongs to the
/// dialog because that is the way out: walk to it and press it again.
///
/// Returns whether the dialog should close.
fn dialog_stops(ui: &egui::Ui, button: &egui::Response, places: &[&egui::Response]) -> bool {
    let mut all: Vec<(Id, egui::Rect)> = vec![(button.id, button.rect)];
    all.extend(places.iter().map(|r| (r.id, r.rect)));
    // The keys land in the dialog when it opens, on the first thing in it.
    if !all.iter().any(|(id, _)| ui.memory(|m| m.has_focus(*id))) {
        if let Some(first) = places.first() {
            first.request_focus();
        }
    }
    ui.data_mut(|d| d.insert_temp(Id::new("dialog stops"), all));
    ui.input(|i| i.key_pressed(egui::Key::Escape))
}

/// The gear on the status bar. It writes its own name while it draws.
fn gear_id(ctx: &egui::Context) -> Option<Id> {
    ctx.data(|d| d.get_temp::<Id>(Id::new("gear button")))
}

/// Where Tab lands in an open animation panel: the frame field, which writes
/// its own widget id while it draws.
fn side_id(ctx: &egui::Context, library: bool) -> Option<Id> {
    field_stop(ctx, Id::new(("cell field", library))).map(|(id, _)| id)
}

/// Where a pair field's button lies, once it has drawn at least once.
fn field_stop(ctx: &egui::Context, field: Id) -> Option<(Id, egui::Rect)> {
    ctx.data(|d| d.get_temp::<(Id, egui::Rect)>(field.with("widget")))
}

/// The title of a pane. It says two things at once, and they are not the
/// same thing. The colour says which pane holds the keys: deep blue for it,
/// faint grey for the rest. The ground behind the letters says the keys are
/// standing on the title itself, and not somewhere else in that pane.
///
/// The spaces are always there, so that the title keeps its width when the
/// ground comes and goes.
fn title_text(ui: &egui::Ui, id: Id, title: &str, keys: bool) -> egui::RichText {
    let t = egui::RichText::new(format!(" {title} ")).strong();
    let t = if keys { t.color(egui::Color32::from_rgb(20, 90, 190)) } else { t.color(egui::Color32::from_gray(180)) };
    if ui.memory(|m| m.has_focus(id)) { t.background_color(egui::Color32::from_rgb(214, 230, 250)) } else { t }
}

/// The title of an animation panel. Like the other titles, it is a place,
/// and it is where Tab leaves you the first time you go there.
fn anim_heading_id(library: bool) -> Id {
    Id::new(("animation title", library))
}

/// The field that names a new tilesheet, above the PROJECT tree. Up from the
/// first row of that tree lands here, and Down goes back.
/// The title of a sheet pane. It is a place the keyboard can stand: the row
/// of fields and buttons beside it is reached with Right and Left from here,
/// and the grid below with Down.
/// The title over a file tree, LIBRARY or PROJECT. It is a place to stand,
/// so that the arrow keys walk the left column the way the eye does.
fn tree_heading_id(library: bool) -> Id {
    Id::new(("tree heading", library))
}

/// The search field, at the top of the left column.
fn search_id() -> Id {
    Id::new("search field")
}

fn heading_id(library: bool) -> Id {
    Id::new(("sheet heading", library))
}

fn new_name_id() -> Id {
    Id::new("new tilesheet name")
}

fn library_tree_id() -> Id {
    Id::new("library free space")
}
fn project_tree_id() -> Id {
    Id::new("project free space")
}

impl App {
    fn new(settings: settings::Settings) -> Self {
        let root = |s: &Option<PathBuf>| s.clone().unwrap_or_default();
        let library = Index::scan(&root(&settings.library.path), settings.library.tile.map_or(TILE, Pair::xy));
        let mut project = Index::scan(&root(&settings.project.path), settings.project.tile.map_or(TILE, Pair::xy));
        migrate_sidecars(&mut project);
        Self {
            settings,
            keys: ai::Keys::load(),
            ai_panel: false,
            config_open: false,
            legend_prompt: false,
            library_eye: false,
            project_eye: false,
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
            library_at: None,
            project_at: None,
            library_rows: Vec::new(),
            project_rows: Vec::new(),
            library_open_dir: None,
            project_open_dir: None,
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
            pane: (Panel::Library, Spot::Tree),
            pane_rects: Vec::new(),
            stops: Vec::new(),
            inner: Vec::new(),
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
        self.library_visible = self.library.visible(&self.qwords, self.settings.search);
        self.project_visible = self.project.visible(&self.qwords, self.settings.search);
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

    /// The arrow keys move a cursor over the folders and files of the tree
    /// that holds them. Nothing opens on the way: Enter, or Space, opens the
    /// file the cursor stands on, and Right and Left unfold and fold the
    /// folder it stands on. In the PROJECT tree, Shift and the arrows grow
    /// the marked group over the files.
    ///
    /// The trees hold the arrows until you click a sheet, and a click in a
    /// tree takes them back. Nothing clicked yet leaves them here.
    fn tree_keys(&mut self, ctx: &egui::Context, library_rows: &[tree::Row], project_rows: &[tree::Row], project_order: &[usize]) {
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| id == library_tree_id() || id == project_tree_id()) {
            return;
        }
        // The tree that holds the keys, which is not always the panel in use.
        let library = match focus {
            Some(id) => id == library_tree_id(),
            None => self.active == Panel::Library,
        };
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
        let rows = if library { library_rows } else { project_rows };
        let at = if library {
            self.library_at.clone().or(self.library_sel.map(tree::Row::File))
        } else {
            self.project_at.clone().or(self.tree_cursor.or(self.project_sel).map(tree::Row::File))
        };
        // Right and Left open and close the folder the keys stand on.
        if fold != 0 {
            if let Some(tree::Row::Dir(d)) = &at {
                let open = (d.clone(), fold > 0);
                if library {
                    self.library_open_dir = Some(open);
                } else {
                    self.project_open_dir = Some(open);
                }
            }
            return;
        }
        // Shift walks the files only: a folder has nothing to mark.
        if grow != 0 && !library {
            let from = self.tree_cursor.or(self.project_sel);
            let Some(i) = walk(project_order, from, grow) else { return };
            let a = self.tree_anchor.or(from).unwrap_or(i);
            self.mark_range(project_order, a, i, false);
            self.tree_anchor = Some(a);
            self.tree_cursor = Some(i);
            self.project_at = Some(tree::Row::File(i));
            self.project_scroll = Some(tree::Row::File(i));
            return;
        }
        if enter && step == 0 && grow == 0 {
            if let Some(row) = &at {
                let panel = if library { Panel::Library } else { Panel::Project };
                self.open_row(ctx, panel, &row.clone());
            }
            return;
        }
        let dir = step + grow;
        // At the end of the rows the keys leave the tree for whatever lies
        // that way: the title above it, or the half of the column below.
        let edge = if dir < 0 { rows.first() } else { rows.last() };
        if edge.is_some_and(|r| Some(r) == at.as_ref()) {
            let id = if library { library_tree_id() } else { project_tree_id() };
            self.step_from(ctx, id, (0, dir.signum()));
            return;
        }
        let Some(row) = walk_rows(rows, at.as_ref(), dir) else { return };
        self.stand_on(ctx, if library { Panel::Library } else { Panel::Project }, row);
    }

    /// Puts the arrow keys on a row of a tree. Standing on a file does not
    /// open it: the cursor and the sheet on show are two different things,
    /// and Enter is what joins them.
    fn stand_on(&mut self, _ctx: &egui::Context, panel: Panel, row: tree::Row) {
        if panel == Panel::Library {
            self.library_at = Some(row);
            self.library_scroll = self.library_at.clone();
            self.active = Panel::Library;
            return;
        }
        self.active = Panel::Project;
        self.project_at = Some(row.clone());
        self.project_scroll = self.project_at.clone();
        // A group grows from wherever the cursor stands, so the two agree.
        if let tree::Row::File(i) = row {
            self.tree_anchor = Some(i);
            self.tree_cursor = Some(i);
        }
    }

    /// Enter, or Space, in a tree: it opens the file the cursor stands on,
    /// or unfolds the folder it stands on.
    fn open_row(&mut self, ctx: &egui::Context, panel: Panel, row: &tree::Row) {
        let library = panel == Panel::Library;
        match row {
            tree::Row::Dir(d) => {
                let open = Some((d.clone(), true));
                if library {
                    self.library_open_dir = open;
                } else {
                    self.project_open_dir = open;
                }
            }
            tree::Row::File(i) if library => self.open_library(ctx, *i),
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
        self.library_visible = self.library.visible(&self.qwords, self.settings.search);
        self.library_sel = self.library_sheet.as_ref().and_then(|s| self.library.position(&s.rel));
        self.status = format!("{} files in the library", self.library.entries.len());
    }

    fn rescan_project(&mut self) {
        self.marked.clear();
        self.project = Index::scan(&self.project.root, self.project.tile);
        self.project_tree = Node::build(&self.project.entries.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(), &self.project.dirs);
        self.project_visible = self.project.visible(&self.qwords, self.settings.search);
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

    /// A click on the legend asks before it goes; the settings bring it back.
    fn legend_dialog(&mut self, ctx: &egui::Context) {
        if !self.legend_prompt {
            return;
        }
        let mut done = false;
        let modal = egui::Modal::new(Id::new("legend dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Hide the legend?");
            ui.label("You can show it again in the settings: the gear at the right end of the status line.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Hide").clicked() {
                    self.settings.hide_legend = true;
                    self.settings.save();
                    done = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    done = true;
                }
            });
        });
        if done || modal.should_close() {
            self.legend_prompt = false;
        }
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

    /// Moves or copies one file of the PROJECT tree, with its book entry. The
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
        self.project_visible = self.project.visible(&self.qwords, self.settings.search);
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
            format!("trimmed to {}x{} tiles", sheet.cols(), sheet.rows())
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

    /// The status line: the settings gear at the right, then the text, which
    /// disappears ten seconds after it last changed. With the legend hidden
    /// it runs under the whole window; with the legend shown it stays under
    /// the sheet panels, so the trees keep their height.
    fn status_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, stops: &mut Vec<((Panel, Spot), Id, egui::Rect)>) {
        const STATUS_SECS: u64 = 10;
        if self.status != self.shown_status {
            self.shown_status = self.status.clone();
            self.status_at = std::time::Instant::now();
        }
        let age = self.status_at.elapsed();
        egui::Panel::bottom("status").show_separator_line(true).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let gear = ui.small_button("⚙").on_hover_text("settings (Ctrl+,)");
                stops.push(((Panel::Project, Spot::Status), gear.id, gear.rect));
                ui.data_mut(|d| d.insert_temp(Id::new("gear button"), gear.id));
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
        let mut shut = false;
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
                shut = dialog_stops(ui, gear, &[&first]);
                if AI_VISIBLE {
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                        ai::settings_ui(ui, &mut self.settings.ai, &mut self.keys);
                    });
                }
            });
        if shut {
            egui::Popup::close_id(ui.ctx(), id);
            gear.request_focus();
        }
        let open = egui::Popup::is_id_open(ui.ctx(), id) && !shut;
        if self.config_open && !open {
            self.settings.save();
            self.keys.save();
        }
        self.config_open = open;
    }

    /// A dialog or a popup is up: the keys belong to it, Escape first of all.
    fn dialog_open(&self, ctx: &egui::Context) -> bool {
        self.prompt.is_some() || self.confirm.is_some() || self.pending.is_some() || self.legend_prompt || egui::Popup::is_any_open(ctx)
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        // A dialog standing in front of the window is the whole world while
        // it is open: the arrows walk its places, Tab does nothing, and no
        // command of the window behind it fires.
        if self.dialog_open(ctx) {
            let Some(id) = ctx.memory(|m| m.focused()) else { return };
            if self.spot_of(ctx, id) == Some((Panel::Library, Spot::Dialog)) {
                let dirs = [(Key::ArrowRight, (1, 0)), (Key::ArrowLeft, (-1, 0)), (Key::ArrowDown, (0, 1)), (Key::ArrowUp, (0, -1))];
                let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
                if let Some((_, d)) = dirs.into_iter().find(|(k, _)| key(Modifiers::NONE, *k)) {
                    self.step_from(ctx, id, d);
                }
            }
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
            for s in [self.library_sheet.as_mut(), self.project_sheet.as_mut()].into_iter().flatten() {
                s.end_run();
            }
        }
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
        // The search field, from anywhere. It is the one place the panes do
        // not walk to in a step or two, because it sits above them all.
        if key(cmd, Key::F) {
            self.go(ctx, search_id());
        }
        if key(cmd, Key::Comma) {
            egui::Popup::toggle_id(ctx, Id::new("settings popup"));
        }
        let focus = ctx.memory(|m| m.focused());
        if !focus.is_none_or(|id| self.is_pane(ctx, id)) {
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
        let eye = match self.active {
            Panel::Library => self.library_eye,
            Panel::Project => project_eye,
        };
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
            let from = match self.active {
                Panel::Library => &self.library_sheet,
                Panel::Project => &self.project_sheet,
            };
            if let Some(b) = from.as_ref().and_then(Sheet::copy) {
                self.status = format!("copied {}x{} tiles", b.cols, b.rows);
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
        if in_half && !project_eye && (paste || key(cmd, Key::V)) {
            if let (Some(block), Some(sheet)) = (&self.clip, &mut self.project_sheet) {
                let at = sheet.sel.origin().unwrap_or((0, 0));
                sheet.paste(ctx, at, block);
                self.active = Panel::Project;
                self.after_edit();
            }
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
        if in_half && self.active == Panel::Project && !project_eye {
            if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
                if let Some(sheet) = &mut self.project_sheet {
                    sheet.clear_selection(ctx);
                    self.after_edit();
                }
            }
        }
        if in_half && !eye && key(cmd, Key::A) {
            if let Some(s) = self.sheet_mut(self.active) {
                s.sel = Sel::rect((0, 0), (s.cols() - 1, s.rows() - 1));
            }
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
        if in_half && key(Modifiers::NONE, Key::E) {
            match self.active {
                Panel::Library if LIBRARY_EYE_VISIBLE => self.library_eye = !self.library_eye,
                Panel::Library => {}
                Panel::Project => self.project_eye = !self.project_eye,
            }
        }
        // Tab walks the panes to the right and down, Shift+Tab the other way,
        // and Ctrl+Tab swaps the halves of the window. The most modifiers go
        // first: `consume_key` lets a plain Tab eat a shifted one.
        if key(cmd, Key::Tab) {
            if in_half {
                self.press_tab(ctx, 0);
            }
        } else if key(Modifiers::SHIFT, Key::Tab) {
            self.press_tab(ctx, -1);
        } else if key(Modifiers::NONE, Key::Tab) {
            self.press_tab(ctx, 1);
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
            // A grid against its top row hands the keys up to the title of
            // its own pane, where the header row starts. Every other edge
            // holds them: an arrow never leaves a pane.
            let library = self.active == Panel::Library;
            let at_top = self.sheet_mut(self.active).and_then(|s| s.sel.bounds()).is_some_and(|b| b.y0 == 0);
            match hit {
                Some(((0, -1), Move::Step(false))) if at_top => self.go(ctx, heading_id(library)),
                Some((d, what)) => {
                    if let Some(s) = self.sheet_mut(self.active) {
                        match what {
                            Move::Grow(ctrl) => s.arrow(d, true, ctrl),
                            Move::Step(ctrl) => s.arrow(d, false, ctrl),
                            Move::Whole => s.nudge(d),
                        }
                    }
                }
                None => {}
            }
        }
        // Not on a grid and not in a tree: the arrows walk from place to
        // place, by where each one lies on screen.
        let trees = [library_tree_id(), project_tree_id()];
        if let Some(id) = focus.filter(|id| !sheet_keys && !trees.contains(id)) {
            let dirs = [(Key::ArrowRight, (1, 0)), (Key::ArrowLeft, (-1, 0)), (Key::ArrowDown, (0, 1)), (Key::ArrowUp, (0, -1))];
            if let Some((_, d)) = dirs.into_iter().find(|(k, _)| key(Modifiers::NONE, *k)) {
                self.step_from(ctx, id, d);
            }
        }
        if in_half && key(Modifiers::NONE, Key::Escape) {
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
        if self.project_eye {
            self.status = "the eye is on in your tilesheet; switch it off to drop".into();
            return;
        }
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
        let mut places: Vec<(Id, egui::Rect)> = Vec::new();
        ui.horizontal(|ui| {
            let head = ui.label(title_text(ui, heading_id(library), title, keys));
            ui.interact(head.rect, heading_id(library), egui::Sense::click());
            places.push((heading_id(library), head.rect));
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
            places.extend(ui.data(|d| d.get_temp::<Vec<(Id, egui::Rect)>>(Id::new("tail stops"))).unwrap_or_default());
        });
        // The fields of the row, gathered after they drew: the arrows walk
        // them like every other place in the window.
        for field in ["tile field", "gap field", "offset field"] {
            places.extend(field_stop(ui.ctx(), Id::new((field, library))));
        }
        ui.data_mut(|d| d.insert_temp(Id::new(("header stops", library)), places));
        (new_grid, clicked)
    }

    /// The right end of a header: the buttons that open the side panels,
    /// then the name of the sheet and the cell under the pointer. The two
    /// texts are truncated: a long name can never push the fields off screen.
    /// Returns whether one of the three buttons was clicked.
    fn header_tail(ui: &mut egui::Ui, sheet: Option<&mut Sheet>, ai: Option<&mut bool>, eye: Option<&mut bool>, name: String, cell: Option<String>) -> bool {
        let mut clicked = false;
        let mut here: Vec<(Id, egui::Rect)> = Vec::new();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The buttons of the sheet wait, greyed out, for a sheet.
            let open = sheet.is_some();
            if let Some(ai) = ai {
                let r = ui.add_enabled(open, egui::Button::new("✨").small().selected(*ai)).on_hover_text("AI assist panel (I)");
                here.push((r.id, r.rect));
                if r.clicked() {
                    *ai = !*ai;
                    clicked = true;
                }
            }
            let anim = sheet.as_ref().is_some_and(|s| s.anim_panel);
            let r = ui.add_enabled(open, egui::Button::new("🎬").small().selected(anim)).on_hover_text("animation panel (A)");
            here.push((r.id, r.rect));
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
            if let Some(eye) = eye {
                let r = ui.add_enabled(open, egui::Button::new("👁").small().selected(*eye)).on_hover_text("view information about the sheet, no editing (E)");
                here.push((r.id, r.rect));
                if r.clicked() {
                    *eye = !*eye;
                    clicked = true;
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(name).truncate());
                if let Some(cell) = cell {
                    ui.add(egui::Label::new(egui::RichText::new(cell).weak()).truncate());
                }
            });
        });
        ui.data_mut(|d| d.insert_temp(Id::new("tail stops"), here));
        clicked
    }

    /// The AI assist panel of the library. The research features land here;
    /// for now it says which models are set up.
    fn assist_panel(ui: &mut egui::Ui, ai: &ai::Ai, keys: &ai::Keys) {
        ui.set_min_width(ui.available_width());
        ui.strong("AI assist");
        ui.weak("Not here yet.");
        ui.add_space(4.0);
        for mode in [ai::Mode::Instant, ai::Mode::Batch] {
            let line = match ai.chosen(mode) {
                Some((p, m)) => {
                    let key = match p.key_source(keys) {
                        ai::KeySource::Typed => "typed key".to_string(),
                        ai::KeySource::Env(name) => format!("key from ${name}"),
                        ai::KeySource::None => "no key".to_string(),
                    };
                    format!("{}: {} {} ({key})", mode.label(), p.name, m.id)
                }
                None => format!("{}: no model chosen", mode.label()),
            };
            ui.weak(line);
        }
        ui.weak("The settings, behind the gear at the right end of the status line, change these.");
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

    /// The pane that holds the keys, read from the keyboard focus. A widget
    /// of a pane counts as that pane, a header field as much as the grid, so
    /// the title stays lit while you work along the header. A focus on
    /// nothing of ours leaves the answer where it was.
    fn spot(&self, ctx: &egui::Context) -> (Panel, Spot) {
        let Some(id) = ctx.memory(|m| m.focused()) else {
            return self.pane;
        };
        self.spot_of(ctx, id).unwrap_or(self.pane)
    }

    /// The pane a widget belongs to. A title counts as its pane, and so does
    /// any other widget that lies inside one, a header field as much as the
    /// grid, so the title stays lit while you work along the header.
    fn spot_of(&self, ctx: &egui::Context, id: Id) -> Option<(Panel, Spot)> {
        for (panel, library) in [(Panel::Library, true), (Panel::Project, false)] {
            if id == if library { library_tree_id() } else { project_tree_id() } || id == tree_heading_id(library) {
                return Some((panel, Spot::Tree));
            }
            if id == if library { library_id() } else { project_id() } || id == heading_id(library) {
                return Some((panel, Spot::Sheet));
            }
            if Some(id) == side_id(ctx, library) || id == anim_heading_id(library) {
                return Some((panel, Spot::Side));
            }
        }
        // A registered place says which pane it belongs to, whether or not
        // it lies inside that pane's own rectangle: the search field and the
        // name field sit above their trees, not in them.
        if let Some(&(at, _, _)) = self.stops.iter().find(|(_, sid, _)| *sid == id) {
            return Some(at);
        }
        // Anything else belongs to the pane it lies in. The rectangles are
        // last frame's, which is where the widget drew.
        let area = |r: &egui::Rect| r.width() * r.height();
        let rect = ctx.read_response(id).map(|r| r.rect)?;
        let hit = self.pane_rects.iter().filter(|(_, r)| r.contains(rect.center())).min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)));
        hit.map(|&(at, _)| at)
    }

    /// Gives the keys to a widget, and says so: the pane it belongs to is
    /// the pane in use from now on. Every key that moves the focus goes
    /// through here, or `self.active` drifts away from where the keys are
    /// and the panels start answering for one another.
    fn go(&mut self, ctx: &egui::Context, id: Id) {
        if std::env::var_os("TILEPICKY_KEYS").is_some() {
            eprintln!("[keys]   -> {}", place_name(self, ctx, id));
        }
        ctx.memory_mut(|m| m.request_focus(id));
        if let Some(at) = self.spot_of(ctx, id) {
            self.pane = at;
            // The status bar belongs to neither half, so it leaves the panel
            // in use as it was: `A`, `M` and copying still mean that panel.
            if at.1 != Spot::Status {
                self.active = at.0;
            }
        }
        // A tree with no row under the keys, or a grid with nothing
        // selected, shows nothing at all. Arriving puts that right at once.
        self.enter_tree(ctx, id, (0, 1));
        if let Some(panel) = [(library_id(), Panel::Library), (project_id(), Panel::Project)].iter().find(|(k, _)| *k == id).map(|(_, p)| *p)
            && let Some(sheet) = self.sheet_mut(panel)
        {
            sheet.start();
        }
    }

    /// Puts the keys on a row when they arrive in a tree with none. Coming
    /// up from below takes the last row, going down takes the first.
    fn enter_tree(&mut self, ctx: &egui::Context, id: Id, d: (i32, i32)) {
        let library = if id == library_tree_id() {
            true
        } else if id == project_tree_id() {
            false
        } else {
            return;
        };
        let panel = if library { Panel::Library } else { Panel::Project };
        let rows = if library { &self.library_rows } else { &self.project_rows };
        let here = if library { &self.library_at } else { &self.project_at };
        // A cursor that is still on a row of this tree stays where it is.
        if here.as_ref().is_some_and(|r| rows.contains(r)) {
            return;
        }
        // The file on show is the friendliest place to start.
        let open = if library { self.library_sel } else { self.project_sel };
        let row = open
            .map(tree::Row::File)
            .filter(|r| rows.contains(r))
            .or_else(|| if d.1 < 0 { rows.last().cloned() } else { rows.first().cloned() });
        if let Some(row) = row {
            self.stand_on(ctx, panel, row);
        }
    }

    /// Whether a pane is there to walk to. An empty panel has no sheet, and
    /// the animation panel needs cells to work on.
    fn has_spot(&self, panel: Panel, spot: Spot) -> bool {
        let sheet = match panel {
            Panel::Library => self.library_sheet.as_ref(),
            Panel::Project => self.project_sheet.as_ref(),
        };
        match spot {
            Spot::Tree => true,
            Spot::Sheet => sheet.is_some(),
            Spot::Side => sheet.is_some_and(|s| s.anim_panel && !s.sel.is_empty()),
            // One status bar, and it rides with the project half.
            Spot::Status => panel == Panel::Project,
            // Tab never walks into a dialog: it opens with a button, and it
            // holds the keys by itself until it closes.
            Spot::Dialog => false,
        }
    }

    /// The panes that are there, left to right and then down: the two file
    /// trees, the two sheets, and the two animation panels when they are
    /// open. Tab walks this list, and a file tree is always in it.
    fn stations(&self) -> Vec<(Panel, Spot)> {
        let mut all = Vec::with_capacity(6);
        for panel in [Panel::Library, Panel::Project] {
            for spot in [Spot::Tree, Spot::Sheet, Spot::Side, Spot::Status] {
                if self.has_spot(panel, spot) {
                    all.push((panel, spot));
                }
            }
        }
        all
    }

    /// Where the work happens in a pane: the rows of a tree, the grid of a
    /// sheet, the fields of an animation panel.
    fn spot_id(&self, ctx: &egui::Context, at: (Panel, Spot)) -> Option<Id> {
        let library = at.0 == Panel::Library;
        Some(match at.1 {
            Spot::Tree if library => library_tree_id(),
            Spot::Tree => project_tree_id(),
            Spot::Sheet if library => library_id(),
            Spot::Sheet => project_id(),
            // The frame field is where the work is; the title is always
            // there, even on the frame the panel opens.
            Spot::Side => side_id(ctx, library).unwrap_or_else(|| anim_heading_id(library)),
            Spot::Status => gear_id(ctx)?,
            Spot::Dialog => return None,
        })
    }

    /// The title of a pane: where Tab leaves you the first time you visit,
    /// because a title says where you are and takes nothing back.
    fn spot_head(&self, at: (Panel, Spot)) -> Option<Id> {
        let library = at.0 == Panel::Library;
        Some(match at.1 {
            Spot::Tree => tree_heading_id(library),
            Spot::Sheet => heading_id(library),
            Spot::Side => anim_heading_id(library),
            // Neither the status bar nor a dialog has a title.
            Spot::Status | Spot::Dialog => return None,
        })
    }

    /// The place the keys last stood in a pane, if it is still on screen.
    fn spot_last(&self, ctx: &egui::Context, at: (Panel, Spot)) -> Option<Id> {
        let id = self.inner.iter().find(|(p, _)| *p == at).map(|(_, id)| *id)?;
        ctx.read_response(id).map(|_| id)
    }

    /// The place nearest to `from` in the direction `d`, of all the places
    /// the arrow keys can stand. A place that shares the other axis with
    /// `from`, and so lies straight that way on screen, comes before one
    /// that sits off to a side.
    fn step_stop(&self, pane: (Panel, Spot), from: egui::Rect, d: (i32, i32)) -> Option<Id> {
        // Distance goes between the edges that face each other, not between
        // the middles. A file tree is tall and its title is a thin strip, and
        // by middles the title loses to whatever lies further up.
        let gap = |a: (f32, f32), b: (f32, f32)| (b.0 - a.1).max(a.0 - b.1).max(0.0);
        let mut best: Option<(f32, Id)> = None;
        for &(_, id, r) in self.stops.iter().filter(|(at, _, _)| *at == pane) {
            let (ahead, aside, straight) = if d.1 != 0 {
                let ahead = if d.1 > 0 { r.min.y - from.max.y } else { from.min.y - r.max.y };
                (ahead, gap((from.min.x, from.max.x), (r.min.x, r.max.x)), r.min.x < from.max.x && from.min.x < r.max.x)
            } else {
                let ahead = if d.0 > 0 { r.min.x - from.max.x } else { from.min.x - r.max.x };
                (ahead, gap((from.min.y, from.max.y), (r.min.y, r.max.y)), r.min.y < from.max.y && from.min.y < r.max.y)
            };
            // A place must lie beyond the edge, not across it: a rectangle
            // that holds `from`, or that `from` holds, is no step at all.
            if ahead < -1.0 {
                continue;
            }
            // A place off to the side loses to any place straight ahead.
            let score = ahead.max(0.0) + aside + if straight { 0.0 } else { 100_000.0 };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Moves the keys one place in a direction, inside the pane they are in.
    /// Nothing happens when that pane has no place that way.
    fn step_from(&mut self, ctx: &egui::Context, id: Id, d: (i32, i32)) {
        let Some(rect) = ctx.read_response(id).map(|r| r.rect) else { return };
        let Some(pane) = self.spot_of(ctx, id) else { return };
        if let Some(to) = self.step_stop(pane, rect, d) {
            self.go(ctx, to);
            self.enter_tree(ctx, to, d);
        }
    }

    /// A click anywhere inside a pane gives it the keys. A click that lands
    /// on a widget which wants the keyboard itself, a text field for one,
    /// keeps them, because that widget has just taken the focus.
    fn take_pane(&mut self, ctx: &egui::Context, panes: &[((Panel, Spot), egui::Rect)]) {
        if self.dialog_open(ctx) || !ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
            return;
        }
        if !ctx.memory(|m| m.focused()).is_none_or(|id| self.is_pane(ctx, id)) {
            return;
        }
        let Some(p) = ctx.input(|i| i.pointer.interact_pos()) else { return };
        let area = |r: &egui::Rect| r.width() * r.height();
        let hit = panes.iter().filter(|(_, r)| r.contains(p)).min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)));
        if let Some(&(at, _)) = hit {
            self.focus_pane(ctx, at, false);
        }
    }

    /// Whether the keyboard focus sits inside one of the panes.
    fn is_pane(&self, ctx: &egui::Context, id: Id) -> bool {
        self.spot_of(ctx, id).is_some()
    }

    /// Gives the keys to a pane. A pane that is not open passes them to the
    /// sheet of its half, and then to the file tree, which is always there.
    ///
    /// `back` means Tab, which returns you where you were in that pane, and
    /// leaves you on its title the first time you ever go there. Without it
    /// the keys land where the work is: the grid of a sheet, the rows of a
    /// tree.
    fn focus_pane(&mut self, ctx: &egui::Context, at: (Panel, Spot), back: bool) {
        let spot = [at.1, Spot::Sheet, Spot::Tree].into_iter().find(|&s| self.has_spot(at.0, s));
        let at = (at.0, spot.unwrap_or(Spot::Tree));
        let id = if back {
            // Back to where you were, else the title, else the work: the
            // status bar has no title, only a gear.
            self.spot_last(ctx, at).or_else(|| self.spot_head(at)).or_else(|| self.spot_id(ctx, at))
        } else {
            self.spot_id(ctx, at)
        };
        let Some(id) = id else { return };
        self.go(ctx, id);
        self.pane = at;
        self.active = at.0;
        if let Some(s) = self.sheet_mut(at.0) {
            s.end_run();
        }
    }

    /// Tab walks the panes to the right and down, and wraps; Shift+Tab walks
    /// them the other way. Ctrl+Tab swaps the halves of the window and keeps
    /// the pane, so a tree meets a tree and a sheet meets a sheet.
    fn press_tab(&mut self, ctx: &egui::Context, step: i32) {
        let now = self.spot(ctx);
        let all = self.stations();
        let at = all.iter().position(|&x| x == now).unwrap_or(0);
        let to = if step == 0 {
            // The other half, on the same pane, and only that one: a sheet
            // meets a sheet or nothing. Landing somewhere else would answer
            // a question nobody asked.
            let other = if now.0 == Panel::Library { Panel::Project } else { Panel::Library };
            if !self.has_spot(other, now.1) {
                return;
            }
            (other, now.1)
        } else {
            all[(at as i32 + step).rem_euclid(all.len() as i32) as usize]
        };
        // Ctrl+Tab crosses to work; Tab walks the panes, and remembers.
        self.focus_pane(ctx, to, step != 0);
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
        let head = ui.label(title_text(ui, anim_heading_id(library), "Animation", keys));
        ui.interact(head.rect, anim_heading_id(library), egui::Sense::click());
        let mut places: Vec<(Id, egui::Rect)> = vec![(anim_heading_id(library), head.rect)];
        let stash = |ui: &egui::Ui, places: Vec<(Id, egui::Rect)>| {
            ui.data_mut(|d| d.insert_temp(Id::new(("side stops", library)), places));
        };
        let Some(b) = sheet.sel.bounds() else {
            ui.weak("Select tiles to play them.");
            stash(ui, places);
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
        for field in ["cell field", "ms field"] {
            places.extend(field_stop(ui.ctx(), Id::new((field, library))));
        }
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
        // Tab reaches this panel here, so the field holds the keys as a pane
        // does; without that egui would walk the focus out of it.
        if let Some(id) = side_id(ui.ctx(), library) {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, sheet::pane_focus()));
        }
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
            places.push((r.id, r.rect));
            if !can_store {
                r.on_disabled_hover_text("an animation needs more than one frame");
            } else if r.clicked() {
                result = sheet.toggle_animation().map(|()| true);
            }
        });
        stash(ui, places);
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
        // The panes own the keyboard. egui walks the focus with Tab and with
        // the bare arrows unless the widget that holds it claims them, and a
        // pane can claim them only while it holds the focus. So the keys go
        // back to the pane they were in as soon as nothing else holds them.
        if !self.dialog_open(ctx) && ctx.memory(|m| m.focused()).is_none() {
            let back = self.pane;
            self.focus_pane(ctx, back, true);
        }
        self.handle_keys(ctx);
        // The preview sets this flag while drawing; clear it first, so that
        // a closed preview does not keep it.
        for s in [&mut self.library_sheet, &mut self.project_sheet].into_iter().flatten() {
            s.preview_hovered = false;
        }

        // Which pane holds the keys. Read once, after the keys of this frame
        // moved it, so that every title agrees.
        let keys = self.spot(ctx);
        self.pane = keys;
        // A tree draws its cursor only while the keys are on its rows. The
        // title of the tree and the fields above it belong to the same pane,
        // but standing on one of those is not standing in the tree.
        let on_rows = (ctx.memory(|m| m.has_focus(library_tree_id())), ctx.memory(|m| m.has_focus(project_tree_id())));
        if let Some(id) = ctx.memory(|m| m.focused()) {
            match self.inner.iter_mut().find(|(p, _)| *p == keys) {
                Some(seat) => seat.1 = id,
                None => self.inner.push((keys, id)),
            }
        }
        // Where each pane lies, filled in as they draw. A click inside one
        // gives it the keys; the smallest pane under the pointer wins, so
        // an animation panel beats the half it sits in.
        let mut panes: Vec<((Panel, Spot), egui::Rect)> = Vec::new();
        let mut library_action = None;
        let mut project_action = None;
        // A click in an empty pane asks for that side's folder.
        let (library_set, project_set) = (self.is_set(Panel::Library), self.is_set(Panel::Project));
        let mut ask: Option<Panel> = None;
        let mut hover_dir: Option<String> = None;
        let mut to_tree = false;
        let mut to_heading = false;
        let mut leave_search = false;
        let mut to_search_in = false;
        let mut shut_search_in = false;
        let mut to_new_button = false;
        let mut tab_out: Option<i32> = None;
        let mut new_button: Option<Id> = None;
        let mut search_in: Option<Id> = None;
        // Every place the arrow keys can stand, and where it lies. The
        // arrows walk these by direction, so the keyboard moves the way the
        // eye does. Inside the panes nothing of egui's own walking runs; the
        // titles, the header fields and the buttons are all places here.
        let mut stops: Vec<((Panel, Spot), Id, egui::Rect)> = Vec::new();
        let mut library_rows: Vec<tree::Row> = Vec::new();
        let mut project_rows: Vec<tree::Row> = Vec::new();
        let mut delete_in_mine = false;
        let mut create = false;
        if self.settings.hide_legend {
            self.status_bar(ctx, ui, &mut stops);
        }
        egui::Panel::left("left").resizable(true).default_size(340.0).size_range(240.0..=800.0).show(ui, |ui| {
            ui.add_space(4.0);
            // One row: the filter button at the left, the box in the rest.
            ui.horizontal(|ui| {
                {
                    // What the search matches on, in a popup under the button.
                    let r = ui.small_button("☰").on_hover_text("search in… (Left from the search field)");
                    if shut_search_in {
                        // The popup of a toggle button is named after it.
                        egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&r));
                        r.request_focus();
                    }
                    // The button has no name of its own; keep the one egui
                    // gave it, so the keys can come back to it.
                    search_in = Some(r.id);
                    stops.push(((Panel::Library, Spot::Tree), r.id, r.rect));
                    egui::Popup::from_toggle_button_response(&r).show(|ui| {
                        ui.set_min_width(220.0);
                        ui.strong("Search in");
                        let first = ui.checkbox(&mut self.settings.search.folders, "folder names");
                        let second = ui.checkbox(&mut self.settings.search.files, "file names");
                        let mut changed = first.changed();
                        changed |= second.changed();
                        if dialog_stops(ui, &r, &[&first, &second]) {
                            shut_search_in = true;
                        }
                        if changed {
                            self.refresh_query();
                            self.settings.save();
                        }
                        if AI_VISIBLE {
                            ui.weak("The captions and tags the AI writes join this list later.");
                        }
                    });
                    // Down leaves the search field for the files below. It
                    // is taken before the field draws, because a text field
                    // eats an arrow first.
                    if ui.memory(|m| m.has_focus(search_id())) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                        leave_search = true;
                    }
                    if ui.memory(|m| m.has_focus(search_id())) {
                        tab_out = tab_out.or(taken_tab(ui));
                    }
                    let search = egui::TextEdit::singleline(&mut self.query)
                        .id(search_id())
                        .hint_text("search: rock wall")
                        .desired_width(ui.available_width());
                    let out = search.show(ui);
                    let r = out.response;
                    // Left with the writing cursor at the start of the text
                    // leaves the field for the button beside it.
                    let at_start = out.cursor_range.is_none_or(|c| c.is_empty() && c.primary.index.0 == 0);
                    if r.has_focus() && at_start && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowLeft)) {
                        to_search_in = true;
                    }
                    // The field keeps the up and down arrows, or egui walks
                    // the focus off with them on its own.
                    ui.memory_mut(|m| {
                        m.set_focus_lock_filter(search_id(), egui::EventFilter { tab: true, escape: true, horizontal_arrows: true, vertical_arrows: true })
                    });
                    stops.push(((Panel::Library, Spot::Tree), search_id(), r.rect));
                    if r.changed() {
                        self.refresh_query();
                    }
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
                         tab: next panel | ctrl+tab: library vs project | arrows: move the selection, shift: extend | {ai_key}ctrl+f: search | ctrl+wheel: zoom | ctrl+z, ctrl+y: undo, redo | ctrl+s: save"
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
                    ui.horizontal(|ui| {
                        if AI_VISIBLE {
                            // The batch job over the library, in a popup under
                            // the button. Not built yet: it says what it will
                            // do, and with which model.
                            let r = ui.add(egui::Button::new("✨").small()).on_hover_text("analyze the library: a batch job");
                            egui::Popup::from_toggle_button_response(&r).show(|ui| {
                                ui.set_max_width(360.0);
                                ui.strong("Analyze the library");
                                ui.label("A batch job over every sheet in the library: it finds the islands of each sheet and asks the batch model to label them, and the sheet as a whole.");
                                let line = match self.settings.ai.chosen(ai::Mode::Batch) {
                                    Some((p, m)) => format!("batch model: {} {}", p.name, m.id),
                                    None => "no batch model chosen; see the settings".to_string(),
                                };
                                ui.weak(line);
                                ui.add_enabled(false, egui::Button::new("Start")).on_disabled_hover_text("not built yet");
                            });
                        }
                        let head = ui.label(title_text(ui, tree_heading_id(true), "LIBRARY", keys == (Panel::Library, Spot::Tree)));
                        ui.interact(head.rect, tree_heading_id(true), egui::Sense::click());
                        stops.push(((Panel::Library, Spot::Tree), tree_heading_id(true), head.rect));
                    });
                    egui::ScrollArea::vertical().id_salt("library scroll").auto_shrink([false, false]).show(ui, |ui| {
                        // The whole visible area answers, before the tree
                        // draws: the files and folders lie on top of it, so
                        // every place that is not one of them is free space.
                        let bg = ui.interact(ui.clip_rect(), library_tree_id(), egui::Sense::click());
                        panes.push(((Panel::Library, Spot::Tree), ui.clip_rect()));
                        stops.push(((Panel::Library, Spot::Tree), library_tree_id(), ui.clip_rect()));
                        ui.memory_mut(|m| m.set_focus_lock_filter(library_tree_id(), sheet::pane_focus()));
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
                            scroll_to: self.library_scroll.as_ref(),
                            cursor: on_rows.0.then(|| self.library_at.as_ref()).flatten(),
                            open_dir: self.library_open_dir.as_ref().map(|(d, o)| (d.as_str(), *o)),
                            sweeping: false,
                            lifting: false,
                        };
                        library_action = self.library_tree.show(ui, &view, "", &mut Vec::new(), &mut library_rows, &mut None);
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
                let title = title_text(ui, tree_heading_id(false), "PROJECT", keys == (Panel::Project, Spot::Tree));
                let heading = ui.add(egui::Label::new(title).sense(egui::Sense::click()));
                ui.interact(heading.rect, tree_heading_id(false), egui::Sense::click());
                stops.push(((Panel::Project, Spot::Tree), tree_heading_id(false), heading.rect));
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
                    stops.push(((Panel::Project, Spot::Tree), new.id, new.rect));
                    if new.clicked() {
                        create = true;
                    }
                    new_button = Some(new.id);
                    // Down goes back to the tree. It is taken before the
                    // field draws, because the field would eat it first.
                    if ui.memory(|m| m.has_focus(new_name_id())) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                        to_tree = true;
                    }
                    if ui.memory(|m| m.has_focus(new_name_id())) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                        to_heading = true;
                    }
                    if ui.memory(|m| m.has_focus(new_name_id())) {
                        tab_out = tab_out.or(taken_tab(ui));
                    }
                    let field = egui::TextEdit::singleline(&mut self.new_name)
                        .id(new_name_id())
                        .hint_text("new tilesheet name")
                        .desired_width(ui.available_width());
                    let out = field.show(ui);
                    let r = out.response;
                    stops.push(((Panel::Project, Spot::Tree), new_name_id(), r.rect));
                    // Right with the writing cursor at the end of the text
                    // leaves the field for the New button beside it.
                    let at_end = out.cursor_range.is_none_or(|c| c.is_empty() && c.primary.index.0 >= self.new_name.chars().count());
                    if r.has_focus() && at_end && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowRight)) {
                        to_new_button = true;
                    }
                    // The field keeps the up and down arrows, so that egui
                    // does not walk the focus away with them.
                    ui.memory_mut(|m| m.set_focus_lock_filter(new_name_id(), egui::EventFilter { tab: true, escape: true, horizontal_arrows: true, vertical_arrows: true }));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        create = true;
                    }
                });
                }
                egui::ScrollArea::vertical().id_salt("project scroll").auto_shrink([false, false]).show(ui, |ui| {
                    // The free space around the tree offers the folder menu.
                    let bg = ui.interact(ui.clip_rect(), project_tree_id(), egui::Sense::click());
                    panes.push(((Panel::Project, Spot::Tree), ui.clip_rect()));
                    stops.push(((Panel::Project, Spot::Tree), project_tree_id(), ui.clip_rect()));
                    ui.memory_mut(|m| m.set_focus_lock_filter(project_tree_id(), sheet::pane_focus()));
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
                        scroll_to: self.project_scroll.as_ref(),
                        cursor: on_rows.1.then(|| self.project_at.as_ref()).flatten(),
                        open_dir: self.project_open_dir.as_ref().map(|(d, o)| (d.as_str(), *o)),
                        sweeping: self.sweep.is_some(),
                        lifting: self.file_drag.is_some(),
                    };
                    // The tree area itself is the root folder; the tree names
                    // a folder inside it when the pointer is over one.
                    if self.file_drag.is_some() && bg.contains_pointer() {
                        hover_dir = Some(String::new());
                    }
                    project_action = self.project_tree.show(ui, &view, "", &mut Vec::new(), &mut project_rows, &mut hover_dir);
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
        if !self.settings.hide_legend {
            self.status_bar(ctx, ui, &mut stops);
        }
        self.open_trees = false;
        self.library_scroll = None;
        self.project_scroll = None;
        self.library_open_dir = None;
        self.project_open_dir = None;
        // The files of the PROJECT tree, in the order it shows them: what a
        // marked group runs over.
        let project_order: Vec<usize> = project_rows.iter().filter_map(|r| if let tree::Row::File(i) = r { Some(*i) } else { None }).collect();
        self.tree_keys(ctx, &library_rows, &project_rows, &project_order);
        if leave_search {
            self.step_from(ctx, search_id(), (0, 1));
        }
        if let (true, Some(id)) = (to_search_in, search_in) {
            self.go(ctx, id);
        }
        if let (true, Some(id)) = (to_new_button, new_button) {
            self.go(ctx, id);
        }
        if let Some(step) = tab_out {
            self.press_tab(ctx, step);
        }
        if to_heading {
            self.go(ctx, tree_heading_id(false));
        }
        // Down from the name field goes to the first row of the PROJECT tree.
        if to_tree {
            self.focus_pane(ctx, (Panel::Project, Spot::Tree), false);
            if let Some(row) = project_rows.first() {
                self.stand_on(ctx, Panel::Project, row.clone());
            }
        }
        match library_action {
            Some(TreeAction::Open(i)) => self.open_library(ctx, i),
            Some(TreeAction::Refresh) => self.rescan_library(),
            Some(TreeAction::Reveal(i)) => reveal(&file_path(&self.library.root, &self.library.entries[i].rel)),
            Some(TreeAction::RevealDir(dir)) => reveal(&file_path(&self.library.root, &dir)),
            Some(TreeAction::CopyPath(i, whole)) => {
                let rel = self.library.entries[i].rel.clone();
                self.copy_path(ctx, &self.library.root.clone(), &rel, whole);
            }
            Some(TreeAction::CopyDirPath(dir, whole)) => self.copy_path(ctx, &self.library.root.clone(), &dir, whole),
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
            Some(TreeAction::RevealDir(dir)) => reveal(&file_path(&self.project.root, &dir)),
            Some(TreeAction::CopyPath(i, whole)) => {
                let rel = self.project.entries[i].rel.clone();
                self.copy_path(ctx, &self.project.root.clone(), &rel, whole);
            }
            Some(TreeAction::CopyDirPath(dir, whole)) => self.copy_path(ctx, &self.project.root.clone(), &dir, whole),
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
        self.legend_dialog(ctx);

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
            panes.push(((Panel::Library, Spot::Sheet), ui.max_rect()));
            let live = keys == (Panel::Library, Spot::Sheet);
            let ai = AI_VISIBLE.then_some(&mut self.ai_panel);
            let eye = LIBRARY_EYE_VISIBLE.then_some(&mut self.library_eye);
            let clicked;
            (library_tile, clicked) = Self::sheet_header(ui, "Source", live, true, self.library_sheet.as_mut(), ai, eye);
            if clicked {
                self.active = Panel::Library;
            }
            let eye = self.library_eye;
            if self.ai_panel {
                egui::Panel::right("library assist").resizable(true).default_size(260.0).show(ui, |ui| Self::assist_panel(ui, &self.settings.ai, &self.keys));
            }
            if let Some(s) = &mut self.library_sheet {
                if s.anim_panel {
                    let side = egui::Panel::right("library animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        library_anim = Self::animation_panel(ui, s, true, keys == (Panel::Library, Spot::Side));
                    });
                    panes.push(((Panel::Library, Spot::Side), side.response.rect));
                }
                let out = egui::CentralPanel::default().show(ui, |ui| s.view(ui, library_id(), dragging, false, eye, keys == (Panel::Library, Spot::Sheet)));
                stops.push(((Panel::Library, Spot::Sheet), library_id(), out.response.rect));
                let ev = out.inner;
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
        if let Some(state) = egui::PanelState::load(ctx, panel_id) {
            if total > 0.0 {
                self.split = (state.outer_rect.height() / total).clamp(0.1, 0.9);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.project_rect = ui.max_rect();
            panes.push(((Panel::Project, Spot::Sheet), ui.max_rect()));
            let live = keys == (Panel::Project, Spot::Sheet);
            let clicked;
            (project_tile, clicked) = Self::sheet_header(ui, "Canvas", live, false, self.project_sheet.as_mut(), None, Some(&mut self.project_eye));
            if clicked {
                self.active = Panel::Project;
            }
            let eye = self.project_eye;
            if let Some(s) = &mut self.project_sheet {
                if s.anim_panel {
                    let side = egui::Panel::right("my animation").resizable(true).default_size(220.0).show(ui, |ui| {
                        anim_changed = Self::animation_panel(ui, s, false, keys == (Panel::Project, Spot::Side));
                    });
                    panes.push(((Panel::Project, Spot::Side), side.response.rect));
                }
                let out = egui::CentralPanel::default().show(ui, |ui| s.view(ui, project_id(), dragging, true, eye, keys == (Panel::Project, Spot::Sheet)));
                stops.push(((Panel::Project, Spot::Sheet), project_id(), out.response.rect));
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
                    s.clear_selection(ctx);
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
            if let Some(s) = &self.project_sheet {
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
        // Where everything ended up this frame: the panes, the places the
        // arrow keys can stand, and the header rows. The keys of the next
        // frame read these, so they must wait until the last pane has drawn.
        self.pane_rects = panes.clone();
        // A dialog that drew this frame owns its places, the button it hangs
        // from included. They go in front, so that button answers as part of
        // the dialog while it is open and as part of its pane when it is not.
        let dialog = ctx.data_mut(|d| d.remove_temp::<Vec<(Id, egui::Rect)>>(Id::new("dialog stops"))).unwrap_or_default();
        stops.splice(0..0, dialog.into_iter().map(|(id, r)| ((Panel::Library, Spot::Dialog), id, r)));
        // The places of the header rows and the animation panels, gathered
        // by the code that draws them.
        for library in [true, false] {
            let panel = if library { Panel::Library } else { Panel::Project };
            for (key, spot) in [("header stops", Spot::Sheet), ("side stops", Spot::Side)] {
                let here = ctx.data(|d| d.get_temp::<Vec<(Id, egui::Rect)>>(Id::new((key, library)))).unwrap_or_default();
                stops.extend(here.into_iter().map(|(id, r)| ((panel, spot), id, r)));
            }
        }
        self.stops = stops;
        self.library_rows = library_rows;
        self.project_rows = project_rows;
        self.take_pane(ctx, &panes);
        // Whatever holds the keys inside a pane claims them from egui, which
        // would otherwise walk the focus with the arrows and with Tab behind
        // our back, and land on things like a panel's resize bar. A header
        // row keeps its sideways walk, because egui does that one well: it
        // steps between the fields by where they lie.
        if let Some(id) = ctx.memory(|m| m.focused())
            && !ctx.text_edit_focused()
            && self.spot_of(ctx, id).is_some()
        {
            ctx.memory_mut(|m| m.set_focus_lock_filter(id, sheet::pane_focus()));
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
            // focus of a widget that has gone, and the panes hand the keys
            // straight back to a button that is not there: a loop with
            // nowhere to stand.
            let mut box_ = egui::TextEdit::singleline(buf).desired_width(56.0);
            if let Some((id, _)) = ui.data(|d| d.get_temp::<(egui::Id, egui::Rect)>(self.id.with("widget"))) {
                box_ = box_.id(id);
            }
            let r = ui.add(box_);
            if edit.focus {
                r.request_focus();
                edit.focus = false;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                edit.text = None;
                shut_text_box(ui, r.id);
                return None;
            }
            if r.lost_focus() {
                let v = (self.parse)(buf);
                edit.text = None;
                shut_text_box(ui, r.id);
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
        // The arrows walk from place to place by where each one lies, and
        // only the widget itself knows that.
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
