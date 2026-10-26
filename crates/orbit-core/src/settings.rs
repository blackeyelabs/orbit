//! App-wide settings (not per vault), stored as `settings.json` in Orbit's
//! config folder. Loading never fails: a missing or unparsable file means
//! defaults, and within a readable file each field that is missing or
//! invalid falls back to its default on its own; unknown keys are ignored.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.json";
const MIN_SIZE: u16 = 80;
const MAX_SIZE: u16 = 150;
const SIZE_STEP: u16 = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    OrbitPaper,
    OrbitPaperDark,
    /// Orbit Paper or Orbit Paper Dark, following the desktop.
    MatchSystem,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// The halftone texture on every surface.
    pub paper_grain: bool,
    interface_size_percent: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::default(),
            paper_grain: true,
            interface_size_percent: 100,
        }
    }
}

impl Settings {
    /// Reads `settings.json` from `config_dir`; anything unreadable means defaults.
    pub fn load(config_dir: &Path) -> Settings {
        let mut settings = Settings::default();
        let Some(serde_json::Value::Object(fields)) = fs::read(config_dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        else {
            return settings;
        };
        let field = |name: &str| fields.get(name).cloned();
        if let Some(theme) = field("theme").and_then(|v| serde_json::from_value(v).ok()) {
            settings.theme = theme;
        }
        if let Some(grain) = field("paper_grain").and_then(|v| v.as_bool()) {
            settings.paper_grain = grain;
        }
        if let Some(size) = field("interface_size_percent").and_then(|v| v.as_u64()) {
            settings.set_interface_size_percent(size.min(u64::from(u16::MAX)) as u16);
        }
        settings
    }

    /// Writes `settings.json` atomically (temp file, then rename).
    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(config_dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = config_dir.join(format!("{FILE_NAME}.tmp"));
        fs::write(&tmp, json)?;
        fs::rename(tmp, config_dir.join(FILE_NAME))
    }

    /// How large the interface is drawn, in percent of the default (80–150).
    pub fn interface_size_percent(&self) -> u16 {
        self.interface_size_percent
    }

    pub fn can_grow_interface(&self) -> bool {
        self.interface_size_percent < MAX_SIZE
    }

    pub fn can_shrink_interface(&self) -> bool {
        self.interface_size_percent > MIN_SIZE
    }

    /// One step (10%) larger, up to 150%.
    pub fn grow_interface(&mut self) {
        self.set_interface_size_percent(self.interface_size_percent + SIZE_STEP);
    }

    /// One step (10%) smaller, down to 80%.
    pub fn shrink_interface(&mut self) {
        self.set_interface_size_percent(self.interface_size_percent.saturating_sub(SIZE_STEP));
    }

    /// Clamps to 80–150% and rounds down to a 10% step.
    fn set_interface_size_percent(&mut self, percent: u16) {
        let clamped = percent.clamp(MIN_SIZE, MAX_SIZE);
        self.interface_size_percent = clamped - clamped % SIZE_STEP;
    }
}

/// `$XDG_CONFIG_HOME/orbit`, else `~/.config/orbit`.
pub fn default_config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("orbit"))
}
