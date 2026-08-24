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
    /// Make a folder inside this directory ("" is the root).
    NewFolder(String),
    RenameFolder(String),
    DeleteFolder(String),
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
    /// Bring this file into view: the keyboard moved to it.
    pub scroll_to: Option<usize>,
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
        order: &mut Vec<usize>,
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
            let open = if v.apply_query {
                let satisfied = crate::index::matches(v.query, |q| path_words.iter().any(|w| w.starts_with(q)));
                Some(!satisfied)
            } else {
                None
            };
            let rel = if dir_rel.is_empty() { name.clone() } else { format!("{dir_rel}/{name}") };
            let id = ui.make_persistent_id(("tree", &rel));
            let header = egui::CollapsingHeader::new(name).id_salt(id).open(open).show(ui, |ui| {
                if let Some(a) = dir.show(ui, v, &rel, path_words, order, hover_dir) {
                    action = Some(a);
                }
            });
            // A folder under lifted files is where they land.
            if v.lifting && header.header_response.contains_pointer() {
                *hover_dir = Some(rel.clone());
                let stroke = Stroke::new(2.0, Color32::from_rgb(80, 160, 255));
                ui.painter().rect_stroke(header.header_response.rect, 2.0, stroke, egui::StrokeKind::Inside);
            }
            if v.menus {
                header.header_response.context_menu(|ui| {
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
                });
            }
            path_words.truncate(before);
        }
        for (name, i) in &self.files {
            if v.visible.is_some_and(|vis| !vis[*i]) {
                continue;
            }
            order.push(*i);
            let is_marked = v.marked.is_some_and(|m| m.contains(i));
            let mut r = ui.selectable_label(v.selected == Some(*i) || is_marked, name);
            if v.scroll_to == Some(*i) {
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
