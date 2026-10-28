// SPDX-License-Identifier: GPL-3.0-only
//! The read-only Tiled tab of the library inspector: the file, its grid, the
//! data in its tiles, the health checks, and the links to its images. The
//! panel writes nothing; a click leaves an `Action` for the caller.

use crate::index::{Entry, Index};
use crate::sheet::Sheet;
use crate::tiled::{self, Tiled};
use crate::tileset::Grid;
use eframe::egui;

/// The rows that the tile table draws. ERW holds an atlas with 1,065 tiles
/// that carry collision, so the panel shows a summary and counts the rest.
const ROW_LIMIT: usize = 100;

/// What a click asks the library to do. The caller owns the state.
pub enum Action {
    /// Open another entry, by its path from the library root.
    Jump(String),
    /// Drop the person's grid, so that the sheet follows the tileset.
    UseGrid,
    /// Turn on `Images in tilesets`, so that the hidden images appear.
    ShowImages,
}

/// Draws the tab for entry `i`. `sheet` is the open sheet of that entry, when
/// there is one: a tileset card has no pixels, so it has no sheet.
pub fn show(ui: &mut egui::Ui, index: &Index, i: usize, sheet: Option<&Sheet>) -> Option<Action> {
    let entry = index.entries.get(i)?;
    let Some(tiled) = &entry.tiled else {
        ui.weak("This entry has no Tiled tileset.");
        return None;
    };
    let mut action = None;
    ui.push_id(&entry.rel, |ui| {
        identity(ui, index, entry, tiled, &mut action);
        ui.separator();
        grid(ui, entry, tiled, sheet, &mut action);
        ui.separator();
        data(ui, tiled);
        if tiled.is_collection() {
            ui.separator();
            let images = tiled.tiles.values().filter(|tile| tile.image.is_some()).count();
            if ui.button(format!("Show these {images} images")).clicked() {
                action = Some(Action::ShowImages);
            }
            ui.small("This turns on Images in tilesets. The other filters stay as they are.");
        }
        tiles(ui, index, tiled, &mut action);
        ui.separator();
        health(ui, entry, tiled, sheet);
    });
    action
}

/// File, format, and the shape of the set. A person must know which file they
/// read before they trust it.
fn identity(ui: &mut egui::Ui, index: &Index, entry: &Entry, tiled: &Tiled, action: &mut Option<Action>) {
    ui.add(egui::Label::new(egui::RichText::new(file_name(&tiled.file)).strong())).on_hover_text(&tiled.file);
    let kind = if tiled.is_collection() { "image collection" } else { "atlas" };
    let mut line = format!("{}, {kind}", tiled.format.label());
    if !tiled.version.is_empty() {
        line.push_str(&format!(", Tiled {}", tiled.version));
    }
    ui.small(line);
    if !tiled.name.is_empty() && tiled.name != file_name(&tiled.file) {
        ui.small(format!("Name: {}", tiled.name));
    }
    if !tiled.class.is_empty() {
        ui.small(format!("Class: {}", tiled.class));
    }
    let Some(source) = tiled.image() else { return };
    let resolved = tiled::resolve(&index.root, &tiled.file, source);
    if let Some(rel) = &resolved.rel
        && *rel != entry.rel {
        // A second tileset for one image stands on a card of its own, so its
        // image is somewhere else in the library.
        ui.horizontal_wrapped(|ui| {
            ui.weak("Image:");
            if index.position(rel).is_some() {
                if ui.link(file_name(rel)).on_hover_text(rel).clicked() {
                    *action = Some(Action::Jump(rel.clone()));
                }
            } else {
                ui.add(egui::Label::new(file_name(rel)).truncate()).on_hover_text(rel);
            }
        });
    }
    if let Some(problem) = resolved.problem {
        ui.colored_label(ui.visuals().warn_fg_color, problem);
    }
}

/// The grid that the file states, and whether the open sheet follows it.
fn grid(ui: &mut egui::Ui, entry: &Entry, tiled: &Tiled, sheet: Option<&Sheet>, action: &mut Option<Action>) {
    ui.strong("Grid");
    if tiled.is_collection() {
        ui.small("A collection has no grid. Every tile is an image of its own.");
        return;
    }
    if tiled.tile[0] == 0 || tiled.tile[1] == 0 {
        ui.small("The file states no tile size, so no grid follows.");
        return;
    }
    ui.small(format!("Tiles: {} x {} px", tiled.tile[0], tiled.tile[1]));
    if tiled.margin > 0 || tiled.spacing > 0 {
        ui.small(format!("Margin {}, spacing {} px", tiled.margin, tiled.spacing));
    }
    let Some(grid) = tiled.grid() else {
        ui.small("The file does not state the image size, so no grid follows.");
        reports(ui, tiled);
        return;
    };
    ui.small(format!("{} columns, {} rows", grid.cols, grid.rows));
    if tiled.columns == 0 {
        ui.small("The columns come from the image width.");
    }
    if tiled.tilecount == 0 {
        ui.small("The rows come from the image height.");
    }
    ui.small(format!("Image size in the file: {} x {}", grid.image[0], grid.image[1]));
    let Some(sheet) = sheet else { return };
    let actual = [sheet.img.width(), sheet.img.height()];
    if actual != grid.image {
        ui.colored_label(ui.visuals().warn_fg_color, format!("The image is {} x {}.", actual[0], actual[1]));
    }
    if agrees(sheet, &grid) {
        ui.small("The grid agrees with this sheet.");
        return;
    }
    ui.small(format!("The sheet uses {} x {} px tiles, gap {}, offset {}.",
        sheet.tile[0], sheet.tile[1], axes(sheet.gap), axes(sheet.offset)));
    if ui.button("Use the tileset grid").clicked() {
        *action = Some(Action::UseGrid);
    }
    if person_grid(entry) {
        ui.small("Your grid wins over the tileset. The button drops your grid.");
    } else {
        ui.small("The button applies the tileset grid and opens the sheet again.");
    }
}

/// What the file says its columns and tile count are, when no grid follows.
fn reports(ui: &mut egui::Ui, tiled: &Tiled) {
    if tiled.columns > 0 {
        ui.small(format!("Columns: {}", tiled.columns));
    }
    if tiled.tilecount > 0 {
        ui.small(format!("Tile count: {}", tiled.tilecount));
    }
}

/// Whether the sheet uses the grid that the tileset states.
fn agrees(sheet: &Sheet, grid: &Grid) -> bool {
    sheet.tile == grid.tile && sheet.gap == grid.gap
        && sheet.offset == [grid.offset[0].max(0), grid.offset[1].max(0)]
}

/// Whether a person chose the grid of this sheet. A grid that the pixels or
/// the tileset supplied is marked as read, so it does not win.
fn person_grid(entry: &Entry) -> bool {
    entry.side.tile.is_some() && !entry.side.read
}

/// `16`, or `16 x 8` when the two axes differ.
fn axes<T: std::fmt::Display + PartialEq>(v: [T; 2]) -> String {
    if v[0] == v[1] { v[0].to_string() } else { format!("{} x {}", v[0], v[1]) }
}

/// What the tileset holds beyond the pictures.
fn data(ui: &mut egui::Ui, tiled: &Tiled) {
    ui.strong("Data");
    let mut parts = Vec::new();
    if tiled.counts.animated > 0 {
        parts.push(format!("{} animated", tiled.counts.animated));
    }
    if tiled.counts.wangsets > 0 {
        parts.push(format!("{} wang sets", tiled.counts.wangsets));
    }
    if tiled.counts.collisions > 0 {
        parts.push(format!("{} collision shapes", tiled.counts.collisions));
    }
    let classes = tiled.tiles.values().filter(|tile| !tile.class.is_empty()).count();
    if classes > 0 {
        parts.push(format!("{classes} tiles with a class"));
    }
    let properties = tiled.tiles.values().filter(|tile| !tile.properties.is_empty()).count();
    if properties > 0 {
        parts.push(format!("{properties} tiles with properties"));
    }
    if parts.is_empty() {
        ui.small("No tile carries data in this tileset.");
    } else {
        ui.small(parts.join(", "));
    }
    if !tiled.properties.is_empty() {
        egui::CollapsingHeader::new(format!("Tileset properties ({})", tiled.properties.len())).show(ui, |ui| {
            for (name, value) in &tiled.properties {
                ui.small(format!("{name}: {value}"));
            }
        });
    }
}

/// The tiles that a person can find by a link or by their data. A collection
/// lists its images; an atlas lists the tiles that carry a class, an
/// animation, a collision, or a property.
fn tiles(ui: &mut egui::Ui, index: &Index, tiled: &Tiled, action: &mut Option<Action>) {
    if tiled.is_collection() {
        let members: Vec<(&u32, &tiled::Tile)> = tiled.tiles.iter().filter(|(_, tile)| tile.image.is_some()).collect();
        if members.is_empty() {
            ui.small("The file names no tile images.");
            return;
        }
        ui.separator();
        ui.strong(format!("Tiles ({})", members.len()));
        ui.small("Each row opens that image.");
        for (id, tile) in members.iter().take(ROW_LIMIT) {
            ui.push_id(("tile row", id), |ui| {
                let Some(source) = tile.image.as_deref() else { return };
                let resolved = tiled::resolve(&index.root, &tiled.file, source);
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(format!("#{id}"));
                    match &resolved.rel {
                        Some(rel) if index.position(rel).is_some() => {
                            if ui.link(file_name(rel)).on_hover_text(rel).clicked() {
                                *action = Some(Action::Jump(rel.clone()));
                            }
                        }
                        Some(rel) => {
                            ui.add(egui::Label::new(file_name(rel)).truncate()).on_hover_text(rel);
                        }
                        None => {
                            ui.add(egui::Label::new(file_name(source)).truncate()).on_hover_text(source);
                        }
                    }
                });
                if let Some(problem) = &resolved.problem {
                    ui.small(problem);
                }
                tile_data(ui, **id, tile);
            });
        }
        if members.len() > ROW_LIMIT {
            ui.small(format!("{} more tiles are in the file.", members.len() - ROW_LIMIT));
        }
        return;
    }
    let with_data: Vec<(&u32, &tiled::Tile)> = tiled.tiles.iter()
        .filter(|(_, tile)| !tile.class.is_empty() || !tile.frames.is_empty() || tile.collisions > 0 || !tile.properties.is_empty())
        .collect();
    if with_data.is_empty() {
        return;
    }
    ui.separator();
    egui::CollapsingHeader::new(format!("Tiles with data ({})", with_data.len()))
        .default_open(with_data.len() <= 20)
        .show(ui, |ui| {
            for (id, tile) in with_data.iter().take(ROW_LIMIT) {
                ui.push_id(("data tile", id), |ui| {
                    ui.monospace(format!("#{id}"));
                    tile_data(ui, **id, tile);
                });
            }
            if with_data.len() > ROW_LIMIT {
                ui.small(format!("{} more tiles carry data.", with_data.len() - ROW_LIMIT));
            }
        });
}

/// The lines under one tile row. No line at all for a plain tile.
fn tile_data(ui: &mut egui::Ui, id: u32, tile: &tiled::Tile) {
    if tile.class.is_empty() && tile.frames.is_empty() && tile.collisions == 0 && tile.properties.is_empty() {
        return;
    }
    ui.indent(("tile data", id), |ui| {
        if !tile.class.is_empty() {
            ui.small(format!("Class: {}", tile.class));
        }
        if !tile.frames.is_empty() {
            let list = tile.frames.iter().take(12).map(u32::to_string).collect::<Vec<_>>().join(", ");
            let more = if tile.frames.len() > 12 { format!(", and {} more", tile.frames.len() - 12) } else { String::new() };
            ui.small(format!("Animation: {} frames: {list}{more}", tile.frames.len()));
        }
        if tile.collisions > 0 {
            ui.small(format!("{} collision shapes", tile.collisions));
        }
        for (name, value) in &tile.properties {
            ui.small(format!("{name}: {value}"));
        }
    });
}

/// The problems that the file shows by itself, plus what the scan found: a
/// path whose case differs, a second tileset for one image, a size that the
/// image does not have.
fn health(ui: &mut egui::Ui, entry: &Entry, tiled: &Tiled, sheet: Option<&Sheet>) {
    ui.strong("Health");
    let mut issues: Vec<String> = Vec::new();
    if let Some(note) = &entry.tiled_note {
        issues.push(note.clone());
    }
    issues.extend(tiled.health());
    if let Some([width, height]) = tiled.declared_size()
        && let Some(sheet) = sheet {
        let actual = [sheet.img.width(), sheet.img.height()];
        if actual != [width, height] {
            issues.push(format!("The file states an image of {width} x {height}; the image is {} x {}.", actual[0], actual[1]));
        }
    }
    if issues.is_empty() {
        ui.small("No problems found.");
        return;
    }
    for issue in issues {
        ui.colored_label(ui.visuals().warn_fg_color, issue);
    }
}

/// The last part of a path, as Tiled and the cards show it.
fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_number_stands_for_two_equal_axes() {
        assert_eq!(axes([16, 16]), "16");
        assert_eq!(axes([16, 8]), "16 x 8");
        assert_eq!(axes([0, 0]), "0");
    }

    #[test]
    fn a_person_grid_is_the_only_one_that_wins() {
        let mut entry = Entry {
            dir_words: Vec::new(), name_words: Vec::new(), side: Default::default(), rel: "a.png".into(),
            embed: None, text_embed: None, tiled: None, in_tileset: false, tileset_of: None, tileset_only: false,
            tiled_error: None, tiled_note: None,
        };
        assert!(!person_grid(&entry), "no grid at all");
        entry.side.tile = Some(crate::sidecar::Pair::of([16, 16]));
        entry.side.read = true;
        assert!(!person_grid(&entry), "a grid that the tool read");
        entry.side.read = false;
        assert!(person_grid(&entry), "a grid that a person chose");
    }
}
