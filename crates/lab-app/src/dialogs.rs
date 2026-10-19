//! Native file dialogs (M1-SPEC §7.6). Stub until T6b brings the implementation from T6-spike.

#![allow(dead_code)] // Used by the file flows (T6b).

use std::path::PathBuf;

pub enum DialogKind {
    Open,
    SaveArchive { suggested: String },
    SaveFolder { suggested: String },
}

pub enum DialogResult {
    Chosen(PathBuf),
    Cancelled,
    Failed(String),
}

/// Shows a file dialog and reports the result on the UI thread. Stub: always `Cancelled`.
pub fn show_file_dialog(
    _owner: isize,
    _kind: DialogKind,
    _initial_dir: Option<PathBuf>,
    done: impl FnOnce(DialogResult) + Send + 'static,
) {
    slint::invoke_from_event_loop(move || done(DialogResult::Cancelled)).ok();
}

/// The native window handle of `window`, 0 if unavailable. Stub: 0.
pub fn owner_hwnd(_window: &slint::Window) -> isize {
    0
}
