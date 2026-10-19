//! Crash-safe file replacement: write a temp file next to the target, flush it to disk, then
//! rename it over the target. The target is untouched unless the rename succeeds.
//!
//! Limitations: ACLs and attributes are not carried over to the new file, and there is no
//! directory fsync on Windows.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{FileError, FileErrorKind, is_lock_error};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const CREATE_ATTEMPTS: u32 = 16;
const RENAME_ATTEMPTS: u32 = 10;

/// Writes `bytes` to `target`, replacing it atomically.
pub fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let fail = |kind: FileErrorKind| FileError::new(target, kind);
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if !parent.is_dir() {
        return Err(fail(FileErrorKind::NotFound));
    }
    if target.is_dir() {
        return Err(fail(FileErrorKind::Invalid("is a folder".into())));
    }
    let name = target
        .file_name()
        .ok_or_else(|| fail(FileErrorKind::Invalid("the path has no file name".into())))?
        .to_string_lossy();

    let pid = std::process::id();
    let mut created = None;
    for _ in 0..CREATE_ATTEMPTS {
        let tmp = parent.join(format!(".{name}.{pid}.{}.tmp", COUNTER.fetch_add(1, Ordering::Relaxed)));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => {
                created = Some((tmp, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(FileError::from_io(target, e)),
        }
    }
    let (tmp, mut file) =
        created.ok_or_else(|| fail(FileErrorKind::Internal("could not create a unique temporary file".into())))?;

    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(FileError::from_io(target, e));
    }

    let mut attempt = 1;
    loop {
        match fs::rename(&tmp, target) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < RENAME_ATTEMPTS && (e.kind() == io::ErrorKind::PermissionDenied || is_lock_error(&e)) =>
            {
                std::thread::sleep(Duration::from_millis(25 * u64::from(attempt)));
                attempt += 1;
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(FileError::from_io(target, e));
            }
        }
    }
}
