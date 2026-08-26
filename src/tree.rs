// SPDX-License-Identifier: GPL-3.0-only
//! A folder tree built from relative paths, filtered by a visibility mask.

use eframe::egui::{self, Color32, Stroke, Ui};
use std::collections::HashSet;
use std::collections::BTreeMap;

/// What the user did in the tree.
pub enum TreeAction {
    Open(usize),
    /// Ctrl+click: add or remove the file from the marked set.
    Toggle(usize),
    /// Shift+click: mark the range from the anchor to this file; with Ctrl
    /// held too, add the range to the marks instead of replacing them.
    Range(usize, bool),
    /// A drag began on this file: it marks every file the pointer crosses.
    SweepStart(usize),
    /// The pointer crossed this file during such a drag.
    Sweep(usize),
    /// The pointer held still on this file: it goes into the air, to be
    /// dropped on a folder.
    LiftFile(usize),
    /// Show the file in the desktop's file manager.
    Reveal(usize),
    /// Put the file's path on the clipboard: relative to the root, or whole.
    CopyPath(usize, bool),
    RenameFile(usize),
    DuplicateFile(usize),
    DeleteFile(usize),
    /// Delete every marked file.
    DeleteMarked,
    /// Scan the directory again; files may have changed behind our back.
    Refresh,
    /// Show the folder in the desktop's file manager.
    RevealDir(String),
    /// Put the folder's path on the clipboard: relative to the root, or whole.
    CopyDirPath(String, bool),
    /// Make a folder inside this directory ("" is the root).
    NewFolder(String),
    RenameFolder(String),
    DeleteFolder(String),
}

/// One row of the tree, in the order the eye reads them. The arrow keys walk
/// these, so a folder is a place to stand as much as a file is.
#[derive(Clone, PartialEq, Debug)]
pub enum Row {
    /// A folder, by its path below the root.
    Dir(String),
    /// A file, by its index in the index.
    File(usize),
}

/// What the tree needs to draw itself, apart from the nodes.
pub struct View<'a> {
    /// Which entries the search leaves visible; `None` shows every one.
    pub visible: Option<&'a [bool]>,
    /// The file that is open in the panel.
    pub selected: Option<usize>,
    /// The files in the marked group.
    pub marked: Option<&'a HashSet<usize>>,
    pub query: &'a [String],
    /// The query changed this frame: set the folders open or closed once.
    pub apply_query: bool,
    /// Offer the file and folder menus.
    pub menus: bool,
    /// Bring this row into view: the keyboard moved to it.
    pub scroll_to: Option<&'a Row>,
    /// The row the arrow keys stand on. A folder shows it with a frame; a
    /// file is already marked as the open one.
    pub cursor: Option<&'a Row>,
    /// Open or close this folder once, because the keyboard said so.
    pub open_dir: Option<(&'a str, bool)>,
    /// A drag over the files is marking a group right now.
    pub sweeping: bool,
    /// Files are in the air, looking for a folder to land in.
    pub lifting: bool,
}

#[derive(Default)]
pub struct Node {
    pub dirs: BTreeMap<String, Node>,
    /// File name and the index of the entry it stands for.
    pub files: Vec<(String, usize)>,
}

impl Node {
    pub fn build(rels: &[String], dirs: &[String]) -> Self {
        let mut root = Node::default();
        for d in dirs {
            let mut node = &mut root;
            for part in d.split('/') {
                node = node.dirs.entry(part.to_string()).or_default();
            }
        }
        for (i, rel) in rels.iter().enumerate() {
            let parts: Vec<&str> = rel.split('/').collect();
            let mut node = &mut root;
            for dir in &parts[..parts.len() - 1] {
                node = node.dirs.entry((*dir).to_string()).or_default();
            }
            node.files.push((parts[parts.len() - 1].to_string(), i));
        }
        root
    }

    fn any_visible(&self, visible: Option<&[bool]>) -> bool {
        let Some(v) = visible else { return true };
        self.files.iter().any(|(_, i)| v[*i]) || self.dirs.values().any(|d| d.any_visible(visible))
    }

    /// Draws the tree. Returns the entry the user clicked, if any. It also
    /// fills `order` with the visible files, top to bottom, which is the
    /// order the arrow keys walk.
    ///
    /// On the frame the query changed (`apply_query`), the folders are set
    /// once: a folder opens when it holds matches below but its own path
    /// does not yet satisfy the query; a folder whose path satisfies it is
    /// shown closed, since everything inside matches anyway. After that
    /// frame the user folds and unfolds freely.
    pub fn show(
        &self,
        ui: &mut Ui,
        v: &View,
        dir_rel: &str,
        path_words: &mut Vec<String>,
        rows: &mut Vec<Row>,
        hover_dir: &mut Option<String>,
    ) -> Option<TreeAction> {
        let mut action = None;
        for (name, dir) in &self.dirs {
            if !dir.any_visible(v.visible) {
                continue;
            }
            let before = path_words.len();
            for w in crate::index::words(name) {
                if !path_words.contains(&w) {
                    path_words.push(w);
                }
            }
            let rel = if dir_rel.is_empty() { name.clone() } else { format!("{dir_rel}/{name}") };
            let open = if v.apply_query {
                let satisfied = crate::index::matches(v.query, |q| path_words.iter().any(|w| w.starts_with(q)));
                Some(!satisfied)
            } else {
                // A folder the keyboard opened or closed. `CollapsingHeader`
                // keeps what it is given, so one frame of this is enough.
                v.open_dir.filter(|(d, _)| *d == rel).map(|(_, o)| o)
            };
            // The folder comes before what it holds, and a closed one draws
            // no body, so the rows read as the eye does.
            rows.push(Row::Dir(rel.clone()));
            let here = Row::Dir(rel.clone());
            // A band behind the row, kept back until the header has drawn
            // and its size is known. It goes in first, so the letters of the
            // folder stay on top of it.
            let band = ui.painter().add(egui::Shape::Noop);
            let id = ui.make_persistent_id(("tree", &rel));
            let header = egui::CollapsingHeader::new(name).id_salt(id).open(open).show(ui, |ui| {
                if let Some(a) = dir.show(ui, v, &rel, path_words, rows, hover_dir) {
                    action = Some(a);
                }
            });
            if v.scroll_to == Some(&here) {
                header.header_response.scroll_to_me(None);
            }
            // The arrow keys stand here. A folder has no chosen look of its
            // own, so it borrows the one a chosen file wears.
            if v.cursor == Some(&here) {
                let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.4);
                ui.painter().set(band, egui::Shape::rect_filled(header.header_response.rect, 2.0, fill));
            }
            // A folder under lifted files is where they land.
            if v.lifting && header.header_response.contains_pointer() {
                *hover_dir = Some(rel.clone());
                let stroke = Stroke::new(2.0, Color32::from_rgb(80, 160, 255));
                ui.painter().rect_stroke(header.header_response.rect, 2.0, stroke, egui::StrokeKind::Inside);
            }
            header.header_response.context_menu(|ui| {
                // Only a tree the user owns offers the items that change folders.
                if v.menus {
                    if ui.button("New folder…").clicked() {
                        action = Some(TreeAction::NewFolder(rel.clone()));
                        ui.close();
                    }
                    if ui.button("Rename…").clicked() {
                        action = Some(TreeAction::RenameFolder(rel.clone()));
                        ui.close();
                    }
                    if ui.button("Delete…").clicked() {
                        action = Some(TreeAction::DeleteFolder(rel.clone()));
                        ui.close();
                    }
                    ui.separator();
                }
                if ui.button("Open location").clicked() {
                    action = Some(TreeAction::RevealDir(rel.clone()));
                    ui.close();
                }
                if ui.button("Copy relative path").clicked() {
                    action = Some(TreeAction::CopyDirPath(rel.clone(), false));
                    ui.close();
                }
                if ui.button("Copy absolute path").clicked() {
                    action = Some(TreeAction::CopyDirPath(rel.clone(), true));
                    ui.close();
                }
            });
            path_words.truncate(before);
        }
        for (name, i) in &self.files {
            if v.visible.is_some_and(|vis| !vis[*i]) {
                continue;
            }
            rows.push(Row::File(*i));
            let is_marked = v.marked.is_some_and(|m| m.contains(i));
            // The cursor goes on behind: the file on show wears the solid
            // colour, and this pale band says only where the keys stand.
            let band = ui.painter().add(egui::Shape::Noop);
            let mut r = ui.selectable_label(v.selected == Some(*i) || is_marked, name);
            if v.cursor == Some(&Row::File(*i)) {
                let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.4);
                ui.painter().set(band, egui::Shape::rect_filled(r.rect, 2.0, fill));
            }
            if v.scroll_to == Some(&Row::File(*i)) {
                r.scroll_to_me(None);
            }
            // Only a tree that marks groups answers a drag across the files.
            if v.marked.is_some() && !v.lifting {
                r = r.interact(egui::Sense::click_and_drag());
                if r.drag_started() {
                    action = Some(TreeAction::SweepStart(*i));
                } else if v.sweeping && r.contains_pointer() {
                    action = Some(TreeAction::Sweep(*i));
                } else if r.is_pointer_button_down_on() && !r.dragged() {
                    // Holding still on a file lifts it, as it does on a tile.
                    const HOLD_S: f64 = 0.25;
                    const STILL_PX: f32 = 4.0;
                    let (t0, from, now, at) = ui.input(|inp| {
                        (inp.pointer.press_start_time(), inp.pointer.press_origin(), inp.time, inp.pointer.latest_pos())
                    });
                    if let (Some(t0), Some(from), Some(at)) = (t0, from, at) {
                        if (at - from).length() <= STILL_PX {
                            if now - t0 >= HOLD_S {
                                action = Some(TreeAction::LiftFile(*i));
                            } else {
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
                            }
                        }
                    }
                }
            }
            // A file under lifted files points at the folder it lies in.
            if v.lifting && r.contains_pointer() {
                *hover_dir = Some(dir_rel.to_string());
            }
            if r.clicked() {
                let mods = ui.input(|inp| inp.modifiers);
                action = Some(match (mods.shift, mods.command) {
                    (true, additive) => TreeAction::Range(*i, additive),
                    (false, true) => TreeAction::Toggle(*i),
                    (false, false) => TreeAction::Open(*i),
                });
            }
            let group = v.marked.is_some_and(|m| m.len() > 1 && m.contains(i));
            let count = v.marked.map_or(0, HashSet::len);
            r.context_menu(|ui| {
                // Only a tree the user owns offers the items that change files.
                if v.menus && group {
                    if ui.button(format!("Delete {count} files…")).clicked() {
                        action = Some(TreeAction::DeleteMarked);
                        ui.close();
                    }
                    ui.separator();
                } else if v.menus {
                    if ui.button("Rename…").clicked() {
                        action = Some(TreeAction::RenameFile(*i));
                        ui.close();
                    }
                    if ui.button("Duplicate…").clicked() {
                        action = Some(TreeAction::DuplicateFile(*i));
                        ui.close();
                    }
                    if ui.button("Delete…").clicked() {
                        action = Some(TreeAction::DeleteFile(*i));
                        ui.close();
                    }
                    ui.separator();
                }
                if ui.button("Open location").clicked() {
                    action = Some(TreeAction::Reveal(*i));
                    ui.close();
                }
                if ui.button("Copy relative path").clicked() {
                    action = Some(TreeAction::CopyPath(*i, false));
                    ui.close();
                }
                if ui.button("Copy absolute path").clicked() {
                    action = Some(TreeAction::CopyPath(*i, true));
                    ui.close();
                }
            });
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One headless pass over a small tree. It gives back the rows the arrow
    /// keys walk.
    fn rows_of(open: Option<(&str, bool)>) -> Vec<Row> {
        let ctx = egui::Context::default();
        let tree = Node::build(&["a/one.png".to_string(), "a/two.png".to_string(), "top.png".to_string()], &[]);
        let mut rows = Vec::new();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            {
                let v = View {
                    visible: None,
                    selected: None,
                    marked: None,
                    query: &[],
                    apply_query: false,
                    menus: false,
                    scroll_to: None,
                    cursor: None,
                    open_dir: open,
                    sweeping: false,
                    lifting: false,
                };
                tree.show(ui, &v, "", &mut Vec::new(), &mut rows, &mut None);
            }
        });
        // The pass made a font texture that no screen will ever take.
        out.textures_delta.clear();
        rows
    }

    /// A folder is a row of its own, before the files it holds, and a closed
    /// one hides them.
    #[test]
    fn the_rows_hold_the_folders() {
        assert_eq!(rows_of(None), vec![Row::Dir("a".into()), Row::File(2)]);
        assert_eq!(rows_of(Some(("a", true))), vec![Row::Dir("a".into()), Row::File(0), Row::File(1), Row::File(2)]);
    }
}
