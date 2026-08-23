//! Scans a directory of sheets and searches their file and folder names.

use crate::sidecar::{self, Sidecar};
use std::path::{Path, PathBuf};

/// The formats the tool reads. It always writes 32 bit RGBA PNG.
pub const IMAGE_EXTS: [&str; 7] = ["png", "gif", "jpg", "jpeg", "webp", "bmp", "tga"];

pub struct Entry {
    pub rel: String,
    /// The words of the path, lower case.
    pub words: Vec<String>,
    /// The book entry: grid, origins, animations.
    pub side: Sidecar,
}

pub struct Index {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Every directory under the root, so that empty folders show too.
    pub dirs: Vec<String>,
    /// The run's default tile size, for entries that name none.
    pub tile: [u32; 2],
}

impl Index {
    /// Lists every PNG and GIF under `root`, sorted by path.
    pub fn scan(root: &Path, default_tile: [u32; 2]) -> Self {
        let mut rels: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        for e in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
            let Some(rel) = e.path().strip_prefix(root).ok().map(|p| p.to_string_lossy().into_owned()) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            if e.file_type().is_dir() {
                dirs.push(rel);
            } else if e.file_type().is_file() {
                let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                if IMAGE_EXTS.contains(&ext.as_str()) {
                    rels.push(rel);
                }
            }
        }
        rels.sort();
        dirs.sort();
        let mut book = sidecar::load_book(root);
        let entries = rels
            .into_iter()
            .map(|rel| {
                let side = book.remove(&rel).unwrap_or_default();
                Entry { words: path_words(&rel), side, rel }
            })
            .collect();
        Self { root: root.to_path_buf(), entries, dirs, tile: default_tile }
    }

    pub fn position(&self, rel: &str) -> Option<usize> {
        self.entries.binary_search_by(|e| e.rel.as_str().cmp(rel)).ok()
    }

    /// True when every query word is the prefix of a word in the file path.
    pub fn entry_matches(e: &Entry, query: &[String]) -> bool {
        matches(query, |q| e.words.iter().any(|w| w.starts_with(q)))
    }

    pub fn visible(&self, query: &[String]) -> Option<Vec<bool>> {
        if query.is_empty() {
            return None;
        }
        Some(self.entries.iter().map(|e| Self::entry_matches(e, query)).collect())
    }
}

/// Lower-case letter runs, at least two letters long, without duplicates.
pub fn words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_alphabetic() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            if cur.chars().count() >= 2 && !out.contains(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

/// Words of every path component; the file extension is dropped.
pub fn path_words(rel: &str) -> Vec<String> {
    let stem = Path::new(rel).with_extension("");
    words(&stem.to_string_lossy())
}

pub fn query_words(q: &str) -> Vec<String> {
    words(q)
}

pub fn matches(query: &[String], has: impl Fn(&str) -> bool) -> bool {
    query.iter().all(|q| has(q))
}
