//! A folder tree built from relative paths, filtered by a visibility mask.

use eframe::egui::{self, Ui};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Node {
    pub dirs: BTreeMap<String, Node>,
    /// File name and the index of the entry it stands for.
    pub files: Vec<(String, usize)>,
}

impl Node {
    pub fn build(rels: &[String]) -> Self {
        let mut root = Node::default();
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
    /// `open_all` expands every folder this frame; after that the user folds them as they like.
    pub fn show(&self, ui: &mut Ui, visible: Option<&[bool]>, selected: Option<usize>, open_all: bool, salt: &str) -> Option<usize> {
        let mut clicked = None;
        for (name, dir) in &self.dirs {
            if !dir.any_visible(visible) {
                continue;
            }
            let id = ui.make_persistent_id((salt, name));
            egui::CollapsingHeader::new(name)
                .id_salt(id)
                .open(open_all.then_some(true))
                .show(ui, |ui| {
                    let salt = format!("{salt}/{name}");
                    if let Some(c) = dir.show(ui, visible, selected, open_all, &salt) {
                        clicked = Some(c);
                    }
                });
        }
        for (name, i) in &self.files {
            if visible.is_some_and(|v| !v[*i]) {
                continue;
            }
            if ui.selectable_label(selected == Some(*i), name).clicked() {
                clicked = Some(*i);
            }
        }
        clicked
    }
}
