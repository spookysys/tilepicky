// SPDX-License-Identifier: GPL-3.0-only
//! What the tool remembers between runs: the two folders it worked in, and
//! the tile size each of them used last. The folder's own `tilepicky.json`
//! holds that tile size as well, so a folder keeps it when it travels; this
//! file answers for a folder that is new and has none.

use crate::sidecar::Pair;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What is remembered about one side.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Side {
    /// The folder this side pointed at last. Absent when none was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The tile size a sheet without an entry of its own starts with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<Pair>,
}

/// Which text fields the search matches.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SearchIn {
    pub folders: bool,
    pub files: bool,
    #[serde(default = "enabled")]
    pub captions: bool,
    #[serde(default = "enabled")]
    pub tags: bool,
    /// Semantic search: a sheet matches when its embedding is close to the
    /// query's. It does nothing until embeddings exist for the library.
    #[serde(default = "enabled")]
    pub embeddings: bool,
    #[serde(default)]
    pub text_embeddings: bool,
    #[serde(default = "similarity_floor")]
    pub similarity: f32,
}

fn similarity_floor() -> f32 { crate::embed::FLOOR }

fn enabled() -> bool { true }

impl Default for SearchIn {
    fn default() -> Self {
        SearchIn { folders: true, files: true, captions: true, tags: true, embeddings: true, text_embeddings: false, similarity: similarity_floor() }
    }
}

/// Which cards the library shows. The three boxes name the files of the
/// pack: a tileset, an image that a tileset names, and an image that no
/// tileset names. A card can answer to two of them, and it shows once.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    /// The tileset cards: `.tsx` and `.tsj`.
    #[serde(default = "enabled")]
    pub tilesets: bool,
    /// The images that a tileset names, shown as images.
    #[serde(default)]
    pub in_tilesets: bool,
    /// The images that no tileset names.
    #[serde(default = "enabled")]
    pub loose: bool,
}

impl View {
    /// A view that hides nothing.
    pub const fn everything() -> Self {
        View { tilesets: true, in_tilesets: true, loose: true }
    }

    /// Whether the boxes leave every card in view.
    pub fn all(self) -> bool {
        self == Self::everything()
    }
}

impl Default for View {
    /// A pack is a catalogue of tilesets first. The images that a tileset
    /// names are ingredients, and the person opens them on purpose.
    fn default() -> Self {
        View { tilesets: true, in_tilesets: false, loose: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Settings {
    #[serde(default)]
    pub library: Side,
    #[serde(default)]
    pub project: Side,
    /// The tool explained itself once already.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub greeted: bool,
    /// The legend of keys in the lower left corner is hidden.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_legend: bool,
    /// The AI providers, and the models chosen among them.
    #[serde(default)]
    pub ai: crate::ai::Ai,
    #[serde(default)]
    pub search: SearchIn,
    /// Which cards the library shows.
    #[serde(default)]
    pub view: View,
}

impl Settings {
    /// The settings, and the error when the file could not be read. The
    /// tool then runs on the defaults, and `save` refuses to overwrite the
    /// file until the user says yes; see `rewrite`.
    pub fn load() -> (Self, Option<String>) {
        file().map_or_else(|| (Self::default(), None), |p| Self::load_from(&p))
    }

    fn load_from(path: &std::path::Path) -> (Self, Option<String>) {
        let (mut s, notice) = match crate::storage::read::<Settings>(path) {
            Ok(s) => (s, None),
            Err(e) => (Self::default(), Some(e)),
        };
        s.ai.heal();
        (s, notice)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&file().ok_or("No configuration directory available.")?, false)
    }

    /// Writes the settings over a file that could not be read. The user
    /// agreed to lose what it held.
    pub fn rewrite(&self) -> Result<(), String> {
        self.save_to(&file().ok_or("No configuration directory available.")?, true)
    }

    fn save_to(&self, path: &std::path::Path, overwrite: bool) -> Result<(), String> {
        if !overwrite {
            crate::storage::read::<Self>(path)?;
        }
        crate::storage::write_private(path, self)
    }
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.json"))
}

/// `$XDG_CONFIG_HOME/tilepicky`, or the same under `~/.config`.
#[cfg(not(test))]
pub fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(dir.join("tilepicky"))
}

/// A test build never reaches the person's configuration: a UI test that
/// clicks a setting saves it, with the test's temp folders as the library
/// and the project. It uses `$TILEPICKY_TEST_CONFIG/tilepicky` when the
/// interactive fixture names one, else a folder of its own for the run.
#[cfg(test)]
pub fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("TILEPICKY_TEST_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("tilepicky-test-config-{}", std::process::id())));
    Some(dir.join("tilepicky"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file from before the AI settings gets the shipped providers; a file
    /// that lists none keeps none.
    #[test]
    fn an_old_file_gets_the_ai_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.ai, crate::ai::Ai::default());
        let s: Settings = serde_json::from_str(r#"{"ai": {"providers": []}}"#).unwrap();
        assert!(s.ai.providers.is_empty());
    }

    /// An older file that lists the four text fields gets the embeddings
    /// field, on by default like the others.
    #[test]
    fn an_old_search_section_gets_the_embeddings_field() {
        let s: Settings = serde_json::from_str(r#"{"search": {"folders": true, "files": true, "captions": true, "tags": true}}"#).unwrap();
        assert!(s.search.embeddings);
        assert_eq!(s.search, SearchIn::default());
    }

    /// A file from before the view filters shows tilesets and loose images,
    /// which is the same view the tool offers new.
    #[test]
    fn an_old_file_gets_the_view_filters() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.view, View::default());
        assert!(s.view.tilesets && s.view.loose && !s.view.in_tilesets);
        assert!(!s.view.all());
    }

    #[test]
    fn a_damaged_file_is_kept_until_the_user_says_yes() {
        let dir = crate::storage::tests::Folder::new();
        let path = dir.0.join("settings.json");
        assert!(Settings::load_from(&path).1.is_none());
        std::fs::write(&path, b"{broken").unwrap();
        let (s, error) = Settings::load_from(&path);
        assert!(error.unwrap().contains("settings.json"));
        assert_eq!(s.ai, crate::ai::Ai::default());
        assert!(s.save_to(&path, false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        s.save_to(&path, true).unwrap();
        assert!(Settings::load_from(&path).1.is_none());
    }
}
