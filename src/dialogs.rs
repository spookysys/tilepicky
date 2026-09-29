// SPDX-License-Identifier: GPL-3.0-only
//! The dialogs that ask before something happens: names, deletions, unsaved
//! changes, the removal of a label, the legend, and a damaged settings file. Each waits in a field of
//! the app until the user answers.

use crate::{App, ai, labels, sidecar};
use eframe::egui::{self, Id, Key};

/// What to do once the user has decided about unsaved changes. A file is
/// named by its path, since a save can add a file and so move the others.
#[derive(Clone, PartialEq)]
pub enum Pending {
    Open(String),
    Create,
    Close,
}

/// A configuration file that could not be read, and why.
pub enum Damaged {
    Settings(String),
    Keys(String),
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
        self.label_dialog(ctx);
        self.remove_label_dialog(ctx);
        self.clear_labels_dialog(ctx);
        self.prompt_dialog(ctx);
        self.library_batch.confirmation(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.has_unsaved() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        self.save_dialog(ctx);
        self.legend_dialog(ctx);
        self.damaged_dialog(ctx);
    }

    /// A settings file that could not be read is written over with the
    /// defaults, once the user agrees. Quit leaves it as it is, to mend by
    /// hand.
    fn damaged_dialog(&mut self, ctx: &egui::Context) {
        if self.damaged.is_empty() {
            return;
        }
        let mut choice = None;
        egui::Modal::new(Id::new("damaged dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("A settings file is damaged");
            for d in &self.damaged {
                let (Damaged::Settings(e) | Damaged::Keys(e)) = d;
                ui.label(e);
            }
            ui.add_space(4.0);
            ui.label("Tilepicky starts with the defaults and will write them over the damaged file. To mend the file by hand instead, quit now.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Continue").clicked() {
                    choice = Some(true);
                }
                if ui.button("Quit").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                for d in std::mem::take(&mut self.damaged) {
                    let written = match d {
                        Damaged::Settings(_) => self.settings.rewrite(),
                        Damaged::Keys(_) => self.keys.rewrite(),
                    };
                    if let Err(e) = written {
                        self.status = e;
                    }
                }
            }
            Some(false) => {
                self.damaged.clear();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
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
            self.cancel_name();
        } else if apply {
            let p = self.prompt.take().unwrap();
            if let Err(e) = self.apply_name(ctx, &p.what, &p.value) {
                self.status = e;
            }
            // A Save As that succeeded ran the action that waited for it;
            // one that failed drops it.
            self.after_save = None;
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
        let Some(action) = self.pending.clone() else { return };
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
            self.answer_save(ctx, action, save);
        }
    }

    /// Save or Discard in the unsaved changes dialog. A sheet with no name,
    /// or one that is no PNG, asks for a name first, and the action waits
    /// for that save. A save that fails keeps the changes, and the action
    /// is dropped.
    pub fn answer_save(&mut self, ctx: &egui::Context, action: Pending, save: bool) {
        self.pending = None;
        if save {
            self.save();
            if self.prompt.is_some() {
                self.after_save = Some(action);
                return;
            }
            if self.has_unsaved() {
                return;
            }
        }
        if let Some(sheet) = &mut self.project.sheet {
            sheet.dirty = false;
        }
        self.run(ctx, action);
    }

    /// Closes the name prompt, and drops an action that waited for it.
    pub fn cancel_name(&mut self) {
        self.prompt = None;
        self.after_save = None;
    }

    /// A dialog or a popup is up: the keys belong to it, Escape first of all.
    pub fn dialog_open(&self, ctx: &egui::Context) -> bool {
        self.prompt.is_some() || self.confirm.is_some() || self.remove_label.is_some() || self.prompt_view.is_some() || self.library_batch.open()
            || self.clear_labels.is_some() || self.label_view || self.pending.is_some() || self.legend_prompt || !self.damaged.is_empty() || self.settings_open
            || egui::Popup::is_any_open(ctx)
    }

    /// Reads the current sheet again each frame, so a completed request updates the popup.
    fn label_dialog(&mut self, ctx: &egui::Context) {
        if !self.label_view { return; }
        let Some(sheet) = &self.library.sheet else { self.label_view = false; return };
        let ready = self.settings.ai.chosen(ai::Mode::Instant)
            .is_some_and(|(p, _)| p.kind == ai::Kind::OpenAi && p.key_source(&self.keys) != ai::KeySource::None);
        let mut action = None;
        let mut close = false;
        let mut copy_log = false;
        let has_log = self.single_log_available();
        let outcome = self.selected_label_outcome().map(str::to_string);
        egui::Modal::new(Id::new("AI label")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("AI label");
            ui.label(&sheet.rel);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                if let Some(label) = &sheet.side.label {
                    label.show(ui);
                    ui.weak(format!("{} / {}", label.provider, label.model));
                    if ui.button("Current prompt...").clicked() {
                        let title = if label.tag_list.is_some() { "Current prompt with this label's tag list" }
                            else { "Current prompt without a recorded tag list" };
                        self.prompt_view = Some((title.into(),
                            labels::sheet_prompt(labels::prompt(label.tag_list.as_deref().unwrap_or_default()), &sheet.rel)));
                    }
                } else {
                    ui.weak("No label yet.");
                }
            });
            ui.add_space(8.0);
            if let Some(run) = &self.label_run {
                ui.label(format!("{} / {}", run.provider, run.model));
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Labeling {}: {} s", run.rel, run.started.elapsed().as_secs()));
                });
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            } else if let Some(outcome) = &outcome {
                ui.label(outcome);
            }
            if let Some(reason) = self.library_batch.single_block_reason() { ui.label(reason); }
            ui.horizontal(|ui| {
                close = ui.button("Close").clicked();
                copy_log = ui.add_enabled(has_log, egui::Button::new("Copy log"))
                    .on_hover_text("Copy the single-sheet request log for this sheet. Removed on exit when the job has finished.").clicked();
                let busy = self.label_run.is_some() || !self.library_batch.allows_single();
                let text = if sheet.side.label.is_some() { "Label again" } else { "Label with AI" };
                if ui.add_enabled(ready && !busy, egui::Button::new(text)).clicked() { action = Some(labels::Action::Label); }
                if ui.add_enabled(sheet.side.label.is_some() && !busy, egui::Button::new("Remove label...")).clicked() {
                    action = Some(labels::Action::Remove);
                }
                if self.label_run.is_some() && ui.button("Cancel").clicked() { action = Some(labels::Action::Cancel); }
            });
            if self.prompt_view.is_none() && self.remove_label.is_none() && ui.input(|i| i.key_pressed(Key::Escape)) { close = true; }
        });
        if copy_log { self.copy_single_log(ctx); }
        if close { self.label_view = false; }
        if let Some(action) = action { self.label_action(ctx, action); }
    }

    /// Shows a prompt as it goes to the model. The text can be selected and copied.
    fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some((title, texts)) = &self.prompt_view else { return };
        let mut close = false;
        egui::Modal::new(Id::new("labeling prompt")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(title.as_str());
            for (name, text) in ["System", "User, with the image"].iter().zip(texts) {
                ui.add_space(4.0);
                ui.strong(*name);
                ui.add(egui::TextEdit::multiline(&mut text.as_str()).desired_width(f32::INFINITY));
            }
            ui.add_space(4.0);
            close = ui.button("Close").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
        });
        if close { self.prompt_view = None; }
    }

    fn clear_labels_dialog(&mut self, ctx: &egui::Context) {
        let Some(root) = self.clear_labels.clone() else { return };
        let mut choice = None;
        egui::Modal::new(Id::new("clear all AI labels")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading("Clear all AI labels?");
            ui.label(root.display().to_string());
            ui.label("This removes all saved AI captions and tags in this library, including its subfolders.");
            ui.label("Images, grids, animations, and the list of tags to look for stay. This cannot be undone.");
            ui.horizontal(|ui| {
                if ui.button("Clear all").clicked() { choice = Some(true); }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) { choice = Some(false); }
            });
        });
        if let Some(clear) = choice {
            self.clear_labels = None;
            if clear {
                self.status = match self.clear_library_labels(&root) {
                    Ok(n) => format!("Cleared {n} AI labels."),
                    Err(e) => e,
                };
            }
        }
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
