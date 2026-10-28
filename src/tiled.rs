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

/// The data that one tile carries beyond its picture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    /// The class the author gave it. Tiled 1.9 writes `class`, older files
    /// write `type`.
    pub class: String,
    /// The image of a collection tile, as the file words it.
    pub image: Option<String>,
    /// The tile ids of its animation, in order.
    pub frames: Vec<u32>,
    /// How many collision shapes it holds.
    pub collisions: usize,
    /// Name and value, as Tiled shows them.
    pub properties: Vec<(String, String)>,
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
    /// The class of the tileset itself, if the author gave one.
    pub class: String,
    pub tile: [u32; 2],
    pub margin: u32,
    pub spacing: u32,
    /// Zero when the file leaves the column count to the image size.
    pub columns: u32,
    /// Zero when the file does not state it.
    pub tilecount: u32,
    pub shape: Shape,
    pub counts: Counts,
    /// Properties of the tileset, not of one tile.
    pub properties: Vec<(String, String)>,
    /// The tiles that the file describes, by id.
    pub tiles: BTreeMap<u32, Tile>,
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

    /// The image size that the file states, for an atlas.
    pub fn declared_size(&self) -> Option<[u32; 2]> {
        match &self.shape {
            Shape::Atlas { size: Some(size), .. } => Some(*size),
            _ => None,
        }
    }

    /// Problems that the file shows by itself: a column or tile count that
    /// the stated image size cannot hold, a tile id past the last tile, and
    /// an animation frame that is not a tile of this set.
    pub fn health(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Shape::Atlas { size: Some([width, height]), .. } = self.shape {
            if self.tile[0] > 0 && self.columns > 0 {
                let fit = span(width, self.tile[0], self.margin, self.spacing);
                if fit != self.columns {
                    out.push(format!("columns says {}, but the image width holds {fit}", self.columns));
                }
            }
            if self.tile[1] > 0 && self.columns > 0 {
                let rows = span(height, self.tile[1], self.margin, self.spacing);
                let capacity = self.columns * rows;
                if self.tilecount > capacity {
                    out.push(format!("tilecount is {}, but the image holds {capacity} cells", self.tilecount));
                }
            }
        }
        if let Some(limit) = self.limit()
            && let Some((&id, _)) = self.tiles.iter().find(|(id, _)| **id >= limit) {
            out.push(format!("tile {id} is past the last tile, {limit}"));
        }
        for (id, tile) in &self.tiles {
            if let Some(frame) = tile.frames.iter().find(|frame| !self.has_tile(**frame)) {
                out.push(format!("tile {id} animates to {frame}, which is not a tile here"));
            }
        }
        out
    }

    /// The first id that is not a tile, when the file says. An atlas without
    /// a stated count can still derive one from its image size.
    fn limit(&self) -> Option<u32> {
        if self.tilecount > 0 {
            Some(self.tilecount)
        } else if self.is_collection() {
            None
        } else {
            u32::try_from(self.tile_count()).ok().filter(|count| *count > 0)
        }
    }

    /// Whether `id` names a tile. An atlas with no count and no derivable
    /// grid can hold any id, so that case is not a problem.
    fn has_tile(&self, id: u32) -> bool {
        if self.is_collection() {
            self.tiles.contains_key(&id)
        } else {
            self.limit().is_none_or(|limit| id < limit)
        }
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

/// The bytes of a TSX with its top-level image pointed at `image`. A copy
/// moves the tileset beside the copied image. Every other byte stays as it
/// was, so the fields this reader does not model, such as wang sets and
/// terrains, survive the copy.
pub fn retarget_tsx(bytes: &[u8], image: &str) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("The TSX is not UTF-8: {e}"))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut in_tile = false;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|e| format!("The TSX cannot be read: {e}"))?;
        let end = reader.buffer_position() as usize;
        let mut top_image = false;
        match &event {
            Event::Start(e) => {
                let name = e.name();
                if name.as_ref() == "tile" { in_tile = true; }
                top_image = name.as_ref() == "image" && !in_tile;
            }
            Event::Empty(e) => top_image = e.name().as_ref() == "image" && !in_tile,
            Event::End(e) if e.name().as_ref() == "tile" => in_tile = false,
            _ => {}
        }
        if top_image {
            let raw = &text[start..end];
            let at = raw.find('<').ok_or("The TSX image tag is not a tag.")?;
            let (value_start, value_end) = source_span(&raw[at..])?;
            let escaped = escape_attr(image);
            let mut out = String::with_capacity(text.len() + escaped.len());
            out.push_str(&text[..start + at + value_start]);
            out.push_str(&escaped);
            out.push_str(&text[start + at + value_end..]);
            return Ok(out.into_bytes());
        }
        if matches!(&event, Event::Eof) { break; }
    }
    Err("The TSX states no image at the top level.".into())
}

/// Points the TSX beside `rel` at the image's current file name. A missing
/// file is not an error: the caller moves a pair only when one is there.
pub fn retarget_tsx_file(dir: &Path, rel: &str) -> Result<(), String> {
    let path = dir.join(Path::new(rel).with_extension("tsx"));
    if !path.is_file() { return Ok(()); }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let image = Path::new(rel).file_name().and_then(|s| s.to_str()).ok_or("Invalid sheet name")?;
    let out = retarget_tsx(&bytes, image)?;
    crate::storage::replace(&path, false, |file| std::io::Write::write_all(file, &out))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The span of the `source` value in one start tag. The parser has already
/// accepted the document, so this reads the tag's attributes, not the file.
fn source_span(tag: &str) -> Result<(usize, usize), String> {
    let bytes = tag.as_bytes();
    let mut i = 1;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' && bytes[i] != b'>' { i += 1; }
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
        if i >= bytes.len() || bytes[i] == b'/' || bytes[i] == b'>' { break; }
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' && bytes[i] != b'>' { i += 1; }
        let key = &tag[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
        if i >= bytes.len() || bytes[i] != b'=' { continue; }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
        let quote = *bytes.get(i).ok_or("The TSX image has an unfinished attribute.")?;
        if quote != b'"' && quote != b'\'' { return Err("The TSX image has an attribute without quotes.".into()); }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote { i += 1; }
        if i >= bytes.len() { return Err("The TSX image has a value without its closing quote.".into()); }
        if key == "source" { return Ok((value_start, i)); }
        i += 1;
    }
    Err("The TSX image has no source.".into())
}

/// A file name as an XML attribute value.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A property value as the panel shows it: a string as itself, anything
/// else as the JSON text Tiled wrote.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The `properties` array of a tileset or a tile.
fn json_properties(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(list) = value.and_then(Value::as_array) else { return Vec::new() };
    list.iter().filter_map(|property| {
        let name = property.get("name")?.as_str()?.to_string();
        let value = property.get("value").map(value_text).unwrap_or_default();
        Some((name, value))
    }).collect()
}

/// The text of a general reference `&amp;` as an event hands it over: the
/// bare name, or `#49` for a character reference. An unknown entity keeps
/// its written form, because this reader does not read the DTD.
fn entity_text(e: &quick_xml::events::BytesRef) -> Result<String, String> {
    if let Some(ch) = e.resolve_char_ref().map_err(|error| error.to_string())? {
        return Ok(ch.to_string());
    }
    Ok(match e.as_ref() {
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "amp" => "&".to_string(),
        "apos" => "'".to_string(),
        "quot" => "\"".to_string(),
        name => format!("&{name};"),
    })
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
        class: object.get("class").or_else(|| object.get("type")).and_then(Value::as_str).unwrap_or_default().to_string(),
        tile: [number("tilewidth"), number("tileheight")],
        margin: number("margin"),
        spacing: number("spacing"),
        columns: number("columns"),
        tilecount: number("tilecount"),
        shape: Shape::Collection { images: Vec::new() },
        counts: Counts::default(),
        properties: json_properties(object.get("properties")),
        tiles: BTreeMap::new(),
    };
    let tiles = object.get("tiles").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut collection: Vec<String> = Vec::new();
    for tile in &tiles {
        let id = tile.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
        let data = Tile {
            class: tile.get("class").or_else(|| tile.get("type")).and_then(Value::as_str).unwrap_or_default().to_string(),
            image: tile.get("image").and_then(Value::as_str).map(str::to_string),
            frames: tile.get("animation").and_then(Value::as_array).map(|frames| {
                frames.iter().filter_map(|frame| frame.get("tileid")?.as_u64().map(|id| id as u32)).collect()
            }).unwrap_or_default(),
            collisions: tile.get("objectgroup").and_then(|group| group.get("objects")).and_then(Value::as_array)
                .map_or(0, Vec::len),
            properties: json_properties(tile.get("properties")),
        };
        if !data.frames.is_empty() {
            tiled.counts.animated += 1;
        }
        if let Some(wangsets) = tile.get("wangsets").and_then(Value::as_array) {
            tiled.counts.wangsets += wangsets.len();
        }
        tiled.counts.collisions += data.collisions;
        if let Some(image) = &data.image {
            collection.push(image.clone());
        }
        tiled.tiles.insert(id, data);
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
    // Text keeps its spacing: a property value is the text between its tags,
    // and `trim_text` would run "soft &amp; damp" into "softdamp".
    reader.config_mut().trim_text(false);
    let mut tiled = Tiled {
        file: file.to_string(),
        format: Format::Tsx,
        name: String::new(),
        version: String::new(),
        class: String::new(),
        tile: [0, 0],
        margin: 0,
        spacing: 0,
        columns: 0,
        tilecount: 0,
        shape: Shape::Collection { images: Vec::new() },
        counts: Counts::default(),
        properties: Vec::new(),
        tiles: BTreeMap::new(),
    };
    let mut state = State::default();
    loop {
        let event = reader.read_event().map_err(|e| format!("{file}: {e}"))?;
        match event {
            Event::Start(e) => state.element(&mut tiled, &e, true),
            Event::Empty(e) => state.element(&mut tiled, &e, false),
            Event::End(e) => state.close(&mut tiled, e.name().as_ref()),
            Event::Text(e) => {
                if let Some((_, value)) = &mut state.property {
                    let text: &str = &e;
                    let text = quick_xml::escape::unescape(text).map_err(|error| format!("{file}: {error}"))?;
                    value.push_str(&text);
                }
            }
            Event::GeneralRef(e) => {
                if let Some((_, value)) = &mut state.property {
                    value.push_str(&entity_text(&e)?);
                }
            }
            Event::CData(e) => {
                if let Some((_, value)) = &mut state.property {
                    value.push_str(&e);
                }
            }
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
    /// The tile whose children are being read.
    tile_id: Option<u32>,
    /// A property whose value is the element text, until `</property>`.
    property: Option<(String, String)>,
    /// Whether that property belongs to a tile rather than the tileset.
    property_in_tile: bool,
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
                tiled.class = attrs.get("class").or_else(|| attrs.get("type")).cloned().unwrap_or_default();
                tiled.tile = [number("tilewidth"), number("tileheight")];
                tiled.columns = number("columns");
                tiled.tilecount = number("tilecount");
                tiled.margin = number("margin");
                tiled.spacing = number("spacing");
            }
            "image" if !self.in_object => {
                let source = attrs.get("source").cloned().unwrap_or_default();
                if self.in_tile {
                    if let Some(id) = self.tile_id {
                        tiled.tiles.entry(id).or_default().image = Some(source.clone());
                    }
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
                let id = attrs.get("id").and_then(|v| v.trim().parse::<u32>().ok());
                if let Some(id) = id {
                    let class = attrs.get("class").or_else(|| attrs.get("type")).cloned().unwrap_or_default();
                    let tile = tiled.tiles.entry(id).or_default();
                    if !class.is_empty() {
                        tile.class = class;
                    }
                    if open {
                        self.tile_id = Some(id);
                    }
                }
                if open {
                    self.in_tile = true;
                    self.tile_animated = false;
                }
            }
            "animation" if open && self.in_tile && !self.tile_animated => {
                self.tile_animated = true;
                tiled.counts.animated += 1;
            }
            "frame" if self.in_tile => {
                if let (Some(id), Some(frame)) = (self.tile_id, attrs.get("tileid").and_then(|v| v.trim().parse::<u32>().ok())) {
                    tiled.tiles.entry(id).or_default().frames.push(frame);
                }
            }
            "property" => {
                let name = attrs.get("name").cloned().unwrap_or_default();
                if !name.is_empty() {
                    if let Some(value) = attrs.get("value").cloned() {
                        self.push_property(tiled, name, value);
                    } else if open {
                        self.property = Some((name, String::new()));
                        self.property_in_tile = self.in_tile;
                    } else {
                        self.push_property(tiled, name, String::new());
                    }
                }
            }
            "wangset" => tiled.counts.wangsets += 1,
            "objectgroup" if open && self.in_tile => self.in_objects = true,
            "object" if self.in_objects => {
                tiled.counts.collisions += 1;
                if let Some(id) = self.tile_id {
                    tiled.tiles.entry(id).or_default().collisions += 1;
                }
                // A collision shape can carry a picture of its own. That
                // picture is not a tile, so it does not join the collection.
                if open {
                    self.in_object = true;
                }
            }
            _ => {}
        }
    }

    fn push_property(&self, tiled: &mut Tiled, name: String, value: String) {
        if self.in_tile {
            if let Some(id) = self.tile_id {
                tiled.tiles.entry(id).or_default().properties.push((name, value));
            }
        } else {
            tiled.properties.push((name, value));
        }
    }

    fn finish_property(&mut self, tiled: &mut Tiled) {
        let Some((name, value)) = self.property.take() else { return };
        let value = value.trim().to_string();
        if self.property_in_tile {
            if let Some(id) = self.tile_id {
                tiled.tiles.entry(id).or_default().properties.push((name, value));
            }
        } else {
            tiled.properties.push((name, value));
        }
    }

    fn close(&mut self, tiled: &mut Tiled, name: &str) {
        match name {
            "property" => self.finish_property(tiled),
            "tile" => {
                self.in_tile = false;
                self.tile_animated = false;
                self.tile_id = None;
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
        assert_eq!(tiled.tiles[&164], Tile::default(), "a tile with only a probability has no data to show");
        let animated = &tiled.tiles[&200];
        assert_eq!(animated.frames, [201, 202]);
        assert_eq!(animated.collisions, 2);
        assert_eq!(tiled.declared_size(), Some([1568, 2016]));
        assert!(tiled.health().is_empty(), "this file agrees with itself");
    }

    /// A tile keeps what Tiled shows for it: its class, its animation, its
    /// collision count, and its properties. A property value may be an
    /// attribute or the element text, and the text is unescaped.
    #[test]
    fn an_xml_tile_keeps_its_class_and_properties() {
        let tiled = parse_xml(r#"<tileset tilewidth="8" tileheight="8" tilecount="2" columns="2">
            <image source="a.png" width="16" height="8"/>
            <properties><property name="theme" value="forest"/></properties>
            <tile id="0" type="Rock">
                <properties>
                    <property name="solid" type="bool" value="true"/>
                    <property name="note">soft &amp; damp</property>
                </properties>
            </tile>
        </tileset>"#);
        assert_eq!(tiled.properties, [("theme".to_string(), "forest".to_string())]);
        let tile = &tiled.tiles[&0];
        assert_eq!(tile.class, "Rock");
        assert_eq!(tile.properties, [("solid".to_string(), "true".to_string()), ("note".to_string(), "soft & damp".to_string())]);
    }

    #[test]
    fn health_reports_a_grid_that_the_image_cannot_hold() {
        let tiled = parse_xml(r#"<tileset tilewidth="16" tileheight="16" columns="9" tilecount="20">
            <image source="a.png" width="64" height="32"/>
            <tile id="0"><animation><frame tileid="99"/></animation></tile>
            <tile id="20"><image source="b.png"/></tile>
        </tileset>"#);
        let issues = tiled.health();
        assert!(issues.iter().any(|issue| issue.contains("columns says 9, but the image width holds 4")), "{issues:?}");
        assert!(issues.iter().any(|issue| issue.contains("tilecount is 20, but the image holds 18 cells")), "{issues:?}");
        assert!(issues.iter().any(|issue| issue.contains("tile 20 is past the last tile, 20")), "{issues:?}");
        assert!(issues.iter().any(|issue| issue.contains("tile 0 animates to 99")), "{issues:?}");
    }

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
        assert_eq!(tiled.tiles[&0].frames, [1, 2]);
        assert_eq!(tiled.tiles[&1].collisions, 1);
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
            "type": "tileset", "version": "1.10", "name": "Tiles", "class": "Terrain",
            "tilewidth": 16, "tileheight": 16, "columns": 4, "tilecount": 8,
            "image": "tiles.png", "imagewidth": 64, "imageheight": 32,
            "properties": [{ "name": "theme", "value": "cave" }],
            "wangsets": [{ "name": "one" }],
            "tiles": [
                { "id": 3, "class": "Rock", "properties": [{ "name": "solid", "type": "bool", "value": true }],
                  "animation": [{ "tileid": 4, "duration": 100 }] },
                { "id": 5, "objectgroup": { "objects": [{ "id": 1 }, { "id": 2 }] } }
            ]
        }"#;
        let tiled = from_json("t.tsj", text).unwrap();
        assert_eq!(tiled.format, Format::Tsj);
        assert_eq!(tiled.class, "Terrain");
        assert_eq!(tiled.properties, [("theme".to_string(), "cave".to_string())]);
        assert_eq!(tiled.grid().unwrap(), Grid {
            image: [64, 32], tile: [16, 16], gap: [0, 0], offset: [0, 0], cols: 4, rows: 2,
        });
        assert_eq!(tiled.counts, Counts { animated: 1, wangsets: 1, collisions: 2 });
        assert_eq!(tiled.tiles[&3].class, "Rock");
        assert_eq!(tiled.tiles[&3].frames, [4]);
        assert_eq!(tiled.tiles[&3].properties, [("solid".to_string(), "true".to_string())]);
        assert_eq!(tiled.tiles[&5].collisions, 2);
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

    /// A copied TSX changes only the image path, so the fields this reader
    /// does not model survive the copy.
    #[test]
    fn a_copied_tsx_points_at_the_new_image_and_keeps_every_field() {
        let text = r##"<?xml version="1.0" encoding="UTF-8"?>
<tileset version="1.9" tiledversion="1.9.2" name="Fences &amp; walls" tilewidth="32" tileheight="32" tilecount="2" columns="2">
 <image source="../../Tiles/fences &amp; walls.png" width="64" height="32"/>
 <wangsets><wangset name="walls" type="corner" tile="-1"><wangcolor name="stone" color="#ff0000" tile="-1" probability="1"/></wangset></wangsets>
 <tile id="0"><image source="single.png"/></tile>
</tileset>"##;
        let out = String::from_utf8(retarget_tsx(text.as_bytes(), "fences & walls copy.png").unwrap()).unwrap();
        assert!(out.contains(r#"<image source="fences &amp; walls copy.png" width="64" height="32"/>"#), "{out}");
        assert!(out.contains("<wangsets>"), "a field this reader does not model survives");
        assert!(out.contains(r#"<image source="single.png"/>"#), "an image inside a tile is not the atlas image");
        assert!(out.contains(r#"name="Fences &amp; walls""#), "the rest of the file is byte for byte");
    }

    #[test]
    fn a_tsx_without_a_top_image_is_refused() {
        let text = r#"<tileset name="c" tilewidth="8" tileheight="8" tilecount="1" columns="1">
 <tile id="0"><image source="a.png"/></tile></tileset>"#;
        assert!(retarget_tsx(text.as_bytes(), "b.png").is_err());
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
