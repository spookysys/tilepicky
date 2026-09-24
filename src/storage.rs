// SPDX-License-Identifier: GPL-3.0-only
//! JSON reads distinguish a missing file from damaged data. Writes replace complete files.

use serde::{Serialize, de::DeserializeOwned};
use std::{io::Write, path::Path};

pub fn read<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)
        })();
        if result.is_err() { let _ = std::fs::remove_file(&temp); }
        result
    })();
    result.map_err(|e| format!("Cannot save {}: {e}", path.display()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    pub struct Folder(pub PathBuf);
    impl Folder {
        pub fn new() -> Self {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("tilepicky-storage-{}-{stamp}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Folder { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn missing_is_empty_but_invalid_books_block_every_write() {
        let dir = Folder::new();
        assert!(crate::sidecar::load_book(&dir.0).unwrap().sheets.is_empty());
        let path = dir.0.join(crate::sidecar::BOOK);
        std::fs::write(&path, b"{broken").unwrap();
        assert!(crate::sidecar::load_book(&dir.0).is_err());
        assert!(crate::sidecar::store_tile(&dir.0, [16, 16]).is_err());
        assert!(crate::sidecar::store_entry(&dir.0, "a.png", &Default::default()).is_err());
        assert!(crate::sidecar::store_label(&dir.0, "a.png", None).is_err());
        assert!(crate::sidecar::move_entry(&dir.0, "a.png", "b.png", false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    }

    #[test]
    fn writes_replace_complete_files_and_failed_writes_preserve_them() {
        let dir = Folder::new();
        let path = dir.0.join("data.json");
        write(&path, &vec![1, 2]).unwrap();
        write(&path, &vec![3]).unwrap();
        assert_eq!(read::<Vec<u32>>(&path).unwrap(), [3]);
        let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temp, b"already here").unwrap();
        assert!(write(&path, &vec![4]).is_err());
        assert_eq!(read::<Vec<u32>>(&path).unwrap(), [3]);
        assert_eq!(std::fs::read(&temp).unwrap(), b"already here");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
