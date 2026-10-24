//! Native file dialogs (Win32 common dialogs, spec §7.6).
//!
//! `GetOpenFileNameW`/`GetSaveFileNameW` run a modal message loop until the user answers. Each
//! dialog therefore runs on a short-lived thread of its own (a COM single-threaded apartment), and
//! the answer comes back to the UI thread through `slint::invoke_from_event_loop`. The Slint event
//! loop never stops: the viewport keeps rendering, I/O results and timers keep arriving, and no
//! `RefCell` borrow can be alive across the dialog because `done` never runs synchronously.
//!
//! Owner window behaviour (verified in the T6-spike with the viewport rendering continuously):
//! - The dialog is modal to the main window: it stays above it, it is centred on it, and it takes
//!   the main window's place in Alt+Tab and the taskbar.
//! - While the dialog is open, Windows **disables the owner** (`IsWindowEnabled` is false). Mouse
//!   clicks and keys sent to the main window are refused (a click flashes the dialog instead), so
//!   the user cannot edit, close the window or start another file action from the main window.
//!   The main window still repaints, resizes its content and shows new viewport frames, because
//!   its thread keeps running its own event loop.
//! - When the dialog closes, Windows re-enables the owner and activates it again, before `done`
//!   runs on the UI thread.
//! - The owner and the dialog belong to different threads, so Windows attaches their input
//!   queues while the dialog is open. Both threads keep pumping messages, so neither can hang the
//!   other; a long stall on the UI thread would only delay keyboard focus changes in the dialog.

use std::path::PathBuf;

/// Which file dialog to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// Choose an existing `.labsetup` archive or a project folder's `project.json`.
    Open,
    /// Choose an existing `.step`/`.stp` file to import (POLA-59).
    OpenStep,
    /// Choose one or more `.step`/`.stp` files (POLA-154); answers `ChosenMany`.
    OpenSteps,
    /// Choose a `.labsetup` file to save to; `suggested` pre-fills the name field.
    SaveArchive { suggested: String },
    /// Type the name of a project folder to save to; `suggested` pre-fills the name field.
    SaveFolder { suggested: String },
    /// Choose a `.csv` file to save to (BOM export).
    SaveCsv { suggested: String },
    /// Choose a `.png` file to export to (POLA-66); `suggested` pre-fills the name field.
    SavePng { suggested: String },
}

/// What the user answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogResult {
    /// The chosen path. For `SaveFolder` it is the folder itself (a typed `.labsetup` is removed).
    Chosen(PathBuf),
    /// The chosen files of `OpenSteps` (at least one).
    ChosenMany(Vec<PathBuf>),
    Cancelled,
    /// The dialog could not be shown; the text is a complete user-facing sentence.
    Failed(String),
}

/// Length of the path buffer in UTF-16 units (the Win32 long-path limit).
const PATH_BUFFER_LEN: usize = 32_768;
/// `OpenSteps` buffer: room for many names after the directory.
const MULTI_BUFFER_LEN: usize = 65_536;

/// An `OFN_ALLOWMULTISELECT | OFN_EXPLORER` answer: the directory then NUL-separated names,
/// ended by a double NUL; a single selection is just the full path.
fn parse_multi(buffer: &[u16]) -> Vec<PathBuf> {
    let parts: Vec<String> =
        buffer.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![PathBuf::from(single)],
        [dir, names @ ..] => names.iter().map(|n| PathBuf::from(dir).join(n)).collect(),
    }
}

const OPEN_FILTER: &str = "LabSetup projects (*.labsetup; project.json)\0*.labsetup;project.json\0All files\0*.*\0\0";
const STEP_FILTER: &str = "STEP files (*.step; *.stp)\0*.step;*.stp\0All files\0*.*\0\0";
const ARCHIVE_FILTER: &str = "LabSetup project (*.labsetup)\0*.labsetup\0\0";
const FOLDER_FILTER: &str = "Project folder\0*.*\0\0";
const CSV_FILTER: &str = "CSV file (*.csv)\0*.csv\0\0";
const PNG_FILTER: &str = "PNG image (*.png)\0*.png\0\0";
const ARCHIVE_EXTENSION: &str = "labsetup";

/// Shows a modal file dialog owned by `owner` (an `HWND` as `isize`, because `HWND` is not
/// `Send`; 0 for no owner) on a new thread, then calls `done` with the answer on the UI thread.
///
/// `done` always runs later, from the Slint event loop, never inside this call, so the caller may
/// hold a `RefCell` borrow while calling this. If the event loop has already quit when the dialog
/// closes, `done` is dropped without being called. On other platforms `done(Cancelled)` runs the
/// same way.
pub fn show_file_dialog(
    owner: isize,
    kind: DialogKind,
    initial_dir: Option<PathBuf>,
    done: impl FnOnce(DialogResult) + Send + 'static,
) {
    use std::sync::{Arc, Mutex, PoisonError};

    // `done` lives in a shared slot so it can still be answered if the thread cannot be spawned.
    let slot = Arc::new(Mutex::new(Some(done)));
    let take = |slot: &Mutex<Option<_>>| slot.lock().unwrap_or_else(PoisonError::into_inner).take();

    let thread_slot = slot.clone();
    let spawned = std::thread::Builder::new().name("file-dialog".into()).spawn(move || {
        // The apartment ends only after the answer has been sent: CoUninitialize can wait for the
        // shell's background work, and the UI must not wait for that.
        let _apartment = Apartment::enter();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_dialog(owner, &kind, initial_dir)))
            .unwrap_or_else(|_| DialogResult::Failed("The file dialog stopped unexpectedly.".into()));
        if let Some(done) = take(&thread_slot) {
            deliver(result, done);
        }
    });
    if let Err(error) = spawned
        && let Some(done) = take(&slot)
    {
        deliver(DialogResult::Failed(format!("The file dialog could not be opened: {error}.")), done);
    }
}

/// Like [`show_file_dialog`], for callers whose `done` is not `Send` (for example one holding an
/// `Rc<RefCell<AppState>>`). Must be called on the UI thread; `done` runs there later.
pub fn show_file_dialog_local(
    owner: isize,
    kind: DialogKind,
    initial_dir: Option<PathBuf>,
    done: impl FnOnce(DialogResult) + 'static,
) {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    type Pending = BTreeMap<u64, Box<dyn FnOnce(DialogResult)>>;
    thread_local! {
        static PENDING: RefCell<(u64, Pending)> = RefCell::new((0, BTreeMap::new()));
    }

    let ticket = PENDING.with(|pending| {
        let (next, map) = &mut *pending.borrow_mut();
        *next += 1;
        map.insert(*next, Box::new(done));
        *next
    });
    show_file_dialog(owner, kind, initial_dir, move |result| {
        // Runs on the UI thread (the thread that registered the ticket). The borrow ends before
        // `done` runs, so `done` may open another dialog.
        let done = PENDING.with(|pending| pending.borrow_mut().1.remove(&ticket));
        if let Some(done) = done {
            done(result);
        }
    });
}

/// The main window's `HWND` as `isize` (as in M0's `style_title_bar`), or 0 if it is not
/// available (no native window yet, or not Windows).
pub fn owner_hwnd(window: &slint::Window) -> isize {
    native_hwnd(window)
}

#[cfg(windows)]
fn native_hwnd(window: &slint::Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else { return 0 };
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => win32.hwnd.get(),
        _ => 0,
    }
}

#[cfg(not(windows))]
fn native_hwnd(_window: &slint::Window) -> isize {
    0
}

fn deliver(result: DialogResult, done: impl FnOnce(DialogResult) + Send + 'static) {
    // Fails only when the event loop is gone (the app is quitting); nobody is waiting then.
    slint::invoke_from_event_loop(move || done(result)).ok();
}

/// For `SaveFolder`: the typed name becomes the folder, so a typed `.labsetup` extension is
/// removed ("Bench.labsetup" → "Bench").
fn strip_archive_extension(path: PathBuf) -> PathBuf {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return path };
    let suffix = ARCHIVE_EXTENSION.len() + 1;
    if name.len() > suffix
        && name.is_char_boundary(name.len() - suffix)
        && name[name.len() - suffix..].eq_ignore_ascii_case(".labsetup")
    {
        let stem = name[..name.len() - suffix].to_string();
        path.with_file_name(stem)
    } else {
        path
    }
}

/// A suggested name the dialog accepts: characters Windows forbids in file names are replaced
/// with '-', and trailing dots and spaces are removed. (An invalid initial name would make the
/// dialog fail with FNERR_INVALIDFILENAME instead of opening.)
fn file_name_suggestion(suggested: &str) -> String {
    let cleaned: String = suggested
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '-' } else { c })
        .collect();
    cleaned.trim().trim_end_matches(['.', ' ']).to_string()
}

/// A user-facing sentence for a `CommDlgExtendedError` code (never 0).
fn failure_message(code: u32) -> String {
    match code {
        0x3003 => "The chosen path is too long.".to_string(),            // FNERR_BUFFERTOOSMALL
        0x3002 => "The file name is not valid.".to_string(),             // FNERR_INVALIDFILENAME
        0x3001 => "The file dialog ran out of memory.".to_string(),      // FNERR_SUBCLASSFAILURE
        _ => format!("The file dialog could not be shown (error 0x{code:04X})."),
    }
}

/// The dialog thread's COM single-threaded apartment; balances a successful `CoInitializeEx`
/// (S_OK or S_FALSE) when dropped. If COM cannot be initialised the dialog still works.
#[cfg_attr(not(windows), allow(dead_code))]
struct Apartment(bool);

impl Apartment {
    #[cfg(windows)]
    fn enter() -> Self {
        use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: plain FFI call; the reserved pointer must be null.
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        Apartment(hr >= 0)
    }

    #[cfg(not(windows))]
    fn enter() -> Self {
        Apartment(false)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.0 {
            // SAFETY: paired with the successful CoInitializeEx on this same thread.
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(not(windows))]
fn run_dialog(_owner: isize, _kind: &DialogKind, _initial_dir: Option<PathBuf>) -> DialogResult {
    DialogResult::Cancelled
}

/// Runs one dialog on the current (dialog) thread and returns the answer.
#[cfg(windows)]
fn run_dialog(owner: isize, kind: &DialogKind, initial_dir: Option<PathBuf>) -> DialogResult {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST,
        OFN_ALLOWMULTISELECT, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>(); // filters carry their own NULs
    let wide_z = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();

    let base = OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR;
    let (filter, flags, default_ext, suggested) = match kind {
        DialogKind::Open => (wide(OPEN_FILTER), base | OFN_FILEMUSTEXIST, None, None),
        DialogKind::OpenStep => (wide(STEP_FILTER), base | OFN_FILEMUSTEXIST, None, None),
        DialogKind::OpenSteps => (wide(STEP_FILTER), base | OFN_FILEMUSTEXIST | OFN_ALLOWMULTISELECT, None, None),
        DialogKind::SaveArchive { suggested } => {
            (wide(ARCHIVE_FILTER), base | OFN_OVERWRITEPROMPT, Some(wide_z(ARCHIVE_EXTENSION)), Some(suggested))
        }
        DialogKind::SaveFolder { suggested } => (wide(FOLDER_FILTER), base, None, Some(suggested)),
        DialogKind::SaveCsv { suggested } => {
            (wide(CSV_FILTER), base | OFN_OVERWRITEPROMPT, Some(wide_z("csv")), Some(suggested))
        }
        DialogKind::SavePng { suggested } => (wide(PNG_FILTER), base | OFN_OVERWRITEPROMPT, Some(wide_z("png")), Some(suggested)),
    };
    // Shows which kind of save this is; both save kinds would otherwise be titled "Save As".
    let title = matches!(kind, DialogKind::SaveFolder { .. }).then(|| wide_z("Save as Folder"));

    let buffer_len = if *kind == DialogKind::OpenSteps { MULTI_BUFFER_LEN } else { PATH_BUFFER_LEN };
    let mut buffer = vec![0u16; buffer_len];
    if let Some(suggested) = suggested {
        let name: Vec<u16> = file_name_suggestion(suggested).encode_utf16().take(PATH_BUFFER_LEN - 1).collect();
        buffer[..name.len()].copy_from_slice(&name);
    }
    let initial_dir: Option<Vec<u16>> =
        initial_dir.map(|dir| dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect());

    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner as HWND,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer_len as u32,
        lpstrInitialDir: initial_dir.as_ref().map_or(std::ptr::null(), |dir| dir.as_ptr()),
        lpstrTitle: title.as_ref().map_or(std::ptr::null(), |title| title.as_ptr()),
        Flags: flags,
        lpstrDefExt: default_ext.as_ref().map_or(std::ptr::null(), |ext| ext.as_ptr()),
        ..Default::default()
    };

    // SAFETY: every pointer in `ofn` points into a buffer above that outlives the call, the
    // strings are NUL-terminated (the filter double-NUL-terminated), and `nMaxFile` is the
    // length of `buffer`.
    let chosen = unsafe {
        match kind {
            DialogKind::Open | DialogKind::OpenStep | DialogKind::OpenSteps => GetOpenFileNameW(&mut ofn),
            DialogKind::SaveArchive { .. } | DialogKind::SaveFolder { .. } | DialogKind::SaveCsv { .. } | DialogKind::SavePng { .. } => GetSaveFileNameW(&mut ofn),
        }
    };
    if chosen == 0 {
        // SAFETY: plain FFI call on the thread that ran the dialog.
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 { DialogResult::Cancelled } else { DialogResult::Failed(failure_message(code)) };
    }

    if *kind == DialogKind::OpenSteps {
        let paths = parse_multi(&buffer);
        return if paths.is_empty() { DialogResult::Cancelled } else { DialogResult::ChosenMany(paths) };
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    if len == 0 {
        return DialogResult::Cancelled;
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..len]));
    match kind {
        DialogKind::SaveFolder { .. } => DialogResult::Chosen(strip_archive_extension(path)),
        _ => DialogResult::Chosen(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_double_nul_terminated() {
        for filter in [OPEN_FILTER, STEP_FILTER, ARCHIVE_FILTER, FOLDER_FILTER, PNG_FILTER] {
            // Non-empty (description, pattern) pairs, then the terminating empty string.
            let pairs = filter.strip_suffix("\0\0").unwrap_or_else(|| panic!("{filter:?}"));
            let parts: Vec<&str> = pairs.split('\0').collect();
            assert_eq!(parts.len() % 2, 0, "{filter:?}");
            assert!(parts.iter().all(|part| !part.is_empty()), "{filter:?}");
        }
    }

    #[test]
    fn save_folder_strips_a_typed_archive_extension() {
        let strip = |p: &str| strip_archive_extension(PathBuf::from(p));
        assert_eq!(strip(r"C:\Labs\Bench.labsetup"), PathBuf::from(r"C:\Labs\Bench"));
        assert_eq!(strip(r"C:\Labs\Bench.LabSetup"), PathBuf::from(r"C:\Labs\Bench"));
        assert_eq!(strip(r"C:\Labs\Bench"), PathBuf::from(r"C:\Labs\Bench"));
        assert_eq!(strip(r"C:\Labs\Bench.v2"), PathBuf::from(r"C:\Labs\Bench.v2"));
        assert_eq!(strip(r"C:\Labs\.labsetup"), PathBuf::from(r"C:\Labs\.labsetup"));
        assert_eq!(strip(r"C:\Labs\ห้องแล็บ.labsetup"), PathBuf::from(r"C:\Labs\ห้องแล็บ"));
    }

    #[test]
    fn suggestions_are_valid_file_names() {
        assert_eq!(file_name_suggestion("Demo lab"), "Demo lab");
        assert_eq!(file_name_suggestion("A/B: test?"), "A-B- test-");
        assert_eq!(file_name_suggestion("Bench. "), "Bench");
        assert_eq!(file_name_suggestion("ห้องแล็บ"), "ห้องแล็บ");
    }

    #[test]
    fn multi_select_buffers_parse() {
        let buf = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(parse_multi(&buf("C:\\a\\b.step\0\0")), [PathBuf::from(r"C:\a\b.step")]);
        assert_eq!(
            parse_multi(&buf("C:\\a\0x.step\0y.stp\0\0junk")),
            [PathBuf::from(r"C:\a").join("x.step"), PathBuf::from(r"C:\a").join("y.stp")]
        );
        assert!(parse_multi(&buf("\0\0")).is_empty());
    }

    #[test]
    fn failures_are_sentences() {
        for code in [0x3001, 0x3002, 0x3003, 0xFFFF] {
            let message = failure_message(code);
            assert!(message.starts_with("The ") && message.ends_with('.'), "{message}");
        }
    }
}
