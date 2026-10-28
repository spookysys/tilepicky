// SPDX-License-Identifier: GPL-3.0-only
//! The tile-properties panel of the project half: the properties of the
//! selected tiles, edited in place in the Tiled tileset beside the image.
//! The panel touches nothing on disk; `Sheet::save` writes the file.

use crate::sheet::Sheet;
use crate::tileset::{self, Kind, Prop};
use eframe::egui;

/// The property the add row makes next: its name and its kind.
#[derive(Clone, Default)]
struct NewProp {
    name: String,
    kind: Kind,
}

/// Draws the panel for the open sheet. A change marks the sheet unsaved.
pub fn show(ui: &mut egui::Ui, sheet: &mut Sheet, keys: bool) {
    egui::ScrollArea::vertical().id_salt("tile properties").show(ui, |ui| {
        ui.heading(crate::title_text("Tile properties", keys));
        if sheet.rel.is_empty() {
            ui.weak("The tileset goes beside the image when you save it.");
        } else {
            let file = tileset::file_name(&sheet.rel);
            ui.weak(file.display().to_string()).on_hover_text("The Tiled tileset beside the image. Tiled reads it; Tilepicky writes it on save.");
        }
        ui.weak("A turquoise dot marks a tile with properties. Undo restores property edits.");
        ui.separator();
        // A file this tool cannot read, or a grid Tiled cannot state, leaves
        // nothing to edit: a change could not reach the disk.
        if let Some(error) = &sheet.props_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.weak("Saving is refused until the file reads. Fix it outside Tilepicky, then open the sheet again.");
            return;
        }
        if let Err(reason) = tileset::check(sheet.tileset_grid()) {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
            ui.weak("The panel is read-only. Tiled cannot hold this grid, and a save would refuse.");
            return;
        }
        let ids: Vec<u32> = sheet.sel.iter().map(|cell| sheet.tile_id(cell)).collect();
        if ids.is_empty() {
            ui.weak("Select tiles in the sheet to give them properties.");
            return;
        }
        ui.label(match ids.len() {
            1 => "1 tile selected".to_string(),
            n => format!("{n} tiles selected"),
        });
        if ids.len() > 1 {
            ui.weak("An edit applies to every selected tile.");
        }
        ui.add_space(4.0);
        let names: Vec<String> = sheet.props.names(&ids).into_iter().collect();
        let mut removed = None;
        for name in &names {
            let sample = sheet.props.sample(&ids, name);
            let mixed = sheet.props.common(&ids, name).is_none();
            // The row's widgets take their ids from the name, so a field
            // keeps its focus when a row above it goes.
            ui.push_id(name, |ui| {
                // The name and the kind share a line, and the value takes the
                // width below it: a name stays readable in a narrow panel.
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate()).on_hover_text(name);
                    if mixed {
                        ui.weak("mixed").on_hover_text("Not every selected tile holds this value. An edit applies to all of them.");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖").on_hover_text(format!("remove {name} from the selected tiles")).clicked() {
                            removed = Some(name.clone());
                        }
                        if let Some(prop) = &sample {
                            ui.weak(prop.kind().label());
                        }
                    });
                });
                if let Some(prop) = &sample
                    && let Some(value) = value_editor(ui, prop)
                {
                    sheet.set_property(&ids, name, value);
                }
            });
            ui.add_space(6.0);
        }
        if let Some(name) = removed {
            sheet.remove_property(&ids, &name);
        }
        ui.add_space(4.0);
        add_row(ui, sheet, &ids, &names);
        if !ui.ctx().egui_wants_keyboard_input() && !ui.input(|i| i.pointer.any_down()) {
            sheet.end_property_edit();
        }
    });
}

/// One value widget, and the value it changed to.
fn value_editor(ui: &mut egui::Ui, prop: &Prop) -> Option<Prop> {
    match prop {
        Prop::Str(text) => {
            let mut text = text.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
            r.changed().then_some(Prop::Str(text))
        }
        Prop::Int(value) => {
            let mut value = *value;
            let r = ui.add(egui::DragValue::new(&mut value).speed(1.0));
            r.changed().then_some(Prop::Int(value))
        }
        Prop::Float(value) => {
            let mut value = *value;
            let r = ui.add(egui::DragValue::new(&mut value).speed(0.1));
            r.changed().then_some(Prop::Float(value))
        }
        Prop::Bool(value) => {
            let mut value = *value;
            let r = ui.checkbox(&mut value, "");
            r.changed().then_some(Prop::Bool(value))
        }
    }
}

/// The name field, the kind, and the button that give one more property to
/// the selection.
fn add_row(ui: &mut egui::Ui, sheet: &mut Sheet, ids: &[u32], names: &[String]) {
    let state = egui::Id::new("tile property to add");
    let mut new = ui.data(|d| d.get_temp::<NewProp>(state).unwrap_or_default());
    let taken = names.iter().any(|n| n.as_str() == new.name.trim());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut new.name).hint_text("name").desired_width(110.0));
        egui::ComboBox::from_id_salt("tile property kind").selected_text(new.kind.label()).width(80.0).show_ui(ui, |ui| {
            for kind in Kind::ALL {
                ui.selectable_value(&mut new.kind, kind, kind.label());
            }
        });
        let name = new.name.trim().to_string();
        if ui.add_enabled(!name.is_empty() && !taken, egui::Button::new("Add")).on_hover_text("give this property to the selected tiles").clicked() {
            sheet.set_property(ids, &name, new.kind.start());
            new.name.clear();
        }
    });
    if taken && !new.name.trim().is_empty() {
        ui.weak(format!("{} is already on the selection. Edit it above.", new.name.trim()));
    }
    ui.data_mut(|d| d.insert_temp(state, new));
}
