//! Per-user settings (display unit, wall display, last folder). Never part of a document.

use std::collections::BTreeSet;
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
    /// M7: projects opened or saved lately, newest first (at most `MAX_RECENT`).
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// M7: open `recent_files[0]` on start (`file.reopen_last`).
    #[serde(default = "yes")]
    pub reopen_last: bool,
    /// POLA-154: list the built-in demo parts in the library (`library.show_builtin`).
    #[serde(default)]
    pub show_builtin_parts: bool,
    /// POLA-155: graphics choices, applied at the next start. Env vars override them.
    #[serde(default)]
    pub render_quality: RenderQuality,
    #[serde(default)]
    pub gpu: GpuChoice,
    #[serde(default)]
    pub msaa: MsaaChoice,
    /// None: the quality's default.
    #[serde(default)]
    pub shadows: Option<bool>,
    /// POLA-162: named part collections, hidden part ids, and the active library ("" = Default).
    #[serde(default)]
    pub libraries: Vec<UserLibrary>,
    #[serde(default)]
    pub hidden_parts: BTreeSet<String>,
    #[serde(default)]
    pub active_library: String,
    /// POLA-165: library menu view, "grid" or "list" ("" reads as grid).
    #[serde(default)]
    pub library_view: String,
}

/// A user part collection: part ids in insertion order, no duplicates.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserLibrary {
    pub name: String,
    pub parts: Vec<String>,
}

/// `Auto` picks Balanced on integrated GPUs, High otherwise.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RenderQuality {
    #[default]
    Auto,
    High,
    Balanced,
    Low,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum GpuChoice {
    /// Prefer a discrete GPU.
    #[default]
    Auto,
    HighPerformance,
    PowerSaving,
    /// Adapter name as wgpu reports it.
    Named(String),
}

/// `Auto`: the quality's default sample count.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MsaaChoice {
    #[default]
    Auto,
    Off,
    X2,
    X4,
    X8,
}

pub const MAX_RECENT: usize = 10;

impl Settings {
    /// Moves `path` to the front of `recent_files`.
    pub fn push_recent(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(MAX_RECENT);
    }
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
            recent_files: Vec::new(),
            reopen_last: true,
            show_builtin_parts: false,
            render_quality: RenderQuality::Auto,
            gpu: GpuChoice::Auto,
            msaa: MsaaChoice::Auto,
            shadows: None,
            libraries: Vec::new(),
            hidden_parts: BTreeSet::new(),
            active_library: String::new(),
            library_view: "grid".into(),
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

#[cfg(test)]
mod recent_tests {
    use super::*;

    #[test]
    fn recent_files_are_newest_first_unique_and_capped() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.push_recent(Path::new(&format!("p{i}")));
        }
        s.push_recent(Path::new("p5"));
        assert_eq!(s.recent_files.len(), MAX_RECENT);
        assert_eq!(s.recent_files[0], Path::new("p5"));
        assert_eq!(s.recent_files.iter().filter(|p| *p == Path::new("p5")).count(), 1);
    }

    #[test]
    fn old_settings_load_graphics_defaults_and_new_ones_round_trip() {
        let old: Settings = serde_json::from_str(r#"{"show_beams":false}"#).unwrap();
        assert_eq!((old.render_quality, old.gpu.clone(), old.msaa, old.shadows), (RenderQuality::Auto, GpuChoice::Auto, MsaaChoice::Auto, None));
        assert!(!old.show_beams);
        let new = Settings { render_quality: RenderQuality::Low, gpu: GpuChoice::Named("Intel(R) UHD".into()), msaa: MsaaChoice::X8, shadows: Some(false), ..old };
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&new).unwrap()).unwrap();
        assert_eq!(back, new);
    }

    #[test]
    fn old_settings_load_and_libraries_round_trip() {
        let old: Settings = serde_json::from_str(r#"{"show_beams":true}"#).unwrap();
        assert!(old.libraries.is_empty() && old.hidden_parts.is_empty() && old.active_library.is_empty());
        let new = Settings {
            libraries: vec![UserLibrary { name: "MOT".into(), parts: vec!["a".into(), "b".into()] }],
            hidden_parts: BTreeSet::from(["thorlabs:KM100".to_string()]),
            active_library: "MOT".into(),
            ..old
        };
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&new).unwrap()).unwrap();
        assert_eq!(back, new);
    }
}
