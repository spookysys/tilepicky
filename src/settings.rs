// SPDX-License-Identifier: GPL-3.0-or-later
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Settings {
    pub library: Side,
    pub project: Side,
    /// The tool explained itself once already.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub greeted: bool,
}

impl Settings {
    pub fn load() -> Self {
        file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes the file, making its directory when it is not there yet. A
    /// failure is silent: the tool works without remembering.
    pub fn save(&self) {
        let Some(path) = file() else { return };
        let Ok(json) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, json);
    }
}

/// `$XDG_CONFIG_HOME/tilepicky/settings.json`, or the same under `~/.config`.
fn file() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(dir.join("tilepicky").join("settings.json"))
}
