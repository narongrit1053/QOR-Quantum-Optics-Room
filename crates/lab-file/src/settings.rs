//! Per-user settings (display unit, wall display, last folder). Never part of a document.

use std::fs;
use std::path::{Path, PathBuf};

use lab_core::{LengthUnit, ThemeMode, WallDisplay};
use serde::{Deserialize, Serialize};

use crate::FileError;
use crate::atomic::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default)]
    pub display_unit: LengthUnit,
    #[serde(default)]
    pub wall_display: WallDisplay,
    #[serde(default)]
    pub last_dir: Option<PathBuf>,
    #[serde(default)]
    pub theme: ThemeMode,
    /// M6: laser beams drawn in the viewport (`view.beams`).
    #[serde(default = "yes")]
    pub show_beams: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            display_unit: LengthUnit::default(),
            wall_display: WallDisplay::default(),
            last_dir: None,
            theme: ThemeMode::default(),
            show_beams: true,
        }
    }
}

/// `$LABSETUP_SETTINGS`, else `%APPDATA%\LabSetup\settings.json`, else the temp dir.
pub fn settings_path() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(path) = var("LABSETUP_SETTINGS") {
        return path;
    }
    match var("APPDATA") {
        Some(dir) => dir.join("LabSetup").join("settings.json"),
        None => std::env::temp_dir().join("LabSetup").join("settings.json"),
    }
}

/// Missing file: defaults. Unreadable or corrupt file: it is renamed to `<name>.corrupt`
/// (replacing an older one) and defaults are returned.
pub fn load_settings() -> Settings {
    load_settings_at(&settings_path())
}

pub fn load_settings_at(path: &Path) -> Settings {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Settings::default(), // missing or unreadable
    };
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match serde_json::from_slice(bytes) {
        Ok(settings) => settings,
        Err(_) => {
            let mut name = path.as_os_str().to_owned();
            name.push(".corrupt");
            let corrupt = PathBuf::from(name);
            let _ = fs::remove_file(&corrupt);
            let _ = fs::rename(path, &corrupt);
            Settings::default()
        }
    }
}

pub fn save_settings(settings: &Settings) -> Result<(), FileError> {
    save_settings_at(&settings_path(), settings)
}

pub fn save_settings_at(path: &Path, settings: &Settings) -> Result<(), FileError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| FileError::from_io(parent, e))?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|e| FileError::internal(path, e.to_string()))?;
    atomic_write(path, &json)
}
