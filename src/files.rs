// SPDX-License-Identifier: GPL-3.0-only
//! Project file operations keep their folder book in step with the files.

use crate::sidecar;
use std::path::Path;

pub fn normalize_name(name: &str, ext: Option<&str>) -> Option<String> {
    let name = name.trim().trim_matches('/');
    if name.is_empty() || name.contains(['\\', ':']) || name.split('/').any(|p| p.trim().is_empty() || p == "." || p == "..") {
        return None;
    }
    let mut rel = name.to_string();
    if let Some(ext) = ext && !rel.to_ascii_lowercase().ends_with(ext) { rel.push_str(ext); }
    Some(rel)
}

pub fn relocate(root: &Path, old: &str, new: &str, copy: bool) -> Result<(), String> {
    if old == new { return Ok(()); }
    if root.join(new).exists() { return Err(format!("{new} exists")); }
    sidecar::load_book(root)?;
    let directory = root.join(old).is_dir();
    if directory && copy { return Err("Copy folders one file at a time.".into()); }
    if let Some(parent) = root.join(new).parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    if copy { std::fs::copy(root.join(old), root.join(new)).map_err(|e| e.to_string())?; }
    else { std::fs::rename(root.join(old), root.join(new)).map_err(|e| e.to_string())?; }
    if directory { sidecar::move_prefix(root, old, new) } else { sidecar::move_entry(root, old, new, copy) }
}

/// Deletes files and folders. The book forgets what went, even when a later
/// one of them fails.
pub fn remove(root: &Path, rels: &[String]) -> Result<(), String> {
    let mut book = sidecar::load_book(root)?;
    let result = rels.iter().try_for_each(|rel| {
        let path = root.join(rel);
        if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| format!("{rel}: {e}"))?;
        book.sheets.retain(|key, _| key != rel && !key.starts_with(&format!("{rel}/")));
        Ok(())
    });
    sidecar::write_book(root, &book).and(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sidecar::{Pair, Sidecar}, storage::tests::Folder};

    #[test]
    fn nested_paths_scan_rename_and_remove_with_the_book() {
        let dir = Folder::new();
        std::fs::create_dir(dir.0.join("pack")).unwrap();
        std::fs::write(dir.0.join("pack/tree.png"), b"fixture").unwrap();
        let side = Sidecar { tile: Some(Pair::of([8, 16])), ..Default::default() };
        sidecar::store_entry(&dir.0, "pack/tree.png", &side).unwrap();
        let index = crate::index::Index::scan(&dir.0, [16, 16]);
        assert_eq!(index.entries[0].rel, "pack/tree.png");
        assert_eq!(index.entries[0].dir_words, ["pack"]);
        assert_eq!(index.entries[0].side, side);
        relocate(&dir.0, "pack", "plants", false).unwrap();
        relocate(&dir.0, "plants/tree.png", "plants/oak.png", false).unwrap();
        assert_eq!(sidecar::load_book(&dir.0).unwrap().sheets["plants/oak.png"], side);
        remove(&dir.0, &["plants".into()]).unwrap();
        assert!(sidecar::load_book(&dir.0).unwrap().sheets.is_empty());
    }

    #[test]
    fn a_failed_delete_keeps_the_book_in_step() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tree.png"), b"fixture").unwrap();
        sidecar::store_entry(&dir.0, "tree.png", &Sidecar { tile: Some(Pair::of([8, 8])), ..Default::default() }).unwrap();
        assert!(remove(&dir.0, &["tree.png".into(), "gone.png".into()]).is_err());
        assert!(!dir.0.join("tree.png").exists());
        assert!(sidecar::load_book(&dir.0).unwrap().sheets.is_empty());
    }

    #[test]
    fn malformed_book_prevents_file_operations() {
        let dir = Folder::new();
        std::fs::write(dir.0.join("tree.png"), b"image").unwrap();
        std::fs::write(dir.0.join(sidecar::BOOK), b"broken").unwrap();
        assert!(relocate(&dir.0, "tree.png", "oak.png", false).is_err());
        assert!(remove(&dir.0, &["tree.png".into()]).is_err());
        assert!(dir.0.join("tree.png").exists());
        assert!(!dir.0.join("oak.png").exists());
    }

    #[test]
    fn names_cannot_escape_the_folder_on_either_platform() {
        for name in ["../tree", r"..\tree", "C:/tree", "a/../tree"] { assert!(normalize_name(name, Some(".png")).is_none()); }
        assert_eq!(normalize_name("pack/tree", Some(".png")).as_deref(), Some("pack/tree.png"));
    }
}
