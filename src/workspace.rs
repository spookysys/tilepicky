// SPDX-License-Identifier: GPL-3.0-only
//! Task navigation, the visual library, and scoped search tools.

use crate::{App, Panel, Spot, Index, labels, settings, egui, Id};
use crate::tiled_inspect;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Workspace { Library, Build }

/// What a click in the library inspector asks the app to do.
enum Ask {
    /// Show the image face of the card.
    Sheet,
    /// Show the Tiled face of the card.
    Tiled,
    /// Return to the entry that the last cross-link came from.
    Back,
    /// Open another entry, by its path.
    Jump(String),
    /// Drop the person's grid, so that the sheet follows the tileset.
    UseGrid,
    /// Turn on the images that the tilesets name.
    ShowImages,
}

impl App {
    pub(super) fn workspace_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("workspace navigation").show(ui, |ui| {
            crate::set_pane(ui, (Panel::Project, Spot::Status));
            ui.horizontal_wrapped(|ui| {
                ui.strong("Tilepicky");
                let before = self.workspace;
                crate::stop(&ui.selectable_value(&mut self.workspace, Workspace::Library, "Library"));
                crate::stop(&ui.selectable_value(&mut self.workspace, Workspace::Build, "Build"));
                if before != self.workspace {
                    self.drag = None;
                    self.library_drag = None;
                    self.active = Panel::Library;
                    self.pane = (Panel::Library, Spot::Tree);
                    ui.ctx().memory_mut(|m| m.request_focus(crate::library_tree_id()));
                }
                ui.separator();
                if ui.add_enabled(self.library.is_set(), egui::Button::new("Improve search...")).clicked() {
                    self.open_improve();
                }
                let running = self.label_run.is_some() || self.embed_run.is_some() || self.library_batch.processing();
                if ui.selectable_label(self.ai_panel, if running { "Activity (running)" } else { "Activity" }).clicked() {
                    self.ai_panel = !self.ai_panel;
                }
                if crate::stopped(ui.button("AI setup...")).clicked() { self.settings_request = true; }
                ui.menu_button("Folders", |ui| {
                    if ui.button("Open library...").clicked() { self.ask_folder(Panel::Library); ui.close(); }
                    if ui.button("Open project...").clicked() { self.ask_folder(Panel::Project); ui.close(); }
                    if ui.button("Refresh library").clicked() { self.rescan_library(); ui.close(); }
                });
                if self.workspace == Workspace::Build {
                    let mut hints = !self.settings.hide_legend;
                    let response = ui.checkbox(&mut hints, "Keyboard hints");
                    ui.data_mut(|d| d.insert_temp(Id::new("shortcut setting response"), response.id));
                    if response.changed() {
                        self.settings.hide_legend = !hints;
                        if let Err(error) = self.settings.save() { self.status = error; }
                    }
                    if ui.add_enabled(self.project.sheet.is_some(), egui::Button::new("Save")).clicked() { self.save(); }
                    if ui.add_enabled(self.project.sheet.is_some(), egui::Button::new("Colors and export...")).clicked() {
                        self.quant_panel = !self.quant_panel;
                    }
                }
            });
        });
    }

    pub(super) fn open_improve(&mut self) {
        self.improve_rels = self.marked_library_rels().into_iter().collect();
        if self.library_marked.is_empty() && self.library_marked_dirs.is_empty() {
            self.improve_rels = self.library.index.entries.iter().filter(|e| e.sheet_rel().is_some()).map(|e| e.rel.clone()).collect();
        }
        self.improve_open = true;
    }

    pub(super) fn library_workspace(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.library.sel.is_some() {
            let ask = egui::Panel::right("sheet inspector").default_size(250.0).size_range(180.0..=400.0).resizable(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.library_inspector(ui)).inner
                })
                .inner;
            if let Some(ask) = ask {
                self.library_ask(&ctx, ask);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Library");
                let (tilesets, in_tilesets, loose) = self.library.index.view_counts();
                // A pack without Tiled files looks as it did before this
                // feature, so the boxes appear only when one is there.
                if tilesets > 0 {
                    let mut view = self.settings.view;
                    ui.checkbox(&mut view.tilesets, format!("Tilesets ({tilesets})"));
                    ui.checkbox(&mut view.in_tilesets, format!("Images in tilesets ({in_tilesets})"));
                    ui.checkbox(&mut view.loose, format!("Images not in tilesets ({loose})"));
                    if view != self.settings.view {
                        self.settings.view = view;
                        if let Err(error) = self.settings.save() { self.status = error; }
                        self.refresh_visible();
                    }
                }
                ui.checkbox(&mut self.library_without_tags, "Without tags");
                ui.checkbox(&mut self.compare_search, "Compare search");
                if !self.library_marked.is_empty() || !self.library_marked_dirs.is_empty() {
                    ui.label(format!("{} selected", self.marked_library_rels().len()));
                    if ui.button("Clear selection").clicked() {
                        self.library_marked.clear(); self.library_marked_dirs.clear(); self.library_batch.choice = None;
                    }
                }
            });
            if !self.library.is_set() {
                ui.label("Open a folder of sprite sheets to start. Your source images stay unchanged.");
                if ui.button("Open library...").clicked() { self.ask_folder(Panel::Library); }
                return;
            }
            if self.compare_search { self.search_comparison(ui); return; }
            let view = self.library.index.effective_view(self.settings.view);
            let mut order = self.library.index.ranked(&self.qwords, self.settings.search,
                self.library.embed_query.as_deref(), self.library.text_embed_query.as_deref());
            let matched = order.len();
            order.retain(|i| self.library.index.entries[*i].shows(view));
            let shown = order.len();
            if self.library_without_tags { order.retain(|i| self.library.index.entries[*i].side.label.is_none()); }
            // A library that the view does not trim keeps the sentence it had
            // before the tilesets came, so nothing reads differently for a
            // person who does not use Tiled.
            if shown == matched {
                ui.weak(format!("{} sheets. Click to inspect; use checkboxes to select a job scope.", order.len()));
            } else {
                let noun = if self.qwords.is_empty() { "entries" } else { "matches" };
                ui.weak(format!("{} shown of {} {noun}. Click to inspect; use checkboxes to select a job scope.",
                    order.len(), matched));
            }
            if !self.qwords.is_empty() && (self.query_embed.is_some() || self.text_query.pending.is_some()) {
                ui.weak("Meaning search is running. Word matches are available.");
            }
            if let Some((_, error)) = self.query_error.as_ref().or(self.text_query.error.as_ref()) {
                ui.colored_label(ui.visuals().error_fg_color, labels::summary(error));
                if ui.button("Retry meaning search").clicked() { self.query_error = None; self.text_query.error = None; }
            }
            if let Some(i) = self.gallery.show_ordered(ui, &self.library.index, &order, self.library.sel, &mut self.library_marked,
                view) {
                self.open_library(&ctx, i);
            }
            if let Some(i) = self.gallery.take_drag() { self.start_library_drag(i); }
        });
    }

    /// The selected card: the sheet, its Tiled tab, or the tileset of a card
    /// that has no pixels. Returns what a click asks for; the caller owns the
    /// state and the files.
    fn library_inspector(&mut self, ui: &mut egui::Ui) -> Option<Ask> {
        let i = self.library.sel.filter(|i| *i < self.library.index.entries.len())?;
        let mut ask = None;
        // A cross-link may open an entry that the filters hide. The panel
        // says where the person came from, and the filters stay as they are.
        if let Some((from, _)) = &self.library_from {
            ui.horizontal_wrapped(|ui| {
                ui.small(format!("From {from}"));
                if crate::stopped(ui.button("Back")).clicked() { ask = Some(Ask::Back); }
            });
        }
        let entry = &self.library.index.entries[i];
        if entry.tiled.is_some() && !entry.tileset_only {
            ui.horizontal(|ui| {
                if crate::stopped(ui.selectable_label(!self.library_tiled_tab, "Sheet")).clicked() { ask = Some(Ask::Sheet); }
                if crate::stopped(ui.selectable_label(self.library_tiled_tab, "Tiled")).clicked() { ask = Some(Ask::Tiled); }
            });
            ui.separator();
        }
        if entry.tiled.is_some() && (self.library_tiled_tab || entry.tileset_only) {
            let sheet = self.library.sheet.as_ref().filter(|sheet| sheet.rel == entry.rel);
            if let Some(action) = tiled_inspect::show(ui, &self.library.index, i, sheet) {
                ask = Some(match action {
                    tiled_inspect::Action::Jump(to) => Ask::Jump(to),
                    tiled_inspect::Action::UseGrid => Ask::UseGrid,
                    tiled_inspect::Action::ShowImages => Ask::ShowImages,
                });
            }
            return ask;
        }
        ui.strong(&entry.rel);
        ui.add_space(8.0);
        self.gallery.preview(ui, &self.library.index, i, egui::vec2(ui.available_width(), 210.0));
        if let Some(file) = entry.tileset_file() {
            if entry.tiled.is_some() {
                ui.small(format!("Tiled: {file}"));
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.small(format!("Tiled: {file}"));
                    if crate::stopped(ui.link("Open")).clicked() {
                        ask = Some(Ask::Jump(file.to_string()));
                    }
                });
                ui.small("This image is one tile of that collection.");
            }
        }
        if let Some(note) = &entry.tiled_note { ui.small(note); }
        if let Some(error) = &entry.tiled_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(label) = &entry.side.label {
            ui.label(&label.caption);
            ui.horizontal_wrapped(|ui| { for tag in &label.tags { ui.label(egui::RichText::new(tag).strong()); } });
        } else { ui.weak("No captions or tags yet."); }
        let sources = Index::match_sources(entry, &self.qwords, self.settings.search,
            self.library.embed_query.as_deref(), self.library.text_embed_query.as_deref());
        if !self.qwords.is_empty() && !sources.is_empty() { ui.small(format!("Matches: {}", sources.join(", "))); }
        ui.separator();
        let pixels = entry.sheet_rel().is_some();
        ui.add_enabled_ui(pixels, |ui| {
            if ui.button("Use in tilesheet").clicked() {
                self.workspace = Workspace::Build;
                self.active = Panel::Library;
                self.pane = (Panel::Library, Spot::Sheet);
                ui.ctx().memory_mut(|m| m.request_focus(crate::library_id()));
            }
            if ui.button("Copy sheet to project").clicked() { self.copy_library_sheet(ui.ctx(), i); }
            if ui.button("Review / generate tags...").clicked() { self.label_action(ui.ctx(), labels::Action::Show); }
            if ui.button("Improve this sheet...").clicked() {
                self.improve_rels = vec![self.library.index.entries[i].rel.clone()];
                self.improve_open = true;
            }
        });
        ask
    }

    /// Does what the inspector asked. Only this side changes files; the panel
    /// itself is read-only.
    fn library_ask(&mut self, ctx: &egui::Context, ask: Ask) {
        match ask {
            Ask::Sheet => self.library_tiled_tab = false,
            Ask::Tiled => self.library_tiled_tab = true,
            Ask::Back => self.back_library(ctx),
            Ask::Jump(to) => self.jump_library(ctx, &to),
            Ask::UseGrid => self.use_tileset_grid(ctx),
            Ask::ShowImages => self.show_tileset_images(),
        }
    }

    fn search_comparison(&mut self, ui: &mut egui::Ui) {
        if self.qwords.is_empty() { ui.label("Enter a query to compare words, image meaning, and label meaning."); return; }
        ui.weak("Each column ranks its own source. Similarity scores from different models are not compared.");
        let base = settings::SearchIn { folders: false, files: false, captions: false, tags: false,
            embeddings: false, text_embeddings: false, similarity: self.settings.search.similarity };
        let sources = [
            ("Words", settings::SearchIn { folders: true, files: true, captions: true, tags: true, ..base }),
            ("Image meaning", settings::SearchIn { embeddings: true, ..base }),
            ("Label meaning", settings::SearchIn { text_embeddings: true, ..base }),
        ];
        let mut open = None;
        let mut rebuild = None;
        ui.columns(3, |columns| {
            for (column, (title, search)) in columns.iter_mut().zip(sources) {
                column.strong(title);
                if search.embeddings || search.text_embeddings {
                    let (enabled, vector, count) = if search.embeddings {
                        (self.settings.search.embeddings, self.library.embed_query.as_ref(),
                            self.library.index.entries.iter().filter(|e| e.embed.is_some()).count())
                    } else {
                        (self.settings.search.text_embeddings, self.library.text_embed_query.as_ref(),
                            self.library.index.entries.iter().filter(|e| e.text_embed.is_some()).count())
                    };
                    column.small(format!("{count} / {} sheets indexed", self.library.index.entries.len()));
                    if !enabled {
                        if column.button("Enable this source").clicked() {
                            if search.embeddings { self.settings.search.embeddings = true; }
                            else { self.settings.search.text_embeddings = true; }
                            self.refresh_query();
                            if let Err(e) = self.settings.save() { self.status = e; }
                        }
                        continue;
                    }
                    let error = if search.embeddings { self.query_error.as_ref() } else { self.text_query.error.as_ref() };
                    if let Some((_, message)) = error {
                        column.colored_label(column.visuals().error_fg_color, labels::summary(message));
                        if column.button("Retry this source").clicked() {
                            if search.embeddings { self.query_error = None; } else { self.text_query.error = None; }
                        }
                        continue;
                    }
                    let chosen = if search.embeddings { self.settings.ai.chosen_embed() } else { self.settings.ai.chosen_text_embed() };
                    let (saved_model, saved_provider) = if search.embeddings {
                        (&self.library.index.embed_model, &self.library.index.embed_provider)
                    } else { (&self.library.index.text_embed_model, &self.library.index.text_embed_provider) };
                    let Some((provider, model)) = chosen else {
                        column.label("Choose an embedding model in AI setup.");
                        if column.button("AI setup...").clicked() { self.settings_request = true; }
                        continue;
                    };
                    if count == 0 || saved_model != &model.id || saved_provider != &crate::embed::identity(provider) {
                        if count == 0 { column.label("No current index is available for this source."); }
                        else {
                            column.label("The saved index does not match the selected model or provider.");
                            column.small(format!("Saved model: {saved_model}. Selected model: {}.", model.id));
                        }
                        if column.button("Review index generation...").clicked() { rebuild = Some(search.embeddings); }
                        continue;
                    }
                    if vector.is_none() { column.weak("Preparing the search query..."); continue; }
                }
                let mut results = self.library.index.ranked(&self.qwords, search,
                    self.library.embed_query.as_deref(), self.library.text_embed_query.as_deref());
                results.retain(|i| self.library.index.entries[*i].shows(self.library.index.effective_view(self.settings.view)));
                column.small(format!("{} matches", results.len()));
                egui::ScrollArea::vertical().id_salt(title).show(column, |ui| {
                    for i in results {
                        if ui.selectable_label(self.library.sel == Some(i), &self.library.index.entries[i].rel).clicked() { open = Some(i); }
                    }
                });
            }
        });
        if let Some(i) = open { self.open_library(ui.ctx(), i); }
        if let Some(images) = rebuild {
            self.open_improve();
            self.embed_images = images;
            self.embed_text = !images;
        }
    }

    pub(super) fn workflow_windows(&mut self, ctx: &egui::Context) {
        if self.ai_panel {
            let mut open = true;
            egui::Window::new("Activity").id(Id::new("activity window")).open(&mut open)
                .default_width(540.0).default_height(500.0).vscroll(true).show(ctx, |ui| { self.assist_panel(ui); });
            self.ai_panel &= open;
        }
        if !self.improve_open { return; }
        let mut open = true;
        let mut action = None;
        egui::Window::new("Improve search").open(&mut open).default_width(480.0).vscroll(true).show(ctx, |ui| {
            let entire = self.improve_rels.len() == self.library.index.entries.len();
            ui.heading(if entire { format!("Entire library: {} sheets", self.improve_rels.len()) }
                else { format!("Selected sheets: {}", self.improve_rels.len()) });
            if entire { ui.small("Search filters do not limit this scope. Select checkboxes to choose specific sheets."); }
            egui::CollapsingHeader::new("Review scope").show(ui, |ui| {
                for rel in &self.improve_rels { ui.label(rel); }
            });
            ui.label("Choose a task. Review the model, inputs, and replacement options before generation starts.");
            ui.group(|ui| {
                ui.strong("Captions and tags");
                ui.label("Describe visible content and make its words searchable.");
                if ui.button("Review tag generation...").clicked() { action = Some(0); }
                if ui.add_enabled(!self.improve_rels.is_empty(), egui::Button::new("Try one sheet...")).clicked() { action = Some(3); }
            });
            ui.group(|ui| {
                ui.strong("Meaning search");
                ui.checkbox(&mut self.embed_images, "Images");
                ui.checkbox(&mut self.embed_text, "Captions and tags");
                ui.label("Generate independent indexes with separate requests. Sheets without labels skip the text index.");
                if ui.add_enabled(self.embed_images || self.embed_text, egui::Button::new("Review search indexes...")).clicked() {
                    action = Some(1);
                }
            });
            if crate::stopped(ui.button("AI setup...")).clicked() { self.settings_request = true; }
            ui.small("Your selection stays here while you configure AI. No generation starts from setup.");
        });
        self.improve_open = open;
        if let Some(action) = action {
            if self.improve_rels.is_empty() { self.status = "Select at least one sheet.".into(); return; }
            match action {
                0 if self.improve_rels.len() > 1 => {
                    self.library_batch.choose(self.improve_rels.iter().cloned().collect(), format!("Selected sheets: {}", self.improve_rels.len()));
                    self.ai_panel = true;
                }
                0 | 3 => {
                    self.open_label_target(ctx, self.library.index.root.clone(), self.improve_rels[0].clone());
                }
                _ => {
                    self.embed_rels = Some(self.improve_rels.clone());
                    if (self.embed_images && self.settings.ai.chosen_embed().is_none())
                        || (self.embed_text && self.settings.ai.chosen_text_embed().is_none()) {
                        self.settings_request = true; return;
                    }
                    if self.embed_run.is_some() || self.embed_preparing.is_some() {
                        self.status = "Wait for the current search index task or cancel it in Activity.".into();
                        return;
                    }
                    self.prepare_embeddings(ctx);
                    if self.embed_preparing.is_none() { return; }
                }
            }
            self.improve_open = false;
        }
    }
}
