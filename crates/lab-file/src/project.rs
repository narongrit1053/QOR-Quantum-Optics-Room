//! Loading and saving projects: a `.labsetup` zip archive (one `project.json`) or a project
//! folder holding `project.json`.

use std::fs;
use std::path::{Path, PathBuf};

use lab_core::{Document, LoadReport};
use serde::{Deserialize, Serialize};

use crate::atomic::atomic_write;
use crate::zip::{self, ZipEntryOut, ZipError};
use crate::{ARCHIVE_MAX_BYTES, FileError, FileErrorKind, PROJECT_JSON_MAX_BYTES};

pub const PROJECT_JSON: &str = "project.json";
pub const PROJECT_EXTENSION: &str = "labsetup";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectFormat {
    Archive,
    Folder,
}

/// Archive: the `.labsetup` file. Folder: the folder holding `project.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectLocation {
    pub path: PathBuf,
    pub format: ProjectFormat,
}

impl ProjectLocation {
    /// The file stem of an archive, or the name of a folder.
    pub fn display_name(&self) -> String {
        let part = match self.format {
            ProjectFormat::Archive => self.path.file_stem(),
            ProjectFormat::Folder => self.path.file_name(),
        };
        part.map_or_else(|| self.path.display().to_string(), |p| p.to_string_lossy().into_owned())
    }

    fn json_path(&self) -> PathBuf {
        match self.format {
            ProjectFormat::Archive => self.path.clone(),
            ProjectFormat::Folder => self.path.join(PROJECT_JSON),
        }
    }
}

#[derive(Debug)]
pub struct LoadedProject {
    pub document: Document,
    pub location: ProjectLocation,
    pub report: LoadReport,
}

#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    /// Before overwriting, keep a copy of the existing file as `<file>.v{n}.bak` (once).
    pub backup_schema: Option<u32>,
}

fn json_of(doc: &Document) -> Result<String, FileErrorKind> {
    let json = doc.to_json().map_err(|e| FileErrorKind::Internal(format!("could not serialize the project: {e}")))?;
    if json.len() as u64 > PROJECT_JSON_MAX_BYTES {
        return Err(FileErrorKind::TooLarge { size: json.len() as u64, limit: PROJECT_JSON_MAX_BYTES });
    }
    Ok(json)
}

/// The project as a zip archive with a single `project.json`. Byte-identical for equal input.
pub fn encode_archive(doc: &Document) -> Result<Vec<u8>, FileErrorKind> {
    let json = json_of(doc)?;
    Ok(zip::write_zip(&[ZipEntryOut { name: PROJECT_JSON, data: json.as_bytes() }]))
}

fn zip_error(error: ZipError, archive: &[u8]) -> FileErrorKind {
    match error {
        ZipError::NotZip => FileErrorKind::NotAProject("it is not a zip archive".into()),
        ZipError::Truncated => FileErrorKind::Corrupt("the archive is cut short or its directory is damaged".into()),
        ZipError::Unsupported(what) => {
            FileErrorKind::Invalid(format!("the archive uses an unsupported feature ({what})"))
        }
        ZipError::BadCrc => FileErrorKind::Corrupt("the archive failed its checksum".into()),
        ZipError::BadDeflate => FileErrorKind::Corrupt("the archive data cannot be decompressed".into()),
        ZipError::TooLarge => FileErrorKind::TooLarge {
            size: zip::declared_size(archive, PROJECT_JSON).unwrap_or(PROJECT_JSON_MAX_BYTES + 1),
            limit: PROJECT_JSON_MAX_BYTES,
        },
        ZipError::Missing => FileErrorKind::NotAProject(format!("the archive has no {PROJECT_JSON}")),
        ZipError::Duplicate => FileErrorKind::Corrupt(format!("the archive has more than one {PROJECT_JSON}")),
    }
}

fn parse_json(bytes: &[u8]) -> Result<(Document, LoadReport), FileErrorKind> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| FileErrorKind::Corrupt("the text is not valid UTF-8".into()))?;
    Document::from_json(text).map_err(FileErrorKind::from_load)
}

/// Reads a project from memory: a zip archive, or plain JSON (with an optional BOM).
pub fn decode_bytes(bytes: &[u8]) -> Result<(Document, LoadReport), FileErrorKind> {
    if bytes.starts_with(b"PK\x03\x04") {
        let json =
            zip::read_entry(bytes, PROJECT_JSON, PROJECT_JSON_MAX_BYTES as usize).map_err(|e| zip_error(e, bytes))?;
        return parse_json(&json);
    }
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if body.trim_ascii_start().first() == Some(&b'{') {
        if bytes.len() as u64 > PROJECT_JSON_MAX_BYTES {
            return Err(FileErrorKind::TooLarge { size: bytes.len() as u64, limit: PROJECT_JSON_MAX_BYTES });
        }
        return parse_json(bytes);
    }
    Err(FileErrorKind::NotAProject("it is neither a LabSetup archive nor project JSON".into()))
}

/// Reads at most `limit` bytes of `path`, checking the size first.
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, FileError> {
    let size = fs::metadata(path).map_err(|e| FileError::from_io(path, e))?.len();
    if size > limit {
        return Err(FileError::new(path, FileErrorKind::TooLarge { size, limit }));
    }
    fs::read(path).map_err(|e| FileError::from_io(path, e))
}

/// Opens a `.labsetup` archive, a project folder, or a folder's `project.json`. Plain JSON in
/// a `.labsetup` is accepted, so the v1 sample files load too.
pub fn load_project(path: &Path) -> Result<LoadedProject, FileError> {
    let meta = fs::metadata(path).map_err(|e| FileError::from_io(path, e))?;
    let is_project_json = path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PROJECT_JSON));
    let location = if meta.is_dir() {
        ProjectLocation { path: path.to_path_buf(), format: ProjectFormat::Folder }
    } else if is_project_json {
        let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        ProjectLocation { path: folder.to_path_buf(), format: ProjectFormat::Folder }
    } else {
        ProjectLocation { path: path.to_path_buf(), format: ProjectFormat::Archive }
    };

    let file = location.json_path();
    let limit = match location.format {
        ProjectFormat::Archive => ARCHIVE_MAX_BYTES,
        ProjectFormat::Folder => PROJECT_JSON_MAX_BYTES,
    };
    let bytes = match read_limited(&file, limit) {
        Err(e) if location.format == ProjectFormat::Folder && e.kind_is_not_found() => {
            return Err(FileError::new(path, FileErrorKind::NotAProject(format!("the folder has no {PROJECT_JSON}"))));
        }
        other => other?,
    };
    let (document, report) = decode_bytes(&bytes).map_err(|kind| FileError::new(path, kind))?;
    Ok(LoadedProject { document, location, report })
}

impl FileError {
    fn kind_is_not_found(&self) -> bool {
        matches!(self.kind, FileErrorKind::NotFound)
    }
}

/// `<file>.v{n}.bak`, next to the file.
fn backup_path(file: &Path, schema: u32) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(format!(".v{schema}.bak"));
    PathBuf::from(name)
}

/// Saves `doc`. The serialized bytes are decoded again and compared with `doc` first, so a
/// file that would not load back identically is never written.
pub fn save_project(doc: &Document, location: &ProjectLocation, options: &SaveOptions) -> Result<(), FileError> {
    let path = location.path.as_path();
    let fail = |kind: FileErrorKind| FileError::new(path, kind);

    let json = json_of(doc).map_err(fail)?;
    let bytes = match location.format {
        ProjectFormat::Archive => zip::write_zip(&[ZipEntryOut { name: PROJECT_JSON, data: json.as_bytes() }]),
        ProjectFormat::Folder => json.into_bytes(),
    };
    match decode_bytes(&bytes) {
        Ok((back, _)) if back.content_eq(doc) && back.next_free_id() == doc.next_free_id() => {}
        Ok(_) => {
            return Err(FileError::internal(path, "the project did not read back identically; nothing was written"));
        }
        Err(kind) => {
            return Err(FileError::internal(
                path,
                format!("the project would not read back ({kind}); nothing was written"),
            ));
        }
    }

    if location.format == ProjectFormat::Folder {
        match fs::read_dir(path) {
            Ok(mut entries) => {
                if entries.next().is_some() && !path.join(PROJECT_JSON).is_file() {
                    return Err(fail(FileErrorKind::FolderNotEmpty));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(path).map_err(|e| FileError::from_io(path, e))?;
            }
            Err(_) if path.is_file() => {
                return Err(fail(FileErrorKind::Invalid("is a file, not a folder".into())));
            }
            Err(e) => return Err(FileError::from_io(path, e)),
        }
    }

    let file = location.json_path();
    if let Some(schema) = options.backup_schema {
        let backup = backup_path(&file, schema);
        if file.is_file() && !backup.exists() {
            fs::copy(&file, &backup).map_err(|e| FileError::from_io(&backup, e))?;
        }
    }
    atomic_write(&file, &bytes)
}
