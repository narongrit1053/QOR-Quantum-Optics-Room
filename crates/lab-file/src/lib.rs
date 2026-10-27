//! Project files for LabSetup: the zip container, atomic writes, project load/save, autosave
//! and recovery, user settings and the I/O worker thread. `lab-core` stays free of disk I/O.

use std::io;
use std::path::PathBuf;

use lab_core::LoadError;

mod atomic;
mod autosave;
mod project;
mod settings;
mod worker;
mod zip;

pub use atomic::atomic_write;
pub use project::is_extra_name;
pub use autosave::{AutosaveSession, RecoveryCandidate, SessionInfo, default_root, find_recoverable};
pub use project::{
    EmbeddedFile, LoadedProject, PROJECT_EXTENSION, PROJECT_JSON, ProjectFormat, ProjectLocation, SaveOptions, decode_bytes,
    encode_archive, load_project, save_project,
};
pub use settings::{Bundle, UserLibrary, GpuChoice, MsaaChoice, RenderQuality, Settings, load_settings, load_settings_at, save_settings, save_settings_at, settings_path};
pub use worker::{IoJob, IoResult, IoWorker};

/// The largest archive `load_project` reads.
pub const ARCHIVE_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// The largest `project.json` (stored or inflated) that is read or written.
pub const PROJECT_JSON_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// A failed file operation: where, and why.
#[derive(Debug, thiserror::Error)]
#[error("{}: {kind}", .path.display())]
pub struct FileError {
    pub path: PathBuf,
    pub kind: FileErrorKind,
}

impl FileError {
    pub fn new(path: impl Into<PathBuf>, kind: FileErrorKind) -> Self {
        Self { path: path.into(), kind }
    }

    pub fn from_io(path: impl Into<PathBuf>, error: io::Error) -> Self {
        Self::new(path, FileErrorKind::from_io(error))
    }

    pub(crate) fn internal(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::new(path, FileErrorKind::Internal(message.into()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileErrorKind {
    #[error("the file or folder does not exist")]
    NotFound,
    #[error("access was denied (the file may be read-only or open in another program)")]
    PermissionDenied,
    #[error("the file is locked by another program; try again or use Save As")]
    Locked,
    #[error("this is not a LabSetup project ({0})")]
    NotAProject(String),
    #[error("the project file is damaged ({0})")]
    Corrupt(String),
    #[error("the file is too large ({size} bytes; the limit is {limit})")]
    TooLarge { size: u64, limit: u64 },
    #[error(
        "it was saved by a newer LabSetup (format {found}); update LabSetup to open it (this version reads format {supported})"
    )]
    NewerSchema { found: u32, supported: u32 },
    #[error("it contains invalid data: {0}")]
    Invalid(String),
    #[error("the folder is not empty and is not a LabSetup project folder")]
    FolderNotEmpty,
    #[error("{0}")]
    Io(std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Windows ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION.
pub(crate) fn is_lock_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33))
}

impl FileErrorKind {
    /// `NotFound` and `PermissionDenied` map to themselves, OS errors 32 and 33 to `Locked`,
    /// everything else to `Io`.
    pub fn from_io(error: io::Error) -> Self {
        if is_lock_error(&error) {
            return FileErrorKind::Locked;
        }
        match error.kind() {
            io::ErrorKind::NotFound => FileErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => FileErrorKind::PermissionDenied,
            _ => FileErrorKind::Io(error),
        }
    }

    pub fn from_load(error: LoadError) -> Self {
        match error {
            LoadError::NotAProject(why) => FileErrorKind::NotAProject(why),
            LoadError::NewerSchema { found, supported } => FileErrorKind::NewerSchema { found, supported },
            LoadError::Json(e) => FileErrorKind::Corrupt(e.to_string()),
            LoadError::Migration { from, detail } => {
                FileErrorKind::Corrupt(format!("could not convert from format {from}: {detail}"))
            }
            LoadError::Invalid(problems) => FileErrorKind::Invalid(problems.join("; ")),
        }
    }
}

/// Milliseconds since the Unix epoch (0 if the clock is before it).
pub(crate) fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
