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

/// Put the primary action at the right edge, with secondary actions to its left.
pub(crate) fn footer(ui: &mut egui::Ui, buttons: impl FnOnce(&mut egui::Ui)) {
    ui.separator();
    ui.horizontal(|ui| { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), buttons); });
}

pub(crate) fn footer_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y + 3.0 * ui.spacing().item_spacing.y + 1.0
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
        self.label_options_dialog(ctx);
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
            footer(ui, |ui| {
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
            footer(ui, |ui| {
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
            footer(ui, |ui| {
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
            footer(ui, |ui| {
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
            footer(ui, |ui| {
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
            || self.clear_labels.is_some() || self.label_options.is_some() || self.label_view || self.pending.is_some()
            || self.legend_prompt || !self.damaged.is_empty() || self.settings_open
            || egui::Popup::is_any_open(ctx)
    }

    /// The dialog keeps its target when the selected sheet changes.
    fn label_dialog(&mut self, ctx: &egui::Context) {
        if !self.label_view { return; }
        let Some(target) = self.label_target.clone() else { return };
        let path = target.path();
        let chosen = self.settings.ai.chosen(ai::Mode::Instant);
        let ready = chosen.is_some_and(|(p, _)| p.key_source(&self.keys) != ai::KeySource::None);
        let next_model = chosen.map(|(p, m)| format!("{} / {}", p.name, m.id));
        let current_run = self.label_run.as_ref().filter(|run| run.path == path);
        let running = current_run.is_some();
        let progress = current_run.map(|r| format!("{} / {}. Elapsed: {} s", r.provider, r.model, r.started.elapsed().as_secs()));
        let outcome = self.label_outcome.as_ref().filter(|out| out.path == path).cloned();
        let busy = self.label_run.is_some() || !self.library_batch.allows_single();
        let has_log = self.target_log(&path).is_some_and(crate::ai_log::Log::available);
        let mut action = None; let mut close = false; let mut copy = false;
        let mut options = false; let mut open_job = false; let mut settings = false;
        egui::Modal::new(Id::new("AI label")).show(ctx, |ui| {
            ui.set_width(500.0_f32.min(ctx.content_rect().width() - 48.0));
            ui.set_max_height((ctx.content_rect().height() - 96.0).max(160.0));
            ui.heading("Sheet AI label");
            ui.add(egui::Label::new(&target.rel).truncate()).on_hover_text(&target.rel);
            ui.small(format!("Library: {}", target.dir.file_name().unwrap_or_default().to_string_lossy()))
                .on_hover_text(target.dir.display().to_string());
            if let Some(progress) = &progress {
                ui.horizontal_wrapped(|ui| { ui.spinner(); ui.label(progress); });
                ui.label("Closing this dialog keeps the request running.");
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            } else if let Some(outcome) = &outcome {
                if outcome.failed { ui.colored_label(ui.visuals().error_fg_color, labels::summary(&outcome.message)); }
                else { ui.label(&outcome.message); }
            }
            if !target.error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, labels::summary(&target.error)); }
            if busy && !running {
                ui.label(self.library_batch.single_block_reason().unwrap_or("Another sheet is being labeled."));
                open_job = ui.button("Open current job").clicked();
            }
            if !ready {
                ui.label(if chosen.is_none() { "Choose a single-sheet model in Settings." } else { "Add this provider's API key in Settings." });
                settings = ui.button("Configure AI...").clicked();
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if running {
                    if ui.button("Cancel request").on_hover_text("Stops local waiting. The provider may still bill the request.").clicked() {
                        action = Some(labels::Action::Cancel);
                    }
                } else {
                    let text = if outcome.as_ref().is_some_and(|out| out.retry) { "Retry" }
                        else if target.label.is_some() { "Label again" } else { "Label this sheet" };
                    if ui.add_enabled(ready && !busy && target.error.is_empty(), egui::Button::new(text).selected(true)).clicked() {
                        action = Some(labels::Action::Label);
                    }
                }
                copy = ui.add_enabled(has_log, egui::Button::new("Copy log"))
                    .on_hover_text("This sheet's latest request. A new job replaces the log; exit removes it after completion.").clicked();
                if self.label_copied.is_some_and(|at| at.elapsed().as_secs() < 3) {
                    ui.label("Copied"); ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
            });
            if !self.label_copy_error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, labels::summary(&self.label_copy_error)); }
            ui.separator();
            let body_height = (ctx.content_rect().height() - ui.min_rect().height() - 96.0 - footer_height(ui)).max(80.0);
            egui::ScrollArea::vertical().max_height(body_height).show(ui, |ui| {
                if let Some(outcome) = &outcome && outcome.message.chars().count() > 180 {
                    egui::CollapsingHeader::new("Error details").show(ui, |ui| { ui.label(&outcome.message); });
                }
                if !target.error.is_empty() { ui.label(&target.error); }
                if let Some(label) = &target.label {
                    ui.strong("Saved label"); label.show(ui);
                    ui.small(format!("Created with {} / {}", label.provider, label.model));
                    if ui.add_enabled(!busy, egui::Button::new("Remove label...")).clicked() { action = Some(labels::Action::Remove); }
                } else { ui.label("No saved label."); }
                ui.separator();
                if let Some(model) = &next_model { ui.label(format!("Next request: {model}")); }
                ui.label(format!("Tags to look for: {}", target.tags.join(", ")));
                ui.horizontal_wrapped(|ui| {
                    options = ui.button("Edit library options...").clicked();
                    if ui.button("Current prompt...").clicked() {
                        self.prompt_view = Some(("Next request for this sheet".into(), labels::sheet_prompt(labels::prompt(&target.tags), &target.rel)));
                    }
                });
            });
            footer(ui, |ui| { close = ui.button("Close").clicked(); });
            if self.prompt_view.is_none() && self.label_options.is_none() && self.remove_label.is_none()
                && ui.input(|i| i.key_pressed(Key::Escape)) { close = true; }
        });
        if copy { self.label_copy_error = self.copy_single_log(ctx, &path).err().unwrap_or_default(); }
        if close { self.close_label_view(ctx); }
        if options { self.open_label_options(target.dir); }
        if open_job { self.open_active_label_job(ctx); }
        if settings { self.close_label_view(ctx); self.settings_request = true; }
        if let Some(action) = action { self.label_target_action(ctx, action); }
    }

    fn label_options_dialog(&mut self, ctx: &egui::Context) {
        let Some(options) = &mut self.label_options else { return };
        let mut save = false; let mut close = false;
        egui::Modal::new(Id::new("AI library options")).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ctx.content_rect().width() - 48.0));
            ui.heading("Options for new labels");
            ui.label(options.root.display().to_string());
            ui.label("These tags apply to new single-sheet and library requests. Existing jobs keep their original tags.");
            ui.strong("Tags to look for");
            ui.label("Separate tags with commas. The model checks each tag and can add its own.");
            ui.add(egui::TextEdit::multiline(&mut options.text).desired_width(f32::INFINITY).desired_rows(4));
            if !options.error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, &options.error); }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Reset tags").clicked() { options.text = sidecar::TAG_LIST.join(", "); }
                if ui.button("Prompt template...").clicked() {
                    self.prompt_view = Some(("New requests add each sheet's filename and relative folder".into(),
                        labels::prompt(&labels::parse_list(&options.text))));
                }
            });
            footer(ui, |ui| {
                save = ui.button("Save options").clicked();
                close = ui.button("Cancel").clicked() || (self.prompt_view.is_none() && ui.input(|i| i.key_pressed(Key::Escape)));
            });
        });
        if save {
            let tags = labels::parse_list(&options.text);
            match sidecar::store_tag_list(&options.root, &tags) {
                Ok(()) => {
                    if self.library.index.root == options.root { self.library.index.tag_list = tags.clone(); }
                    if let Some(target) = &mut self.label_target && target.dir == options.root { target.tags = tags; }
                    close = true;
                }
                Err(error) => options.error = error,
            }
        }
        if close { self.label_options = None; }
    }

    /// Shows a prompt as it goes to the model. The text can be selected and copied.
    fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some((title, texts)) = &self.prompt_view else { return };
        let mut close = false;
        egui::Modal::new(Id::new("labeling prompt")).show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ctx.content_rect().width() - 48.0));
            ui.set_max_height((ctx.content_rect().height() - 96.0).max(160.0));
            ui.heading(title.as_str());
            let height = (ctx.content_rect().height() - ui.min_rect().height() - 96.0 - footer_height(ui)).max(80.0);
            egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                for (name, text) in ["System", "User, with the image"].iter().zip(texts) {
                    ui.add_space(4.0);
                    ui.strong(*name);
                    ui.add(egui::TextEdit::multiline(&mut text.as_str()).desired_width(f32::INFINITY));
                }
            });
            footer(ui, |ui| { close = ui.button("Close").clicked() || ui.input(|i| i.key_pressed(Key::Escape)); });
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
            footer(ui, |ui| {
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
            footer(ui, |ui| {
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
