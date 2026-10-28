// SPDX-License-Identifier: GPL-3.0-only
//! The Tiled tileset beside a project image: `foo.png` and `foo.tsj`. It
//! holds the grid and the properties of each tile in the shape Tiled reads,
//! so that a map can use the tilesheet with its metadata.
//!
//! The file is the truth for those properties. A property kind this tool
//! does not edit, such as a color, a file, or a collision shape beside it,
//! stays as Tiled wrote it.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The JSON format version this writer speaks. A file that names a newer
/// one keeps its own.
const VERSION: &str = "1.10";

/// The properties of each tile, by tile id. A tile id is
/// `row * columns + column`, the numbering Tiled uses for an image tileset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    pub tiles: BTreeMap<u32, BTreeMap<String, Prop>>,
    /// Per-tile fields that the property panel cannot edit.
    extra: BTreeMap<u32, Map<String, Value>>,
    /// Tileset fields to preserve when Save As creates a new file.
    fields: Map<String, Value>,
}

/// One property value. The kind belongs to the value, so a property cannot
/// hold a kind that does not match it.
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// The kinds a person can give a tile here. Tiled has more; those stay
/// where Tiled wrote them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Str,
    Int,
    Float,
    Bool,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Str, Kind::Int, Kind::Float, Kind::Bool];

    /// The name Tiled shows for this kind.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Str => "string",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Bool => "bool",
        }
    }

    /// The value a new property of this kind starts with.
    pub fn start(self) -> Prop {
        match self {
            Kind::Str => Prop::Str(String::new()),
            Kind::Int => Prop::Int(0),
            Kind::Float => Prop::Float(0.0),
            Kind::Bool => Prop::Bool(false),
        }
    }
}

impl Prop {
    pub fn kind(&self) -> Kind {
        match self {
            Prop::Str(_) => Kind::Str,
            Prop::Int(_) => Kind::Int,
            Prop::Float(_) => Kind::Float,
            Prop::Bool(_) => Kind::Bool,
        }
    }

    /// Reads a Tiled property this tool edits, or `None` for a kind it
    /// leaves alone. A value of the wrong JSON shape reads as the kind's
    /// zero, which is what Tiled does with it too.
    fn read(p: &Value) -> Option<Prop> {
        match p.get("type").and_then(Value::as_str).unwrap_or("string") {
            "string" => Some(Prop::Str(p.get("value").and_then(Value::as_str).unwrap_or_default().to_string())),
            "int" => Some(Prop::Int(p.get("value").and_then(Value::as_i64).unwrap_or(0))),
            "float" => Some(Prop::Float(p.get("value").and_then(Value::as_f64).unwrap_or(0.0))),
            "bool" => Some(Prop::Bool(p.get("value").and_then(Value::as_bool).unwrap_or(false))),
            _ => None,
        }
    }

    fn json(&self, name: &str) -> Value {
        let (kind, value) = match self {
            Prop::Str(s) => (Kind::Str, json!(s)),
            Prop::Int(i) => (Kind::Int, json!(i)),
            Prop::Float(f) => (Kind::Float, json!(f)),
            Prop::Bool(b) => (Kind::Bool, json!(b)),
        };
        json!({ "name": name, "type": kind.label(), "value": value })
    }
}

impl Props {
    /// Whether no tile has a property.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty() && self.extra.is_empty()
    }

    pub fn needs_file(&self) -> bool { !self.is_empty() || !self.fields.is_empty() }

    pub fn ids(&self) -> BTreeSet<u32> {
        self.tiles.keys().chain(self.extra.keys()).copied().collect()
    }

    /// Copies tile metadata to new ids. An omitted id has no destination.
    pub fn remap(&self, map: impl Fn(u32) -> Option<u32>) -> Self {
        let mut out = Self { fields: self.fields.clone(), ..Self::default() };
        for (&id, values) in &self.tiles {
            if let Some(to) = map(id) { out.tiles.insert(to, values.clone()); }
        }
        for (&id, fields) in &self.extra {
            if let Some(to) = map(id) {
                let mut fields = fields.clone();
                if let Some(frames) = fields.get_mut("animation").and_then(Value::as_array_mut) {
                    for frame in frames {
                        if let Some(id) = frame.get("tileid").and_then(Value::as_u64)
                            && let Some(to) = map(id as u32) {
                            frame["tileid"] = json!(to);
                        }
                    }
                }
                out.extra.insert(to, fields);
            }
        }
        out
    }

    /// Keeps cell coordinates when the canvas width changes.
    pub fn resized(&self, old_cols: u32, cols: u32, rows: u32) -> Self {
        self.remap(|id| {
            let (x, y) = (id % old_cols, id / old_cols);
            (x < cols && y < rows).then_some(y * cols + x)
        })
    }

    pub fn clear_tile(&mut self, id: u32) {
        self.tiles.remove(&id);
        self.extra.remove(&id);
    }

    pub fn copy_tile(&mut self, id: u32, source: &Self, from: u32) {
        self.clear_tile(id);
        if let Some(values) = source.tiles.get(&from) { self.tiles.insert(id, values.clone()); }
        if let Some(fields) = source.extra.get(&from) { self.extra.insert(id, fields.clone()); }
    }

    /// Every property name that any of these tiles has.
    pub fn names(&self, ids: &[u32]) -> BTreeSet<String> {
        ids.iter().filter_map(|id| self.tiles.get(id)).flat_map(|m| m.keys().cloned()).collect()
    }

    /// The value all of these tiles share. `None` when one of them lacks the
    /// property or holds another value, which is what the panel calls mixed.
    pub fn common(&self, ids: &[u32], name: &str) -> Option<Prop> {
        let first = self.sample(ids, name)?;
        ids.iter().all(|id| self.tiles.get(id).and_then(|m| m.get(name)) == Some(&first)).then_some(first)
    }

    /// The value the panel edits first: the one of the first tile that has
    /// the property.
    pub fn sample(&self, ids: &[u32], name: &str) -> Option<Prop> {
        ids.iter().find_map(|id| self.tiles.get(id).and_then(|m| m.get(name)).cloned())
    }

    /// Gives the property to all of these tiles.
    pub fn set(&mut self, ids: &[u32], name: &str, prop: Prop) {
        for id in ids {
            self.tiles.entry(*id).or_default().insert(name.to_string(), prop.clone());
        }
    }

    /// Takes the property from all of these tiles.
    pub fn remove(&mut self, ids: &[u32], name: &str) {
        for id in ids {
            if let Some(map) = self.tiles.get_mut(id) {
                map.remove(name);
                if map.is_empty() {
                    self.tiles.remove(id);
                }
            }
        }
    }

    fn from_tileset(root: &Map<String, Value>) -> Props {
        let mut props = Props { fields: root.clone(), ..Props::default() };
        props.fields.remove("tiles");
        for tile in root.get("tiles").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = tile.get("id").and_then(Value::as_u64) else { continue };
            let mut extra = tile.as_object().cloned().unwrap_or_default();
            extra.remove("id");
            let list = extra.remove("properties").and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let kept: Vec<_> = list.iter().filter(|p| Prop::read(p).is_none()).cloned().collect();
            if !kept.is_empty() { extra.insert("properties".into(), Value::Array(kept)); }
            if !extra.is_empty() { props.extra.insert(id as u32, extra); }
            for p in &list {
                let Some(name) = p.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { continue };
                if let Some(prop) = Prop::read(p) {
                    props.tiles.entry(id as u32).or_default().insert(name.to_string(), prop);
                }
            }
        }
        props
    }
}

/// The grid of a tilesheet in the terms a Tiled tileset uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// The image size in pixels.
    pub image: [u32; 2],
    pub tile: [u32; 2],
    pub gap: [u32; 2],
    /// Pixels before the first tile.
    pub offset: [i32; 2],
    pub cols: u32,
    pub rows: u32,
}

/// Says why Tiled cannot hold this grid, when it cannot. Tiled has one
/// margin and one spacing, so a grid that differs on the two axes has no
/// honest file, and a grid that starts before the image has no margin at all.
pub fn check(grid: Grid) -> Result<(), String> {
    if grid.offset[0] < 0 || grid.offset[1] < 0 {
        return Err("The grid starts before the image edge. Tiled has no negative margin.".into());
    }
    if grid.offset[0] != grid.offset[1] {
        return Err("The grid offsets differ on the two axes. Tiled has one margin.".into());
    }
    if grid.gap[0] != grid.gap[1] {
        return Err("The grid gaps differ on the two axes. Tiled has one spacing.".into());
    }
    if grid.cols == 0 || grid.rows == 0 {
        return Err("The sheet has no tiles.".into());
    }
    Ok(())
}

/// The tileset name that belongs to `rel`, beside the image: `sub/foo.png`
/// gives `sub/foo.tsj`.
pub fn file_name(rel: &str) -> PathBuf {
    Path::new(rel).with_extension("tsj")
}

pub fn path(dir: &Path, rel: &str) -> PathBuf {
    dir.join(file_name(rel))
}

/// Reads the properties of a tileset beside the image. A file that is not
/// there reads as none; a file that cannot be read is an error, so that a
/// save never writes over a file this tool did not understand.
pub fn load(dir: &Path, rel: &str) -> Result<Props, String> {
    let path = path(dir, rel);
    if !path.exists() {
        // A copied TSX is the truth for Tiled, and this tool cannot write XML.
        // Refuse the panel instead of writing a second tileset besides it.
        let tsx = Path::new(rel).with_extension("tsx");
        if dir.join(&tsx).is_file() {
            return Err(format!("{} is a Tiled TSX. Tilepicky writes TSJ only, so this sheet's tile properties stay read-only here; edit the TSX in Tiled.",
                tsx.display()));
        }
    }
    match crate::storage::read::<Value>(&path)? {
        Value::Null => Ok(Props::default()),
        Value::Object(map) => Ok(Props::from_tileset(&map)),
        _ => Err(format!("{} is not a Tiled tileset: its top level is not an object.", path.display())),
    }
}

/// Writes the tileset beside the image, keeping what Tiled put there and
/// this tool does not read. The grid is rewritten from the sheet, so the
/// file always describes the image as it is now.
#[cfg(test)]
pub fn write(dir: &Path, rel: &str, grid: Grid, props: &Props) -> Result<(), String> {
    crate::storage::write(&path(dir, rel), &prepare(dir, rel, grid, props)?)
}

/// Reads and validates the destination before the caller writes the image.
pub fn prepare(dir: &Path, rel: &str, grid: Grid, props: &Props) -> Result<Value, String> {
    check(grid)?;
    let path = path(dir, rel);
    let stem = Path::new(rel).file_stem().and_then(|s| s.to_str()).unwrap_or("tiles").to_string();
    let image = Path::new(rel).file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    let mut root = match crate::storage::read::<Value>(&path)? {
        Value::Null => props.fields.clone(),
        Value::Object(map) => map,
        _ => return Err(format!("{} is not a Tiled tileset: its top level is not an object.", path.display())),
    };
    root.entry("type").or_insert(json!("tileset"));
    root.entry("version").or_insert(json!(VERSION));
    // A name chosen in Tiled stays; a new file takes the image's name.
    root.entry("name").or_insert(json!(stem));
    root.insert("tilewidth".into(), json!(grid.tile[0]));
    root.insert("tileheight".into(), json!(grid.tile[1]));
    root.insert("columns".into(), json!(grid.cols));
    root.insert("tilecount".into(), json!(grid.cols as u64 * grid.rows as u64));
    root.insert("margin".into(), json!(grid.offset[0]));
    root.insert("spacing".into(), json!(grid.gap[0]));
    root.insert("image".into(), json!(image));
    root.insert("imagewidth".into(), json!(grid.image[0]));
    root.insert("imageheight".into(), json!(grid.image[1]));
    let tiles = tiles_json(grid, props);
    if tiles.is_empty() {
        root.remove("tiles");
    } else {
        root.insert("tiles".into(), Value::Array(tiles));
    }
    Ok(Value::Object(root))
}

/// Writes the current tile metadata, including fields the panel cannot edit.
fn tiles_json(grid: Grid, props: &Props) -> Vec<Value> {
    let count = grid.cols as u64 * grid.rows as u64;
    let ids: Vec<_> = props.ids().into_iter().filter(|&id| (id as u64) < count).collect();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let mut tile = props.extra.get(&id).cloned().unwrap_or_default();
        let mut list: Vec<Value> = tile
            .remove("properties")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| Prop::read(p).is_none())
            .collect();
        if let Some(mine) = props.tiles.get(&id) {
            list.extend(mine.iter().map(|(name, prop)| prop.json(name)));
        }
        if !list.is_empty() {
            tile.insert("properties".into(), Value::Array(list));
        }
        if !tile.is_empty() {
            tile.insert("id".into(), json!(id));
            out.push(Value::Object(tile));
        }
    }
    out
}

/// Points the tileset of `rel` at its image, after the image was renamed or
/// copied. Does nothing when there is no tileset.
pub fn retarget(dir: &Path, rel: &str) -> Result<(), String> {
    let path = path(dir, rel);
    if !path.is_file() {
        return Ok(());
    }
    let mut root = match crate::storage::read::<Value>(&path)? {
        Value::Object(map) => map,
        _ => return Err(format!("{} is not a Tiled tileset: its top level is not an object.", path.display())),
    };
    let image = Path::new(rel).file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    root.insert("image".into(), json!(image));
    crate::storage::write(&path, &Value::Object(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::Folder;

    fn grid(cols: u32, rows: u32) -> Grid {
        Grid { image: [cols * 16, rows * 16], tile: [16, 16], gap: [0, 0], offset: [0, 0], cols, rows }
    }

    fn read(dir: &Path, name: &str) -> Value {
        serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap()
    }

    #[test]
    fn a_new_file_names_the_image_and_holds_its_grid() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        let mut props = Props::default();
        props.set(&[3], "walkable", Prop::Bool(true));
        write(&dir.0, "tiles.png", grid(4, 2), &props).unwrap();
        let file = read(&dir.0, "tiles.tsj");
        assert_eq!(file["type"], "tileset");
        assert_eq!(file["name"], "tiles");
        assert_eq!(file["image"], "tiles.png");
        assert_eq!(file["imagewidth"], 64);
        assert_eq!(file["imageheight"], 32);
        assert_eq!(file["tilewidth"], 16);
        assert_eq!(file["columns"], 4);
        assert_eq!(file["tilecount"], 8);
        assert_eq!(file["tiles"][0]["id"], 3);
        assert_eq!(file["tiles"][0]["properties"][0]["name"], "walkable");
        assert_eq!(file["tiles"][0]["properties"][0]["type"], "bool");
        assert_eq!(file["tiles"][0]["properties"][0]["value"], true);
        assert_eq!(load(&dir.0, "tiles.png").unwrap().tiles, props.tiles);
    }

    #[test]
    fn a_saved_file_keeps_what_tiled_wrote_and_this_tool_does_not_read() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        std::fs::write(dir.0.join("tiles.tsj"), br##"{
            "type": "tileset", "version": "1.10", "name": "chosen in Tiled",
            "tilewidth": 16, "tileheight": 16, "columns": 4, "tilecount": 8,
            "image": "tiles.png", "imagewidth": 64, "imageheight": 32,
            "editorsettings": { "export": { "target": "out.json" } },
            "tiles": [
                { "id": 3, "probability": 0.5, "type": "floor",
                  "properties": [
                    { "name": "tint", "type": "color", "value": "#ff0000ff" },
                    { "name": "cost", "type": "int", "value": 2 } ] },
                { "id": 9, "properties": [ { "name": "gone", "type": "string", "value": "past the last tile" } ] }
            ]
        }"##).unwrap();
        let props = load(&dir.0, "tiles.png").unwrap();
        assert_eq!(props.tiles[&3]["cost"], Prop::Int(2));
        assert_eq!(props.tiles[&9]["gone"], Prop::Str("past the last tile".into()), "the reader does not know the count");
        write(&dir.0, "tiles.png", grid(4, 2), &props).unwrap();
        let file = read(&dir.0, "tiles.tsj");
        assert_eq!(file["name"], "chosen in Tiled");
        assert_eq!(file["editorsettings"]["export"]["target"], "out.json");
        assert_eq!(file["tiles"][0]["probability"], 0.5);
        assert_eq!(file["tiles"][0]["type"], "floor");
        let kept: Vec<&str> = file["tiles"][0]["properties"].as_array().unwrap().iter()
            .map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(kept, ["tint", "cost"]);
        assert_eq!(file["tiles"].as_array().unwrap().len(), 1, "the entry past the last tile goes");
    }

    #[test]
    fn a_property_taken_from_a_tile_leaves_no_empty_entry() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        let mut props = Props::default();
        props.set(&[0], "a", Prop::Str("x".into()));
        props.remove(&[0], "a");
        assert!(props.tiles.is_empty());
        write(&dir.0, "tiles.png", grid(2, 2), &props).unwrap();
        let file = read(&dir.0, "tiles.tsj");
        assert!(file.get("tiles").is_none(), "no properties means no tiles array");
        assert!(load(&dir.0, "tiles.png").unwrap().is_empty());
    }

    #[test]
    fn a_selection_shares_a_value_only_when_every_tile_holds_it() {
        let mut props = Props::default();
        props.set(&[1, 2], "solid", Prop::Bool(true));
        assert_eq!(props.names(&[1, 2]), ["solid".to_string()].into());
        assert_eq!(props.common(&[1, 2], "solid"), Some(Prop::Bool(true)));
        assert_eq!(props.common(&[1, 3], "solid"), None, "a tile without it makes the row mixed");
        props.set(&[2], "solid", Prop::Bool(false));
        assert_eq!(props.common(&[1, 2], "solid"), None, "two values make the row mixed");
        assert_eq!(props.sample(&[1, 2], "solid"), Some(Prop::Bool(true)));
    }

    #[test]
    fn a_grid_tiled_cannot_hold_is_refused_before_anything_is_written() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        let mut offset = grid(2, 2);
        offset.offset = [-4, 0];
        assert!(write(&dir.0, "tiles.png", offset, &Props::default()).unwrap_err().contains("negative margin"));
        let mut axes = grid(2, 2);
        axes.gap = [1, 2];
        assert!(write(&dir.0, "tiles.png", axes, &Props::default()).unwrap_err().contains("one spacing"));
        assert!(!dir.0.join("tiles.tsj").exists());
    }

    #[test]
    fn a_broken_file_is_an_error_and_stays_untouched() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        std::fs::write(dir.0.join("tiles.tsj"), b"{broken").unwrap();
        assert!(load(&dir.0, "tiles.png").is_err());
        assert!(write(&dir.0, "tiles.png", grid(2, 2), &Props::default()).is_err());
        assert_eq!(std::fs::read(dir.0.join("tiles.tsj")).unwrap(), b"{broken");
    }

    /// A TSX beside the sheet is the truth for Tiled. The panel stays
    /// read-only instead of writing a second tileset beside it.
    #[test]
    fn a_tsx_beside_the_sheet_makes_the_panel_read_only() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tiles.png"), b"").unwrap();
        std::fs::write(dir.0.join("tiles.tsx"), b"<tileset/>").unwrap();
        let error = load(&dir.0, "tiles.png").unwrap_err();
        assert!(error.contains("writes TSJ only"), "{error}");
        assert!(!dir.0.join("tiles.tsj").exists());
    }

    #[test]
    fn a_rename_moves_the_image_name_inside_the_file() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("old.png"), b"").unwrap();
        write(&dir.0, "old.png", grid(2, 2), &Props::default()).unwrap();
        std::fs::rename(dir.0.join("old.png"), dir.0.join("new.png")).unwrap();
        std::fs::rename(dir.0.join("old.tsj"), dir.0.join("new.tsj")).unwrap();
        retarget(&dir.0, "new.png").unwrap();
        assert_eq!(read(&dir.0, "new.tsj")["image"], "new.png");
        retarget(&dir.0, "none.png").unwrap();
    }
}
