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
    let result = if directory { sidecar::move_prefix(root, old, new) } else { sidecar::move_entry(root, old, new, copy) };
    if let Err(error) = result {
        let rollback = if copy {
            std::fs::remove_file(root.join(new))
        } else if root.join(old).try_exists().unwrap_or(true) {
            Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the original path exists; both paths were kept"))
        } else {
            std::fs::rename(root.join(new), root.join(old))
        };
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}. Cannot restore {old}: {rollback}")),
        };
    }
    Ok(())
}

/// Deletes files and folders. The book forgets what went, even when a later
/// one of them fails.
pub fn remove(root: &Path, rels: &[String]) -> Result<(), String> {
    sidecar::update_book(root, |book| {
        // Keep completed deletions even when a later file cannot be removed.
        Ok(rels.iter().try_for_each(|rel| {
            let path = root.join(rel);
            if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| format!("{rel}: {e}"))?;
            book.sheets.retain(|key, _| key != rel && !key.starts_with(&format!("{rel}/")));
            sidecar::forget_quantize(&mut book.quantize, rel);
            Ok(())
        }))
    })?
}

/// Drops the sources in the project's book that name the project itself.
/// Older versions wrote one for every cell drawn in the project: the name
/// of the project sheet, or an empty name for a sheet without one. Only the
/// library is a source, so a name that the project holds and the library
/// does not is one of those. Without a library, or with one out of reach,
/// such as a drive that is not mounted, only the empty name goes.
/// Returns how many entries changed.
pub fn drop_own_sources(project: &Path, library: &Path) -> Result<usize, String> {
    sidecar::update_book(project, |book| {
        let own = |source: &str| {
            source.is_empty()
                || (library.is_dir() && project.join(source).is_file() && !library.join(source).exists())
        };
        let mut changed = 0;
        for side in book.sheets.values_mut() {
            let before = side.provenance.len();
            side.provenance.retain(|p| !own(&p.source));
            changed += usize::from(side.provenance.len() != before);
        }
        if changed > 0 {
            book.sheets.retain(|_, side| !side.is_empty());
        }
        Ok(changed)
    })
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

    /// Older versions named the project sheet itself, or an empty name, as
    /// the source of cells drawn in the project. Those go; a source the
    /// library holds stays, even when the project has a file of that name.
    #[test]
    fn sources_that_name_the_project_are_dropped() {
        use crate::sidecar::Provenance;
        let (project, library) = (Folder::new(), Folder::new());
        for rel in ["mine.png", "both.png"] { std::fs::write(project.0.join(rel), b"image").unwrap(); }
        std::fs::create_dir(library.0.join("pack")).unwrap();
        for rel in ["pack/tree.png", "both.png"] { std::fs::write(library.0.join(rel), b"image").unwrap(); }
        let from = |source: &str| Provenance { source: source.into(), rects: vec![[0, 0, 8, 8]] };
        let side = Sidecar { provenance: ["", "mine.png", "pack/tree.png", "both.png"].map(from).into(), ..Default::default() };
        sidecar::store_entry(&project.0, "mine.png", &side).unwrap();
        let only_own = Sidecar { provenance: vec![from("mine.png")], ..Default::default() };
        sidecar::store_entry(&project.0, "both.png", &only_own).unwrap();
        assert_eq!(drop_own_sources(&project.0, &library.0), Ok(2));
        let book = sidecar::load_book(&project.0).unwrap();
        let sources: Vec<_> = book.sheets["mine.png"].provenance.iter().map(|p| p.source.as_str()).collect();
        assert_eq!(sources, ["pack/tree.png", "both.png"]);
        assert!(!book.sheets.contains_key("both.png"), "an entry with nothing left goes");
        assert_eq!(drop_own_sources(&project.0, &library.0), Ok(0));
        // Without a library, only the empty name is sure to be wrong. A
        // library that is set but out of reach, such as a drive that is not
        // mounted, counts as none.
        for gone in [Path::new(""), &library.0.join("unmounted")] {
            sidecar::store_entry(&project.0, "mine.png", &side).unwrap();
            assert_eq!(drop_own_sources(&project.0, gone), Ok(1));
            assert_eq!(sidecar::load_book(&project.0).unwrap().sheets["mine.png"].provenance.len(), 3);
        }
    }

    fn quant_group(target: sidecar::QuantizeTarget, rel: &str, colors: usize) -> sidecar::QuantizeGroup {
        let mut setting = crate::quant_panel::default_setting();
        setting.snes.as_mut().unwrap().colors = colors;
        sidecar::QuantizeGroup { target, setting, pins: [(rel.into(), vec![true, false])].into() }
    }

    fn create_image_file(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
    }

    #[test]
    fn quantization_file_groups_follow_rename_copy_and_partial_deletion() {
        use sidecar::QuantizeTarget::Files;
        let dir = Folder::new();
        create_image_file(&dir.0, "a.png");
        let group = quant_group(Files(vec!["a.png".into()]), "a.png", 7);
        sidecar::store_quantize(&dir.0, &[group]).unwrap();
        relocate(&dir.0, "a.png", "pack/b.png", false).unwrap();
        let book = sidecar::load_book(&dir.0).unwrap();
        assert_eq!(book.quantize[0].target, Files(vec!["pack/b.png".into()]));
        assert_eq!(book.quantize[0].pins["pack/b.png"], [true, false]);
        assert!(!book.quantize[0].pins.contains_key("a.png"));
        relocate(&dir.0, "pack/b.png", "pack/c.png", true).unwrap();
        let book = sidecar::load_book(&dir.0).unwrap();
        assert_eq!(book.quantize[0].target, Files(vec!["pack/b.png".into(), "pack/c.png".into()]));
        assert_eq!(book.quantize[0].pins["pack/c.png"], [true, false]);
        assert!(remove(&dir.0, &["pack/b.png".into(), "absent.png".into()]).is_err());
        let book = sidecar::load_book(&dir.0).unwrap();
        assert_eq!(book.quantize[0].target, Files(vec!["pack/c.png".into()]));
        assert!(!book.quantize[0].pins.contains_key("pack/b.png"));
        remove(&dir.0, &["pack".into()]).unwrap();
        assert!(sidecar::load_book(&dir.0).unwrap().quantize.is_empty());
    }

    #[test]
    fn quantization_folder_targets_and_pins_follow_folder_renames() {
        use sidecar::QuantizeTarget::{Dir, Files};
        let dir = Folder::new();
        create_image_file(&dir.0, "pack/a.png");
        create_image_file(&dir.0, "pack/sub/b.png");
        let groups = vec![
            quant_group(Dir("pack/sub".into()), "pack/sub/b.png", 7),
            quant_group(Files(vec!["pack/a.png".into()]), "pack/a.png", 3),
        ];
        sidecar::store_quantize(&dir.0, &groups).unwrap();
        relocate(&dir.0, "pack", "renamed", false).unwrap();
        let book = sidecar::load_book(&dir.0).unwrap();
        assert_eq!(book.quantize[0].target, Dir("renamed/sub".into()));
        assert_eq!(book.quantize[0].pins["renamed/sub/b.png"], [true, false]);
        assert_eq!(book.quantize[1].target, Files(vec!["renamed/a.png".into()]));
        remove(&dir.0, &["renamed".into()]).unwrap();
        assert!(sidecar::load_book(&dir.0).unwrap().quantize.is_empty());
    }

    #[test]
    fn a_sheet_keeps_its_source_setting_when_moved_into_another_group() {
        use sidecar::QuantizeTarget::{Dir, Files};
        for copy in [false, true] {
            let dir = Folder::new();
            create_image_file(&dir.0, "source/a.png");
            create_image_file(&dir.0, "destination/b.png");
            let groups = vec![
                quant_group(Dir("source".into()), "source/a.png", 7),
                quant_group(Dir("destination".into()), "destination/b.png", 3),
            ];
            sidecar::store_quantize(&dir.0, &groups).unwrap();
            relocate(&dir.0, "source/a.png", "destination/a.png", copy).unwrap();
            let book = sidecar::load_book(&dir.0).unwrap();
            assert_eq!(book.quantize[0].target, Dir("source".into()));
            assert_eq!(book.quantize[0].pins.contains_key("source/a.png"), copy);
            assert_eq!(book.quantize[1].target, Files(vec!["destination/b.png".into()]));
            assert_eq!(book.quantize[2].target, Files(vec!["destination/a.png".into()]));
            assert_eq!(book.quantize[2].setting, groups[0].setting);
            assert_eq!(book.quantize[2].pins["destination/a.png"], [true, false]);
            assert_eq!(book.quantize[1].pins["destination/b.png"], [true, false]);
        }
    }

    #[test]
    fn moving_a_subfolder_out_of_a_group_keeps_its_setting() {
        use sidecar::QuantizeTarget::{Dir, Files};
        let dir = Folder::new();
        create_image_file(&dir.0, "source/sub/a.png");
        create_image_file(&dir.0, "source/stay.png");
        let group = quant_group(Dir("source".into()), "source/sub/a.png", 7);
        sidecar::store_quantize(&dir.0, &[group]).unwrap();
        relocate(&dir.0, "source/sub", "moved", false).unwrap();
        let book = sidecar::load_book(&dir.0).unwrap();
        assert_eq!(book.quantize[0].target, Dir("source".into()));
        assert!(book.quantize[0].pins.is_empty());
        assert_eq!(book.quantize[1].target, Files(vec!["moved/a.png".into()]));
        assert_eq!(book.quantize[1].pins["moved/a.png"], [true, false]);
    }

    #[test]
    fn names_cannot_escape_the_folder_on_either_platform() {
        for name in ["../tree", r"..\tree", "C:/tree", "a/../tree"] { assert!(normalize_name(name, Some(".png")).is_none()); }
        assert_eq!(normalize_name("pack/tree", Some(".png")).as_deref(), Some("pack/tree.png"));
    }
}
