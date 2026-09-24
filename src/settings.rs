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
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchIn {
    pub folders: bool,
    pub files: bool,
    #[serde(default = "enabled")]
    pub captions: bool,
    #[serde(default = "enabled")]
    pub tags: bool,
}

fn enabled() -> bool { true }

impl Default for SearchIn {
    fn default() -> Self {
        SearchIn { folders: true, files: true, captions: true, tags: true }
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
}

impl Settings {
    /// An unreadable file gives the defaults; `save` then refuses to
    /// overwrite it.
    pub fn load() -> Self {
        let mut s: Settings = file().and_then(|p| crate::storage::read(&p).ok()).unwrap_or_default();
        s.ai.heal();
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let path = file().ok_or("No configuration directory available.")?;
        crate::storage::read::<Self>(&path)?;
        crate::storage::write(&path, self)
    }
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.json"))
}

/// `$XDG_CONFIG_HOME/tilepicky`, or the same under `~/.config`.
pub fn dir() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
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
}
