//! A folder tree built from relative paths, filtered by a visibility mask.

use eframe::egui::{self, Ui};
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

    /// Draws the tree. Returns the entry the user clicked, if any.
    ///
    /// On the frame the query changed (`apply_query`), the folders are set
    /// once: a folder opens when it holds matches below but its own path
    /// does not yet satisfy the query; a folder whose path satisfies it is
    /// shown closed, since everything inside matches anyway. After that
    /// frame the user folds and unfolds freely.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &self,
        ui: &mut Ui,
        visible: Option<&[bool]>,
        selected: Option<usize>,
        marked: Option<&HashSet<usize>>,
        query: &[String],
        apply_query: bool,
        menus: bool,
        dir_rel: &str,
        path_words: &mut Vec<String>,
        order: &mut Vec<usize>,
    ) -> Option<TreeAction> {
        let mut action = None;
        for (name, dir) in &self.dirs {
            if !dir.any_visible(visible) {
                continue;
            }
            let before = path_words.len();
            for w in crate::index::words(name) {
                if !path_words.contains(&w) {
                    path_words.push(w);
                }
            }
            let open = if apply_query {
                let satisfied = crate::index::matches(query, |q| path_words.iter().any(|w| w.starts_with(q)));
                Some(!satisfied)
            } else {
                None
            };
            let rel = if dir_rel.is_empty() { name.clone() } else { format!("{dir_rel}/{name}") };
            let id = ui.make_persistent_id(("tree", &rel));
            let header = egui::CollapsingHeader::new(name).id_salt(id).open(open).show(ui, |ui| {
                if let Some(a) = dir.show(ui, visible, selected, marked, query, apply_query, menus, &rel, path_words, order) {
                    action = Some(a);
                }
            });
            if menus {
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
            if visible.is_some_and(|v| !v[*i]) {
                continue;
            }
            order.push(*i);
            let is_marked = marked.is_some_and(|m| m.contains(i));
            let r = ui.selectable_label(selected == Some(*i) || is_marked, name);
            if r.clicked() {
                let mods = ui.input(|inp| inp.modifiers);
                action = Some(match (mods.shift, mods.command) {
                    (true, additive) => TreeAction::Range(*i, additive),
                    (false, true) => TreeAction::Toggle(*i),
                    (false, false) => TreeAction::Open(*i),
                });
            }
            if menus {
                let group = marked.is_some_and(|m| m.len() > 1 && m.contains(i));
                let count = marked.map_or(0, HashSet::len);
                r.context_menu(|ui| {
                    if group {
                        if ui.button(format!("Delete {count} files…")).clicked() {
                            action = Some(TreeAction::DeleteMarked);
                            ui.close();
                        }
                        return;
                    }
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
                });
            }
        }
        action
    }
}
