// SPDX-License-Identifier: GPL-3.0-only
//! The dialogs that ask before something happens: names, deletions, unsaved
//! changes, the removal of a label, and the legend. Each waits in a field of
//! the app until the user answers.

use crate::{App, sidecar};
use eframe::egui::{self, Id, Key};

/// What to do once the user has decided about unsaved changes.
#[derive(Clone, Copy, PartialEq)]
pub enum Pending {
    Open(usize),
    Create,
    Close,
}

/// What the name prompt is for.
#[derive(Clone, PartialEq)]
pub enum NameFor {
    SaveAs,
    RenameFile(String),
    DuplicateFile(String),
    NewFolder(String),
    RenameFolder(String),
}

pub struct NamePrompt {
    pub title: String,
    pub value: String,
    pub what: NameFor,
    pub focus: bool,
}

impl App {
    /// Shows the dialog that waits, if any. A close of the window with
    /// unsaved changes waits for the save dialog.
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        self.name_dialog(ctx);
        self.confirm_dialog(ctx);
        self.remove_label_dialog(ctx);
        self.library_batch.confirmation(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.has_unsaved() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        self.save_dialog(ctx);
        self.legend_dialog(ctx);
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
                    if let Err(e) = self.settings.save() { self.status = e; }
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

    /// The dialog for unsaved changes: save, discard, or cancel.
    fn save_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending else { return };
        let name = self.project.sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
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
            if save && self.project.sheet.as_ref().is_some_and(|s| s.rel.is_empty()) {
                // No name yet: ask for one; the interrupted action is dropped.
                self.pending = None;
                self.save();
                return;
            }
            self.pending = None;
            if save {
                self.save();
                // A save that failed keeps the changes, and the action waits.
                if self.has_unsaved() {
                    return;
                }
            }
            if let Some(sheet) = &mut self.project.sheet {
                sheet.dirty = false;
            }
            self.run(ctx, action);
        }
    }

    /// A dialog or a popup is up: the keys belong to it, Escape first of all.
    pub fn dialog_open(&self, ctx: &egui::Context) -> bool {
        self.prompt.is_some() || self.confirm.is_some() || self.remove_label.is_some() || self.library_batch.open()
            || self.pending.is_some() || self.legend_prompt || egui::Popup::is_any_open(ctx)
    }

    fn remove_label_dialog(&mut self, ctx: &egui::Context) {
        let Some((dir, rel)) = self.remove_label.clone() else { return };
        let path = dir.join(&rel);
        let mut choice = None;
        egui::Modal::new(Id::new("remove AI label")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Remove the AI label?");
            ui.label(path.display().to_string());
            ui.label("This removes the caption and the tags. The image and the grid stay.");
            ui.horizontal(|ui| {
                if ui.button("Remove label").clicked() { choice = Some(true); }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) { choice = Some(false); }
            });
        });
        if let Some(remove) = choice {
            self.remove_label = None;
            if remove {
                match sidecar::store_labels(&dir, [(rel.as_str(), None)]) {
                    Ok(()) => {
                        self.apply_labels(&dir, &[(rel, None)]);
                        self.status = "AI label removed.".into();
                    }
                    Err(error) => self.status = error,
                }
            }
        }
    }
}
