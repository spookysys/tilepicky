// SPDX-License-Identifier: GPL-3.0-only
//! Records tile identities and prepares explicit updates of Tiled JSON maps.
//! Formats and GID flags follow https://doc.mapeditor.org/en/stable/reference/json-map-format/
//! and https://doc.mapeditor.org/en/stable/reference/global-tile-ids/.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::{Path, PathBuf}, io::Write};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub session: String,
    pub enabled: bool,
    pub complete: bool,
    /// Original tile ID to its current position. None means the tile was removed.
    pub positions: Vec<Option<u32>>,
    #[serde(default)]
    pub updates: BTreeMap<String, Receipt>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub before_hash: String,
    pub after_hash: String,
    pub before: Vec<Option<u32>>,
    pub after: Vec<Option<u32>>,
}

impl Recording {
    pub fn new(count: u32) -> Self {
        Self {
            session: format!("{}-{}", std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()),
            enabled: true, complete: true, positions: (0..count).map(Some).collect(), updates: BTreeMap::new(),
        }
    }
    pub fn changed(&self) -> usize {
        self.positions.iter().enumerate().filter(|(i, p)| **p != Some(*i as u32)).count()
    }
    pub fn deleted(&self) -> usize { self.positions.iter().filter(|p| p.is_none()).count() }
    pub fn apply(&mut self, mapping: &[Option<u32>]) {
        for p in &mut self.positions { *p = p.and_then(|id| mapping.get(id as usize).copied().flatten()); }
    }
}

pub fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

pub struct Update {
    pub path: PathBuf,
    pub key: String,
    pub original: Vec<u8>,
    pub result: Vec<u8>,
    pub references: usize,
    pub receipt: Receipt,
}

/// A selected map must use the recording's original layout, or a layout in its receipt.
/// Receipts are saved before the map write, so either side of an interrupted write is recognizable.
pub fn prepare(path: &Path, tileset: &Path, recording: &Recording, count: u32) -> Result<Update, String> {
    if !recording.complete { return Err("This recording is incomplete. Clear it before starting a new recording.".into()); }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let original = std::fs::read(&path).map_err(|e| e.to_string())?;
    let mut map: Value = serde_json::from_slice(&original).map_err(|e| format!("Use a Tiled JSON map (.tmj): {e}"))?;
    if map["type"] != "map" { return Err("The selected file is not a Tiled JSON map.".into()); }
    let key = hash(path.to_string_lossy().as_bytes());
    let before_hash = hash(&original);
    let before = if let Some(receipt) = recording.updates.get(&key) {
        if before_hash == receipt.after_hash { receipt.after.clone() }
        else if before_hash == receipt.before_hash { receipt.before.clone() }
        else { return Err("This map changed outside Tilepicky after its last update. Its layout must be checked before updating again.".into()); }
    } else { (0..recording.positions.len() as u32).map(Some).collect() };
    let tileset = tileset.canonicalize().map_err(|e| format!("Save the sheet and its tileset first: {e}"))?;
    let sets = map["tilesets"].as_array().ok_or("The map has no tilesets.")?;
    let mut first = None;
    let mut starts = Vec::new();
    for set in sets {
        let start = uint(&set["firstgid"])?;
        if start == 0 || start > 0x0fff_ffff || starts.contains(&start) { return Err("The map has invalid tileset ranges.".into()); }
        starts.push(start);
        if let Some(source) = set["source"].as_str()
            && path.parent().unwrap().join(source).canonicalize().ok().as_ref() == Some(&tileset)
            && first.replace(start).is_some() { return Err("The map references this tileset more than once.".into()); }
    }
    let first = first.ok_or("This map does not reference this sheet's external .tsj tileset.")?;
    let end = starts.into_iter().filter(|s| *s > first).min().unwrap_or(0x1000_0000);
    if first.checked_add(count).is_none_or(|v| v > end) {
        return Err("The resized tileset overlaps another tileset's ID range in this map. No update was made.".into());
    }
    let mut remap = BTreeMap::new();
    for (old, new) in before.iter().zip(&recording.positions) {
        if let Some(old) = old { remap.insert(*old, *new); }
    }
    let mut references = 0;
    let mut change = |value: &mut Value| -> Result<(), String> {
        let gid = uint(value)?;
        let id = gid & 0x0fff_ffff;
        if id == 0 || id < first || id >= end { return Ok(()); }
        let local = id - first;
        let new = remap.get(&local).ok_or("The map uses tiles outside the recorded layout. No update was made.")?
            .ok_or_else(|| format!("This map uses removed tile {local}. Undo its removal or repair the map in Tiled."))?;
        if new >= count { return Err("A recorded tile lies outside the saved sheet.".into()); }
        if new != local { *value = Value::from((gid & 0xf000_0000) | (first + new)); references += 1; }
        Ok(())
    };
    visit_layers(map.get_mut("layers").ok_or("The map has no layers.")?, &mut change)?;
    let result = if references == 0 { original.clone() } else { serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())? };
    let receipt = Receipt { before_hash, after_hash: hash(&result), before, after: recording.positions.clone() };
    Ok(Update { path, key, original, result, references, receipt })
}

fn uint(v: &Value) -> Result<u32, String> {
    v.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| "The map contains an invalid tile ID.".into())
}

fn visit_layers(layers: &mut Value, change: &mut impl FnMut(&mut Value) -> Result<(), String>) -> Result<(), String> {
    for layer in layers.as_array_mut().ok_or("Invalid map layers.")? {
        match layer["type"].as_str().ok_or("A map layer has no type.")? {
            "group" => visit_layers(&mut layer["layers"], change)?,
            "tilelayer" => {
                if layer.get("chunks").is_some() {
                    for chunk in layer["chunks"].as_array_mut().ok_or("Invalid map chunks.")? { change_data(&mut chunk["data"], change)?; }
                } else { change_data(&mut layer["data"], change)?; }
            }
            "objectgroup" => {
                for object in layer["objects"].as_array_mut().ok_or("Invalid map objects.")? {
                    if object.get("template").is_some() { return Err("Object templates are not supported by map updates yet.".into()); }
                    if let Some(gid) = object.get_mut("gid") { change(gid)?; }
                }
            }
            "imagelayer" => (),
            _ => return Err("This map contains an unsupported layer type.".into()),
        }
    }
    Ok(())
}
fn change_data(data: &mut Value, change: &mut impl FnMut(&mut Value) -> Result<(), String>) -> Result<(), String> {
    for gid in data.as_array_mut().ok_or("Use JSON arrays for tile data. Encoded or compressed layers are not supported yet.")? { change(gid)?; }
    Ok(())
}

impl Update {
    /// Check again after the preview. Never replace a map changed by another program.
    pub fn verify(&self) -> Result<(), String> {
        if std::fs::read(&self.path).map_err(|e| e.to_string())? != self.original {
            return Err("The map changed since the preview. Select it again.".into());
        }
        Ok(())
    }
    pub fn write(&self) -> Result<PathBuf, String> {
        self.verify()?;
        let name = self.path.file_name().unwrap().to_string_lossy();
        let backup = self.path.with_file_name(format!("{name}.{}.bak", &self.receipt.before_hash[..12]));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
            Ok(mut f) => { f.write_all(&self.original).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?; }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(&backup).map_err(|e| e.to_string())? != self.original { return Err("The backup file has different contents.".into()); }
            }
            Err(e) => return Err(e.to_string()),
        }
        crate::storage::replace(&self.path, false, |f| f.write_all(&self.result)).map_err(|e| e.to_string())?;
        Ok(backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::Folder;
    use serde_json::json;

    fn fixture() -> (Folder, PathBuf, PathBuf, Recording) {
        let dir = Folder::new();
        let tileset = dir.0.join("tiles.tsj");
        std::fs::write(&tileset, b"{}").unwrap();
        let path = dir.0.join("map.tmj");
        let map = json!({"type":"map", "tilesets":[{"firstgid":1,"source":"tiles.tsj"},{"firstgid":10,"source":"other.tsj"}],
            "layers":[{"type":"group","layers":[{"type":"tilelayer","data":[0,1,2,0x80000001u32,10]},
                {"type":"objectgroup","objects":[{"id":1,"gid":0xf0000002u32,"x":17}]}]},
                {"type":"tilelayer","chunks":[{"x":-4,"y":0,"width":2,"height":1,"data":[2,1]}]}],
            "properties":[{"name":"unchanged","value":"yes"}]});
        crate::storage::write(&path, &map).unwrap();
        let mut recording = Recording::new(3);
        recording.apply(&[Some(2), Some(0), Some(1)]);
        (dir, path, tileset, recording)
    }

    #[test]
    fn updates_layers_chunks_objects_and_flags_with_backup_and_no_double_application() {
        let (_dir, path, tileset, mut r) = fixture();
        let update = prepare(&path, &tileset, &r, 3).unwrap();
        assert_eq!(update.references, 6);
        let value: Value = serde_json::from_slice(&update.result).unwrap();
        assert_eq!(value["layers"][0]["layers"][0]["data"], json!([0,3,1,0x80000003u32,10]));
        assert_eq!(value["layers"][0]["layers"][1]["objects"][0]["gid"], json!(0xf0000001u32));
        assert_eq!(value["properties"][0]["value"], "yes");
        let backup = update.write().unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), update.original);
        r.updates.insert(update.key.clone(), update.receipt);
        assert_eq!(prepare(&path, &tileset, &r, 3).unwrap().references, 0);
        // Undo the rearrangement after a map update. The receipt supplies its actual layout.
        r.positions = (0..3).map(Some).collect();
        let reversed = prepare(&path, &tileset, &r, 3).unwrap();
        let value: Value = serde_json::from_slice(&reversed.result).unwrap();
        let original: Value = serde_json::from_slice(&update.original).unwrap();
        assert_eq!(value, original);
    }

    #[test]
    fn a_receipt_written_before_an_interrupted_update_can_retry() {
        let (_dir, path, tileset, mut r) = fixture();
        let u = prepare(&path, &tileset, &r, 3).unwrap();
        r.updates.insert(u.key, u.receipt);
        assert_eq!(prepare(&path, &tileset, &r, 3).unwrap().references, 6);
    }

    #[test]
    fn deletion_compression_templates_and_range_collisions_refuse_without_writes() {
        let (_dir, path, tileset, mut r) = fixture();
        let original = std::fs::read(&path).unwrap();
        r.positions[0] = None;
        assert!(prepare(&path, &tileset, &r, 3).err().unwrap().contains("removed"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        r.positions[0] = Some(2);
        assert!(prepare(&path, &tileset, &r, 10).err().unwrap().contains("overlaps"));
        let mut map: Value = serde_json::from_slice(&original).unwrap();
        map["layers"][0]["layers"][0]["data"] = json!("encoded");
        crate::storage::write(&path, &map).unwrap();
        assert!(prepare(&path, &tileset, &r, 3).err().unwrap().contains("Encoded"));
        map["layers"][0]["layers"][0]["data"] = json!([1]);
        map["layers"][0]["layers"][1]["objects"][0]["template"] = json!("tree.tx");
        crate::storage::write(&path, &map).unwrap();
        assert!(prepare(&path, &tileset, &r, 3).err().unwrap().contains("templates"));
    }

    #[test]
    fn changed_files_and_different_tilesets_are_rejected() {
        let (dir, path, tileset, mut r) = fixture();
        let u = prepare(&path, &tileset, &r, 3).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(u.write().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"changed");
        std::fs::write(&path, &u.original).unwrap();
        r.updates.insert(u.key, u.receipt);
        let mut map: Value = serde_json::from_slice(&u.original).unwrap();
        map["custom"] = json!(true);
        crate::storage::write(&path, &map).unwrap();
        assert!(prepare(&path, &tileset, &r, 3).err().unwrap().contains("outside Tilepicky"));
        let other = dir.0.join("different.tsj");
        std::fs::write(&other, b"{}").unwrap();
        r.updates.clear();
        assert!(prepare(&path, &other, &r, 3).err().unwrap().contains("does not reference"));
    }
}
