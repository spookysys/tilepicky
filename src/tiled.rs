// SPDX-License-Identifier: GPL-3.0-only
//! A Tiled tileset read from a library folder, in either the old XML form
//! (`foo.tsx`) or the new JSON one (`foo.tsj`). The library reads these to
//! find the image behind a tileset and the grid that the author wrote.
//!
//! Reading only. A project writes its own tileset beside the image; see
//! `tileset`, which speaks JSON. This module never writes a file.

use crate::tileset::Grid;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Which file format the tileset came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The older XML form, `.tsx`.
    Tsx,
    /// The JSON form, `.tsj`.
    Tsj,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Tsx => "TSX",
            Format::Tsj => "TSJ",
        }
    }
}

/// What an image means to the tileset that names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// One image holds every tile. `image` is the path the file names.
    Atlas { image: String, size: Option<[u32; 2]> },
    /// Every tile names its own image.
    Collection { images: Vec<String> },
}

/// The tile data that a person can see without opening Tiled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Tiles that carry an animation.
    pub animated: usize,
    /// Wang sets, which hold terrain rules.
    pub wangsets: usize,
    /// Collision shapes.
    pub collisions: usize,
}

/// One tileset, in the terms both formats share.
#[derive(Clone, Debug, PartialEq)]
pub struct Tiled {
    /// The tileset file, from the library root.
    pub file: String,
    pub format: Format,
    /// The name the author gave. Empty when the file names none.
    pub name: String,
    /// The Tiled version the file speaks.
    pub version: String,
    pub tile: [u32; 2],
    pub margin: u32,
    pub spacing: u32,
    /// Zero when the file leaves the column count to the image size.
    pub columns: u32,
    /// Zero when the file does not state it.
    pub tilecount: u32,
    pub shape: Shape,
    pub counts: Counts,
}

/// The path of an image, as the library sees it, beside why it is not exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The path under the library root, when the file is there.
    pub rel: Option<String>,
    /// Why the path is missing or not exact, in a sentence for the panel.
    pub problem: Option<String>,
}

impl Tiled {
    /// The image path as the file words it, for an image tileset.
    pub fn image(&self) -> Option<&str> {
        match &self.shape {
            Shape::Atlas { image, .. } => Some(image),
            Shape::Collection { .. } => None,
        }
    }

    pub fn is_collection(&self) -> bool {
        matches!(self.shape, Shape::Collection { .. })
    }

    /// How many tiles the tileset declares.
    pub fn tile_count(&self) -> usize {
        if self.tilecount > 0 {
            return self.tilecount as usize;
        }
        match &self.shape {
            Shape::Collection { images } => images.len(),
            Shape::Atlas { size: Some([w, h]), .. } => {
                let grid = self.grid_with(*w, *h);
                grid.map_or(0, |g| (g.cols as usize) * (g.rows as usize))
            }
            Shape::Atlas { size: None, .. } => 0,
        }
    }

    /// The grid this tileset states, when it describes an image whose size
    /// is known. A grid that a person set outranks it, and it outranks what
    /// the pixels say; see `Sheet::open`.
    pub fn grid(&self) -> Option<Grid> {
        match &self.shape {
            Shape::Atlas { size: Some([w, h]), .. } => self.grid_with(*w, *h),
            _ => None,
        }
    }

    fn grid_with(&self, width: u32, height: u32) -> Option<Grid> {
        if self.tile[0] == 0 || self.tile[1] == 0 {
            return None;
        }
        let cols = if self.columns > 0 {
            self.columns
        } else {
            span(width, self.tile[0], self.margin, self.spacing)
        };
        if cols == 0 {
            return None;
        }
        let rows = if self.tilecount > 0 {
            self.tilecount.div_ceil(cols)
        } else {
            span(height, self.tile[1], self.margin, self.spacing)
        };
        if rows == 0 {
            return None;
        }
        Some(Grid {
            image: [width, height],
            tile: self.tile,
            gap: [self.spacing, self.spacing],
            offset: [self.margin as i32, self.margin as i32],
            cols,
            rows,
        })
    }

    /// A line for a card: `TSX, 3,087 tiles, 32x32, 12 animated`.
    pub fn summary(&self) -> String {
        let tiles = self.tile_count();
        let tile = format!("{}x{}", self.tile[0], self.tile[1]);
        let mut parts = Vec::new();
        if self.is_collection() {
            parts.push(format!("{} image collection", self.format.label()));
        } else {
            parts.push(self.format.label().to_string());
        }
        parts.push(format!("{tiles} tiles"));
        if !self.is_collection() {
            parts.push(tile);
        }
        if self.counts.animated > 0 {
            parts.push(format!("{} animated", self.counts.animated));
        }
        if self.counts.wangsets > 0 {
            parts.push(format!("{} wang sets", self.counts.wangsets));
        }
        if self.counts.collisions > 0 {
            parts.push(format!("{} collision shapes", self.counts.collisions));
        }
        parts.join(", ")
    }
}

/// How many cells of `size` fit, with Tiled's one margin and one spacing.
fn span(size: u32, tile: u32, margin: u32, spacing: u32) -> u32 {
    let Some(inner) = size.checked_sub(margin.saturating_mul(2)) else { return 0 };
    let Some(step) = tile.checked_add(spacing).filter(|step| *step > 0) else { return 0 };
    inner.saturating_add(spacing) / step
}

/// Reads the tileset at `root/rel`. An unreadable file is an error for its
/// card, never for the library: the caller keeps the sheet.
pub fn read(root: &Path, rel: &str) -> Result<Tiled, String> {
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
    let mut tiled = if rel.to_ascii_lowercase().ends_with(".tsj") {
        from_json(rel, &text)?
    } else {
        from_xml(rel, &text)?
    };
    tiled.file = rel.to_string();
    Ok(tiled)
}

fn from_json(file: &str, text: &str) -> Result<Tiled, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("{file}: {e}"))?;
    let object = root.as_object().ok_or_else(|| format!("{file}: not a Tiled tileset"))?;
    let number = |key: &str| object.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
    let text_of = |key: &str| object.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut tiled = Tiled {
        file: file.to_string(),
        format: Format::Tsj,
        name: text_of("name"),
        version: text_of("version"),
        tile: [number("tilewidth"), number("tileheight")],
        margin: number("margin"),
        spacing: number("spacing"),
        columns: number("columns"),
        tilecount: number("tilecount"),
        shape: Shape::Collection { images: Vec::new() },
        counts: Counts::default(),
    };
    let tiles = object.get("tiles").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut collection: Vec<String> = Vec::new();
    for tile in &tiles {
        let mut animated = false;
        if let Some(frames) = tile.get("animation").and_then(Value::as_array)
            && !frames.is_empty() {
            animated = true;
        }
        if animated {
            tiled.counts.animated += 1;
        }
        if let Some(wangsets) = tile.get("wangsets").and_then(Value::as_array) {
            tiled.counts.wangsets += wangsets.len();
        }
        if let Some(objects) = tile.get("objectgroup").and_then(|g| g.get("objects")).and_then(Value::as_array) {
            tiled.counts.collisions += objects.len();
        }
        if let Some(image) = tile.get("image").and_then(Value::as_str) {
            collection.push(image.to_string());
        }
    }
    if let Some(wangsets) = object.get("wangsets").and_then(Value::as_array) {
        tiled.counts.wangsets += wangsets.len();
    }
    tiled.shape = match object.get("image").and_then(Value::as_str) {
        Some(image) => match (number("imagewidth"), number("imageheight")) {
            (0, 0) => Shape::Atlas { image: image.to_string(), size: None },
            (width, height) => Shape::Atlas { image: image.to_string(), size: Some([width, height]) },
        },
        None => Shape::Collection { images: collection },
    };
    Ok(tiled)
}

fn from_xml(file: &str, text: &str) -> Result<Tiled, String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut tiled = Tiled {
        file: file.to_string(),
        format: Format::Tsx,
        name: String::new(),
        version: String::new(),
        tile: [0, 0],
        margin: 0,
        spacing: 0,
        columns: 0,
        tilecount: 0,
        shape: Shape::Collection { images: Vec::new() },
        counts: Counts::default(),
    };
    let mut state = State::default();
    loop {
        let event = reader.read_event().map_err(|e| format!("{file}: {e}"))?;
        match event {
            Event::Start(e) => state.element(&mut tiled, &e, true),
            Event::Empty(e) => state.element(&mut tiled, &e, false),
            Event::End(e) => state.close(&mut tiled, e.name().as_ref()),
            Event::Eof => break,
            _ => {}
        }
    }
    tiled.shape = match state.image {
        Some((image, size)) => Shape::Atlas { image, size },
        None => Shape::Collection { images: state.collection },
    };
    Ok(tiled)
}

/// What the XML reader carries between events.
#[derive(Default)]
struct State {
    image: Option<(String, Option<[u32; 2]>)>,
    collection: Vec<String>,
    in_tile: bool,
    in_objects: bool,
    in_object: bool,
    tile_animated: bool,
}

impl State {
    fn element(&mut self, tiled: &mut Tiled, e: &BytesStart, open: bool) {
        let attrs: BTreeMap<String, String> = e
            .attributes()
            .with_checks(false)
            .flatten()
            .filter_map(|a| Some((a.key.as_ref().to_string(), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok()?.into_owned())))
            .collect();
        let number = |key: &str| attrs.get(key).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        match e.name().as_ref() {
            "tileset" => {
                tiled.name = attrs.get("name").cloned().unwrap_or_default();
                tiled.version = attrs.get("version").cloned().unwrap_or_default();
                tiled.tile = [number("tilewidth"), number("tileheight")];
                tiled.columns = number("columns");
                tiled.tilecount = number("tilecount");
                tiled.margin = number("margin");
                tiled.spacing = number("spacing");
            }
            "image" if !self.in_object => {
                let source = attrs.get("source").cloned().unwrap_or_default();
                if self.in_tile {
                    self.collection.push(source);
                } else if !source.is_empty() {
                    let size = match (number("width"), number("height")) {
                        (0, 0) => None,
                        (w, h) => Some([w, h]),
                    };
                    self.image = Some((source, size));
                }
            }
            "tile" => {
                if open {
                    self.in_tile = true;
                    self.tile_animated = false;
                }
            }
            "animation" if open && self.in_tile && !self.tile_animated => {
                self.tile_animated = true;
                tiled.counts.animated += 1;
            }
            "wangset" => tiled.counts.wangsets += 1,
            "objectgroup" if open && self.in_tile => self.in_objects = true,
            "object" if self.in_objects => {
                tiled.counts.collisions += 1;
                // A collision shape can carry a picture of its own. That
                // picture is not a tile, so it does not join the collection.
                if open {
                    self.in_object = true;
                }
            }
            _ => {}
        }
    }

    fn close(&mut self, _tiled: &mut Tiled, name: &str) {
        match name {
            "tile" => {
                self.in_tile = false;
                self.tile_animated = false;
            }
            "object" => self.in_object = false,
            "objectgroup" => self.in_objects = false,
            _ => {}
        }
    }
}

/// The library path that a tileset's `image` names. A path that is not exact
/// is looked for without regard to case, because a pack made on Windows can
/// name a folder that Linux spells differently.
pub fn resolve(root: &Path, tileset_rel: &str, source: &str) -> Resolved {
    let Some(rel) = join(tileset_rel, source) else {
        return Resolved { rel: None, problem: Some(format!("{source} is not a path inside the library.")) };
    };
    if root.join(&rel).is_file() {
        return Resolved { rel: Some(rel), problem: None };
    }
    let mut at = root.to_path_buf();
    let mut found = Vec::new();
    let mut differs = false;
    for part in rel.split('/') {
        let exact = at.join(part);
        if exact.exists() {
            found.push(part.to_string());
            at = exact;
            continue;
        }
        let Some(name) = folder_entry(&at, part) else {
            return Resolved { rel: None, problem: Some(format!("{rel} is not there.")) };
        };
        differs = true;
        found.push(name.clone());
        at = at.join(name);
    }
    if !at.is_file() {
        return Resolved { rel: None, problem: Some(format!("{rel} is not a file.")) };
    }
    let rel = found.join("/");
    let problem = differs.then(|| format!("The image is {rel}; the letter case differs from the tileset."));
    Resolved { rel: Some(rel), problem }
}

/// Joins a tileset's folder and the path it names, with `..` resolved.
/// `None` when the result leaves the library root.
fn join(tileset_rel: &str, source: &str) -> Option<String> {
    let source = source.replace('\\', "/");
    if source.is_empty() || source.starts_with('/') || source.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut parts: Vec<&str> = tileset_rel.split('/').collect();
    parts.pop();
    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// One entry of `dir` that equals `name`, or differs from it in case only.
fn folder_entry(dir: &Path, name: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut folded = None;
    for entry in entries.flatten() {
        let found = entry.file_name().to_string_lossy().into_owned();
        if found == name {
            return Some(found);
        }
        if folded.is_none() && found.to_lowercase() == name.to_lowercase() {
            folded = Some(found);
        }
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::Folder;

    /// Every tileset file under `root`, sorted.
    fn files(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext != "tsx" && ext != "tsj" {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else { continue };
            out.push(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
        }
        out.sort();
        out
    }

    const ATLAS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tileset version="1.9" tiledversion="1.9.2" name="Terrain" tilewidth="32" tileheight="32" tilecount="3087" columns="49">
 <image source="../../Tilesets/Terrain.png" width="1568" height="2016"/>
 <tile id="164" probability="10"/>
 <tile id="200"><animation><frame tileid="201"/><frame tileid="202"/></animation><objectgroup><object id="1"/><object id="2"/></objectgroup></tile>
 <wangsets><wangset name="sand"/><wangset name="grass"/></wangsets>
</tileset>"#;

    const COLLECTION: &str = r#"<tileset version="1.9" name="Props" tilewidth="32" tileheight="64" tilecount="2" columns="0">
 <grid orientation="orthogonal" width="1" height="1"/>
 <tile id="0"><image width="32" height="64" source="../../Props/a.png"/></tile>
 <tile id="1"><image width="1024" height="64" source="../../Props/b.png"/></tile>
</tileset>"#;

    fn parse_xml(text: &str) -> Tiled {
        from_xml("x.tsx", text).unwrap()
    }

    #[test]
    fn an_xml_atlas_states_its_grid_and_its_data() {
        let tiled = parse_xml(ATLAS);
        assert_eq!(tiled.name, "Terrain");
        assert_eq!(tiled.version, "1.9");
        assert_eq!(tiled.tile, [32, 32]);
        assert_eq!(tiled.columns, 49);
        assert_eq!(tiled.tile_count(), 3087);
        assert_eq!(tiled.image(), Some("../../Tilesets/Terrain.png"));
        assert_eq!(tiled.grid().unwrap(), Grid {
            image: [1568, 2016], tile: [32, 32], gap: [0, 0], offset: [0, 0], cols: 49, rows: 63,
        });
        assert_eq!(tiled.counts, Counts { animated: 1, wangsets: 2, collisions: 2 });
        assert_eq!(tiled.summary(), "TSX, 3087 tiles, 32x32, 1 animated, 2 wang sets, 2 collision shapes");
    }

    /// The children of a tile count once for that tile, and only a tile
    /// with an animation counts as animated.
    #[test]
    fn tile_data_counts_by_tile_not_by_element() {
        let tiled = parse_xml(r#"<tileset tilewidth="8" tileheight="8" tilecount="4" columns="2">
            <image source="a.png" width="16" height="16"/>
            <tile id="0"><animation><frame tileid="1"/></animation><animation><frame tileid="2"/></animation></tile>
            <tile id="1"><objectgroup><object id="1"/></objectgroup></tile>
        </tileset>"#);
        assert_eq!(tiled.counts.animated, 1);
        assert_eq!(tiled.counts.collisions, 1);
        assert_eq!(tiled.grid().unwrap().rows, 2);
    }

    #[test]
    fn an_xml_collection_names_its_tiles_and_states_no_grid() {
        let tiled = parse_xml(COLLECTION);
        assert!(tiled.is_collection());
        assert_eq!(tiled.image(), None);
        assert_eq!(tiled.grid(), None);
        assert_eq!(tiled.tile_count(), 2);
        assert_eq!(tiled.summary(), "TSX image collection, 2 tiles");
    }

    /// A collision shape may carry a picture. It is not a tile, so the
    /// collection keeps only the images that tiles name.
    #[test]
    fn an_image_in_a_collision_shape_is_not_a_collection_member() {
        let tiled = parse_xml(r#"<tileset tilewidth="8" tileheight="8" tilecount="1" columns="1">
            <tile id="0"><objectgroup><object id="1"><image source="marker.png" width="8" height="8"/></object></objectgroup></tile>
        </tileset>"#);
        let Shape::Collection { images } = &tiled.shape else { panic!("a tileset without an atlas image is a collection") };
        assert!(images.is_empty());
        assert_eq!(tiled.counts.collisions, 1);
    }

    #[test]
    fn a_json_tileset_reads_the_same_way() {
        let text = r#"{
            "type": "tileset", "version": "1.10", "name": "Tiles",
            "tilewidth": 16, "tileheight": 16, "columns": 4, "tilecount": 8,
            "image": "tiles.png", "imagewidth": 64, "imageheight": 32,
            "wangsets": [{ "name": "one" }],
            "tiles": [
                { "id": 3, "animation": [{ "tileid": 4, "duration": 100 }] },
                { "id": 5, "objectgroup": { "objects": [{ "id": 1 }, { "id": 2 }] } }
            ]
        }"#;
        let tiled = from_json("t.tsj", text).unwrap();
        assert_eq!(tiled.format, Format::Tsj);
        assert_eq!(tiled.grid().unwrap(), Grid {
            image: [64, 32], tile: [16, 16], gap: [0, 0], offset: [0, 0], cols: 4, rows: 2,
        });
        assert_eq!(tiled.counts, Counts { animated: 1, wangsets: 1, collisions: 2 });
    }

    /// Tiled leaves the column count to the image size when it is zero, and
    /// the margin and the spacing decide the cells that fit.
    #[test]
    fn a_missing_column_count_comes_from_the_image_size() {
        let tiled = parse_xml(r#"<tileset tilewidth="16" tileheight="16" margin="2" spacing="1">
            <image source="a.png" width="88" height="53"/>
        </tileset>"#);
        let grid = tiled.grid().unwrap();
        assert_eq!(grid.cols, 5, "(88 - 4 + 1) / 17");
        assert_eq!(grid.rows, 2, "(53 - 4 + 1) / 17");
        assert_eq!(grid.offset, [2, 2]);
        assert_eq!(grid.gap, [1, 1]);
    }

    #[test]
    fn a_broken_file_is_an_error_with_its_name() {
        assert!(from_xml("broken.tsx", "<tileset").unwrap_err().contains("broken.tsx"));
        assert!(from_json("broken.tsj", "{").unwrap_err().contains("broken.tsj"));
    }

    #[test]
    fn an_image_path_is_resolved_against_the_tileset_folder() {
        assert_eq!(
            join("z/Tiled Map Editor/Tilesets/Terrain.tsx", "../../Tilesets/Terrain.png").unwrap(),
            "z/Tilesets/Terrain.png"
        );
        assert_eq!(join("a/x.tsx", "../y.png").unwrap(), "y.png");
        assert_eq!(join("a/x.tsx", "../../y.png"), None, "above the root");
        assert_eq!(join("x.tsx", "/etc/passwd"), None, "absolute");
        assert_eq!(join("x.tsx", "C:/images/a.png"), None, "a drive letter");
    }

    /// A pack made on Windows can name a folder that Linux spells with
    /// another case. The image is found, and the panel says so.
    #[test]
    fn a_reference_that_differs_in_case_still_finds_its_image() {
        let dir = Folder::new();
        std::fs::create_dir_all(dir.0.join("Tilesets")).unwrap();
        std::fs::create_dir_all(dir.0.join("Props/animated")).unwrap();
        std::fs::write(dir.0.join("Props/animated/Fog.png"), b"").unwrap();
        std::fs::write(dir.0.join("Tilesets/atlas.tsx"), b"").unwrap();

        let exact = resolve(&dir.0, "Tilesets/atlas.tsx", "../Props/animated/Fog.png");
        assert_eq!(exact, Resolved { rel: Some("Props/animated/Fog.png".into()), problem: None });

        let folded = resolve(&dir.0, "Tilesets/atlas.tsx", "../Props/Animated/FOG.png");
        assert_eq!(folded.rel.as_deref(), Some("Props/animated/Fog.png"));
        assert!(folded.problem.unwrap().contains("letter case"));

        let missing = resolve(&dir.0, "Tilesets/atlas.tsx", "../Props/none.png");
        assert_eq!(missing.rel, None);
        assert!(missing.problem.unwrap().contains("not there"));
    }

    /// The pack that this feature was built against. It skips on a machine
    /// without it, like the grid test in `detect`.
    #[test]
    fn the_real_pack_reads_as_it_is() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/erw");
        if !root.is_dir() {
            return;
        }
        let files = files(&root);
        assert_eq!(files.len(), 141, "the pack ships 141 tilesets");
        let mut atlases = 0;
        let mut collections = 0;
        let mut problems = Vec::new();
        for file in &files {
            let tiled = read(&root, file).unwrap();
            match &tiled.shape {
                Shape::Atlas { .. } => atlases += 1,
                Shape::Collection { .. } => collections += 1,
            }
            if let Some(source) = tiled.image() {
                let resolved = resolve(&root, file, source);
                if let Some(problem) = resolved.problem {
                    problems.push((file.clone(), resolved.rel, problem));
                }
            }
        }
        assert_eq!((atlases, collections), (128, 13));
        // Every atlas image is there. The pack's one bad reference is inside
        // a collection tile, and this reader resolves atlas images only.
        assert_eq!(problems, Vec::new());
    }
}
