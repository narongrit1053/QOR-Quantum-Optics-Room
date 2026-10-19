//! Autosave and crash recovery.
//!
//! Each running instance owns a session directory under the autosave root, holding an
//! exclusively locked `lock` file, `session.json` and, once something was autosaved,
//! `autosave.labsetup`. The OS drops the lock when the process dies for any reason, so a session
//! directory whose lock can be taken is dead: its autosave is offered for recovery. A clean exit
//! calls [`AutosaveSession::end`] explicitly. There is deliberately no `Drop` impl, so a panic
//! never deletes the files a crash would have left.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lab_core::{Document, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write;
use crate::project::{LoadedProject, ProjectLocation, encode_archive, load_project};
use crate::{FileError, unix_ms};

const LOCK_FILE: &str = "lock";
const SESSION_FILE: &str = "session.json";
const AUTOSAVE_FILE: &str = "autosave.labsetup";
const SESSION_FORMAT: u32 = 1;

/// `$LABSETUP_AUTOSAVE_DIR`, else `%LOCALAPPDATA%\LabSetup\autosave`, else the temp dir.
pub fn default_root() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("LABSETUP_AUTOSAVE_DIR") {
        return dir;
    }
    match var("LOCALAPPDATA") {
        Some(dir) => dir.join("LabSetup").join("autosave"),
        None => std::env::temp_dir().join("LabSetup").join("autosave"),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub format_version: u32,
    pub pid: u32,
    pub started_at_ms: u64,
    pub autosaved_at_ms: Option<u64>,
    pub title: String,
    pub location: Option<ProjectLocation>,
    pub schema_version: u32,
}

/// Takes the exclusive lock without blocking: `Ok(true)` if taken, `Ok(false)` if another
/// handle (this or another process) holds it.
fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}

fn write_info(dir: &Path, info: &SessionInfo) -> Result<(), FileError> {
    let path = dir.join(SESSION_FILE);
    let json = serde_json::to_vec_pretty(info).map_err(|e| FileError::internal(&path, e.to_string()))?;
    atomic_write(&path, &json)
}

/// The live autosave session of this process. Dropping it without calling `end` leaves its
/// files in place, exactly as a crash would.
pub struct AutosaveSession {
    dir: PathBuf,
    lock: File,
    info: SessionInfo,
}

impl AutosaveSession {
    /// Creates `root/s-{unix_ms}-{pid}/`, locks it and writes `session.json`.
    pub fn start(root: &Path, title: &str) -> Result<Self, FileError> {
        fs::create_dir_all(root).map_err(|e| FileError::from_io(root, e))?;
        let pid = std::process::id();
        let mut started = unix_ms();
        let dir = loop {
            let dir = root.join(format!("s-{started}-{pid}"));
            match fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => started += 1,
                Err(e) => return Err(FileError::from_io(dir, e)),
            }
        };
        let lock_path = dir.join(LOCK_FILE);
        let lock = open_lock_file(&lock_path).map_err(|e| FileError::from_io(&lock_path, e))?;
        match try_lock(&lock) {
            Ok(true) => {}
            Ok(false) => {
                return Err(FileError::internal(&lock_path, "the new session is already locked"));
            }
            Err(e) => return Err(FileError::from_io(&lock_path, e)),
        }
        let info = SessionInfo {
            format_version: SESSION_FORMAT,
            pid,
            started_at_ms: started,
            autosaved_at_ms: None,
            title: title.to_string(),
            location: None,
            schema_version: SCHEMA_VERSION,
        };
        write_info(&dir, &info)?;
        Ok(Self { dir, lock, info })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Atomically writes `autosave.labsetup`, then `session.json`.
    pub fn write(&mut self, doc: &Document, title: &str, location: Option<&ProjectLocation>) -> Result<(), FileError> {
        let path = self.dir.join(AUTOSAVE_FILE);
        let bytes = encode_archive(doc).map_err(|kind| FileError::new(&path, kind))?;
        atomic_write(&path, &bytes)?;
        self.info.autosaved_at_ms = Some(unix_ms());
        self.info.title = title.to_string();
        self.info.location = location.cloned();
        self.info.schema_version = doc.schema_version;
        write_info(&self.dir, &self.info)
    }

    /// Deletes `autosave.labsetup` (the document was saved) and updates `session.json`.
    pub fn clear(&mut self) -> Result<(), FileError> {
        let path = self.dir.join(AUTOSAVE_FILE);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::from_io(path, e)),
        }
        self.info.autosaved_at_ms = None;
        write_info(&self.dir, &self.info)
    }

    /// Clean exit: unlocks and removes the session directory.
    pub fn end(self) -> Result<(), FileError> {
        let Self { dir, lock, .. } = self;
        let _ = lock.unlock();
        drop(lock);
        fs::remove_dir_all(&dir).map_err(|e| FileError::from_io(dir, e))
    }
}

/// A dead session with an autosave. It holds the session lock until discarded or released, so
/// no second scan (in this or another process) offers it again.
pub struct RecoveryCandidate {
    pub dir: PathBuf,
    pub info: Option<SessionInfo>,
    pub autosaved_at: SystemTime,
    lock: File,
}

impl RecoveryCandidate {
    /// Loads the autosave. `location` is the session's own project location when it recorded
    /// one; for an untitled document it is the autosave file itself (see `info.location`).
    pub fn load(&self) -> Result<LoadedProject, FileError> {
        let mut loaded = load_project(&self.dir.join(AUTOSAVE_FILE))?;
        if let Some(location) = self.info.as_ref().and_then(|i| i.location.clone()) {
            loaded.location = location;
        }
        Ok(loaded)
    }

    /// Deletes the session directory.
    pub fn discard(self) -> Result<(), FileError> {
        let Self { dir, lock, .. } = self;
        let _ = lock.unlock();
        drop(lock);
        fs::remove_dir_all(&dir).map_err(|e| FileError::from_io(dir, e))
    }

    /// "Later": drops the lock and keeps the files.
    pub fn release(self) {
        let _ = self.lock.unlock();
    }

    /// Whether the autosave was written by a newer LabSetup than this one.
    pub fn newer_format(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.schema_version > SCHEMA_VERSION)
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// Removes leftover `*.tmp` files of a dead session.
fn remove_stray_temp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|e| e == "tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Session directories under `root` whose process is gone and that hold an autosave, newest
/// first. `own` is the caller's live session directory (skipped). Dead sessions without an
/// autosave are deleted.
pub fn find_recoverable(root: &Path, own: Option<&Path>) -> Vec<RecoveryCandidate> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let is_session = entry.file_name().to_string_lossy().starts_with("s-") && dir.is_dir();
        if !is_session || own.is_some_and(|own| same_dir(own, &dir)) {
            continue;
        }
        let Ok(lock) = open_lock_file(&dir.join(LOCK_FILE)) else {
            continue;
        };
        if !matches!(try_lock(&lock), Ok(true)) {
            continue; // alive (or unreadable): not ours to touch
        }

        let autosave = dir.join(AUTOSAVE_FILE);
        let Ok(meta) = fs::metadata(&autosave) else {
            let _ = lock.unlock();
            drop(lock);
            let _ = fs::remove_dir_all(&dir);
            continue;
        };
        remove_stray_temp_files(&dir);
        let info = fs::read(dir.join(SESSION_FILE)).ok().and_then(|b| serde_json::from_slice::<SessionInfo>(&b).ok());
        let autosaved_at = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        found.push(RecoveryCandidate { dir, info, autosaved_at, lock });
    }
    found.sort_by_key(|c| std::cmp::Reverse(c.autosaved_at));
    found
}
