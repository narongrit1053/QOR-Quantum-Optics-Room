//! File flows, autosave timer and crash recovery (M1-SPEC §7.6).
//!
//! Every flow follows the controller's borrow rule: borrow `AppState`, mutate, collect what to
//! do next, drop the borrow, and only then call into Slint, open a dialog or submit a job.
//! Dialogs and I/O jobs answer later from the event loop, never inside the call that started
//! them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime};

use lab_app::Session;
use lab_core::presets::default_document;
use lab_core::sample::demo_document;
use lab_file::{
    IoJob, IoResult, ProjectFormat, ProjectLocation, RecoveryCandidate, SaveOptions, Settings, load_project,
    save_settings,
};
use lab_ui::AppWindow;
use slint::ComponentHandle;

use crate::app_state::{AfterSave, AppState, PendingAction, PendingSave, SheetKind};
use crate::bridge::trace;
use crate::controller;
use crate::dialogs::{self, DialogKind, DialogResult};
use crate::ui_models::UiModels;

/// How long an edited document waits before it is autosaved.
const AUTOSAVE_DELAY: Duration = Duration::from_secs(60);
/// How long quitting waits for the I/O worker to finish its queue.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// What a button of a recovery-flow sheet (`SheetKind::Recovery`) does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryButton {
    Restore,
    Discard,
    Later,
    /// Closes an information sheet shown during recovery, then shows the next candidate.
    Dismiss,
}

/// An autosave job waiting for its result.
struct PendingAutosave {
    /// The session it was taken from (see `Flow::generation`).
    generation: u64,
    /// History state id of the autosaved document.
    state: u64,
    /// A restored crash candidate, discarded once its document is in this run's autosave.
    candidate: Option<RecoveryCandidate>,
}

/// Bookkeeping for jobs and sheets that `AppState` has no fields for.
#[derive(Default)]
struct Flow {
    /// Incremented whenever the document is replaced, so results meant for an earlier
    /// session (whose state ids restart at 0) are not applied to the new one.
    generation: u64,
    autosaves: BTreeMap<u64, PendingAutosave>,
    /// Load jobs: ticket → the chosen path (for the error sheet's name).
    loads: BTreeMap<u64, PathBuf>,
    /// The meaning of each button of the open recovery-flow sheet.
    recovery_buttons: Vec<RecoveryButton>,
}

thread_local! {
    static FLOW: RefCell<Flow> = RefCell::new(Flow::default());
}

fn flow<R>(f: impl FnOnce(&mut Flow) -> R) -> R {
    FLOW.with(|flow| f(&mut flow.borrow_mut()))
}

// ── Entry points ───────────────────────────────────────────────────────────────────────────

/// Handles a `file.*` action, deferred by one event-loop turn so it never runs inside the
/// Slint callback (or key handler) that raised it.
pub fn handle_file_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, action: &str) {
    let (ui_weak, app, models, action) = (ui.as_weak(), app.clone(), models.clone(), action.to_string());
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(ui) = ui_weak.upgrade() {
            run_file_action(&ui, &app, &models, &action);
        }
    });
}

fn run_file_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, action: &str) {
    {
        let a = app.borrow();
        // While a dialog, a save or a load is under way, or a sheet asks a question, file
        // actions are ignored; editing still works.
        if a.busy || a.sheet.is_some() {
            return;
        }
    }
    match action {
        "file.save" => save(ui, app, models, AfterSave::Nothing),
        "file.save_as" => save_as(ui, app, models, ProjectFormat::Archive, AfterSave::Nothing),
        "file.save_folder" => save_as(ui, app, models, ProjectFormat::Folder, AfterSave::Nothing),
        "file.new" => request(ui, app, models, PendingAction::New),
        "file.open" => request(ui, app, models, PendingAction::Open),
        "file.open_demo" => request(ui, app, models, PendingAction::OpenDemo),
        _ => eprintln!("unknown file action {action}"),
    }
}

/// Handles a button of a "Save changes?", "Couldn't save" or recovery sheet.
pub fn handle_sheet_button(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, button: i32) {
    let kind = app.borrow().sheet;
    controller::close_sheet(ui, &mut app.borrow_mut());
    match kind {
        Some(SheetKind::SaveChanges) => {
            let action = app.borrow_mut().pending_action.take();
            if let Some(action) = action {
                match button {
                    0 => {
                        let then = if action == PendingAction::Close { AfterSave::Quit } else { AfterSave::Continue(action) };
                        save(ui, app, models, then);
                    }
                    1 => perform(ui, app, models, action),
                    _ => {} // Cancel
                }
            }
        }
        Some(SheetKind::SaveFailed) => {
            if button == 1 {
                save_as(ui, app, models, ProjectFormat::Archive, AfterSave::Nothing);
            }
        }
        Some(SheetKind::Recovery) => {
            let meaning = flow(|f| std::mem::take(&mut f.recovery_buttons)).get(button as usize).copied();
            match meaning {
                Some(RecoveryButton::Dismiss) | None => {}
                Some(choice) => answer_recovery(ui, app, models, choice),
            }
        }
        Some(SheetKind::Message) | None => {}
    }
    show_next_recovery(ui, app);
}

/// Handles a result from the I/O worker.
pub fn handle_io_result(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, result: IoResult) {
    match result {
        IoResult::Saved { ticket, location, result } => on_saved(ui, app, models, ticket, location, result),
        IoResult::Loaded { ticket, result } => on_loaded(ui, app, models, ticket, result),
        IoResult::Autosaved { ticket, result } => on_autosaved(ui, app, ticket, result),
    }
}

/// Starts the 60 s autosave timer if none is running and the document needs autosaving.
/// No timer runs while the document is clean: a running timer is stopped when the document no
/// longer needs autosaving (for example after undoing back to the saved state).
pub fn arm_autosave(app: &Rc<RefCell<AppState>>) {
    let a = app.borrow();
    if !a.autosave_available || a.io.is_none() || !a.session.needs_autosave() {
        a.timers.autosave.stop();
        return;
    }
    if a.timers.autosave.running() {
        return;
    }
    let weak: Weak<RefCell<AppState>> = Rc::downgrade(app);
    a.timers.autosave.start(slint::TimerMode::SingleShot, AUTOSAVE_DELAY, move || {
        if let Some(app) = weak.upgrade() {
            autosave_now(&app, None);
        }
    });
}

/// The window's close request.
pub fn on_close_requested(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
) -> slint::CloseRequestResponse {
    let _ = models;
    {
        let mut a = app.borrow_mut();
        if let Some(pending) = &mut a.pending_save {
            // Quits once the save succeeds; a failure shows its sheet instead.
            pending.then = AfterSave::Quit;
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        if a.sheet == Some(SheetKind::SaveChanges) {
            return slint::CloseRequestResponse::KeepWindowShown;
        }
    }
    if app.borrow().session.is_dirty() {
        ask_to_save(ui, app, PendingAction::Close);
        return slint::CloseRequestResponse::KeepWindowShown;
    }
    quit(app);
    slint::CloseRequestResponse::HideWindow
}

/// After the first frame: one recovery sheet per crash candidate, newest first.
pub fn startup(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let _ = models;
    exit_on_ui_panic();
    if app.borrow().recovery.is_empty() {
        return;
    }
    // An information sheet shown before (the startup open error) would be replaced by the
    // first recovery sheet; show it again as part of the recovery flow so the next candidate
    // follows it.
    if app.borrow().sheet == Some(SheetKind::Message) {
        let (title, message) = (ui.get_sheet_title(), ui.get_sheet_message());
        show_recovery_sheet(ui, app, &title, &message, &[("OK", RecoveryButton::Dismiss)], 0);
        return;
    }
    show_next_recovery(ui, app);
}

/// Ends the process when the UI thread panics. Without this, the event loop keeps the dead UI
/// thread waiting for the next window message, so the process (and with it the autosave
/// session's lock) can stay alive indefinitely and the autosave is not offered for recovery.
/// Panics on other threads are left alone (the I/O worker catches its own).
fn exit_on_ui_panic() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            default_hook(info);
            if std::thread::current().name() == Some("main") {
                // The last autosave stays on disk and its lock is released with the process:
                // the next start offers it for recovery.
                std::process::exit(101);
            }
        }));
    });
}

/// The document to start with: the project named on the command line (loaded synchronously),
/// else the demo. On a load failure the demo is used and the error is returned for display.
pub fn initial_session(args: &[String], settings: &Settings) -> (Session, Option<String>) {
    let _ = settings;
    let demo = || Session::new_clean(demo_document(), None);
    let Some(path) = args.get(1) else { return (demo(), None) };
    match load_project(Path::new(path)) {
        Ok(project) => (Session::from_loaded(project), None),
        Err(error) => (demo(), Some(error.to_string())),
    }
}

// ── New, Open, Open Demo, Close ────────────────────────────────────────────────────────────

/// Runs `action`, first asking to save when the document has unsaved changes.
fn request(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, action: PendingAction) {
    if app.borrow().session.is_dirty() {
        ask_to_save(ui, app, action);
    } else {
        perform(ui, app, models, action);
    }
}

fn ask_to_save(ui: &AppWindow, app: &Rc<RefCell<AppState>>, action: PendingAction) {
    let mut a = app.borrow_mut();
    a.pending_action = Some(action);
    let title = format!("Save changes to “{}”?", a.session.title());
    controller::show_sheet(
        ui,
        &mut a,
        SheetKind::SaveChanges,
        &title,
        "Your changes will be lost if you don't save them.",
        &["Save", "Don't Save", "Cancel"],
        0,
        1,
    );
}

/// Runs `action` without asking (the document is clean, saved, or its changes were discarded).
fn perform(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, action: PendingAction) {
    match action {
        PendingAction::New => start_over(ui, app, models, Session::new_clean(default_document(), None)),
        PendingAction::OpenDemo => start_over(ui, app, models, Session::new_clean(demo_document(), None)),
        PendingAction::Open => open(ui, app, models),
        PendingAction::Close => {
            quit(app);
            ui.hide().ok();
        }
    }
}

/// Replaces the document with a fresh one; the old document's autosave is no longer wanted.
fn start_over(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, session: Session) {
    replace(ui, app, models, session);
    submit(app, IoJob::ClearAutosave);
}

fn open(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let initial_dir = initial_dir(&app.borrow());
    set_busy(ui, app, models, true, "");
    let (ui_weak, a, m) = (ui.as_weak(), app.clone(), models.clone());
    dialogs::show_file_dialog_local(dialogs::owner_hwnd(ui.window()), DialogKind::Open, initial_dir, move |result| {
        let Some(ui) = ui_weak.upgrade() else { return };
        match result {
            DialogResult::Chosen(path) => {
                let ticket = a.borrow_mut().ticket();
                flow(|f| f.loads.insert(ticket, path.clone()));
                set_busy(&ui, &a, &m, true, "Opening…");
                submit(&a, IoJob::Load { ticket, path });
            }
            DialogResult::Cancelled => set_busy(&ui, &a, &m, false, ""),
            DialogResult::Failed(message) => {
                set_busy(&ui, &a, &m, false, "");
                show_message(&ui, &a, "Couldn't show the Open dialog", &message);
            }
        }
    });
}

fn on_loaded(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    ticket: u64,
    result: Result<lab_file::LoadedProject, lab_file::FileError>,
) {
    let Some(path) = flow(|f| f.loads.remove(&ticket)) else { return };
    set_busy(ui, app, models, false, "");
    match result {
        Ok(project) => {
            let repairs = project.report.repairs.clone();
            remember_dir(app, &project.location);
            start_over(ui, app, models, Session::from_loaded(project));
            if !repairs.is_empty() {
                let list = repairs.iter().map(|r| format!("• {r}")).collect::<Vec<_>>().join("\n");
                show_message(
                    ui,
                    app,
                    "This project was repaired",
                    &format!("Some data could not be used as saved and was corrected:\n\n{list}"),
                );
            }
        }
        Err(error) => {
            // The current document is untouched.
            let title = format!("Couldn't open “{}”", display_name(&path));
            show_message(ui, app, &title, &error.to_string());
        }
    }
}

/// The name a user knows a chosen path by: a folder project's folder name for `project.json`,
/// else the file name.
fn display_name(path: &Path) -> String {
    let is_project_json = path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(lab_file::PROJECT_JSON));
    let named = if is_project_json { path.parent().unwrap_or(path) } else { path };
    named.file_name().map_or_else(|| named.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The quit sequence: end the autosave session (clean exit), wait for the I/O worker, save the
/// settings, release the viewport, stop the event loop.
fn quit(app: &Rc<RefCell<AppState>>) {
    let (io, settings) = {
        let mut a = app.borrow_mut();
        a.timers.autosave.stop();
        a.recovery.clear(); // releases the candidates' locks; their files stay for the next run
        (a.io.take(), a.settings.clone())
    };
    trace("quit: ending the session");
    if let Some(io) = io {
        io.submit(IoJob::EndSession);
        if !io.shutdown(SHUTDOWN_TIMEOUT) {
            eprintln!("the I/O worker did not finish within {SHUTDOWN_TIMEOUT:?}");
        }
    }
    trace("quit: I/O worker finished");
    if let Err(error) = save_settings(&settings) {
        eprintln!("settings could not be saved: {error}");
    }
    app.borrow_mut().requests = None;
    slint::quit_event_loop().ok();
    trace("quit: event loop told to quit");
}

// ── Save, Save As, Save as Folder ──────────────────────────────────────────────────────────

fn save(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, then: AfterSave) {
    let location = app.borrow().session.location.clone();
    match location {
        Some(location) => submit_save(ui, app, models, location, then),
        None => save_as(ui, app, models, ProjectFormat::Archive, then),
    }
}

fn save_as(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, format: ProjectFormat, then: AfterSave) {
    let (suggested, initial_dir) = {
        let a = app.borrow();
        (a.session.title(), initial_dir(&a))
    };
    let kind = match format {
        ProjectFormat::Archive => DialogKind::SaveArchive { suggested },
        ProjectFormat::Folder => DialogKind::SaveFolder { suggested },
    };
    set_busy(ui, app, models, true, "");
    let (ui_weak, a, m) = (ui.as_weak(), app.clone(), models.clone());
    dialogs::show_file_dialog_local(dialogs::owner_hwnd(ui.window()), kind, initial_dir, move |result| {
        let Some(ui) = ui_weak.upgrade() else { return };
        set_busy(&ui, &a, &m, false, "");
        match result {
            DialogResult::Chosen(path) => submit_save(&ui, &a, &m, ProjectLocation { path, format }, then),
            // Cancelling also cancels what the save was for (New, Open, closing the window).
            DialogResult::Cancelled => {}
            DialogResult::Failed(message) => show_message(&ui, &a, "Couldn't show the Save dialog", &message),
        }
    });
}

fn submit_save(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, location: ProjectLocation, then: AfterSave) {
    let job = {
        let mut a = app.borrow_mut();
        let ticket = a.ticket();
        let state = a.session.history.state_id();
        let options = SaveOptions { backup_schema: a.session.migrated_from };
        a.pending_save = Some(PendingSave { ticket, state, location: location.clone(), then });
        IoJob::Save { ticket, doc: Box::new(a.session.doc.clone()), location, options }
    };
    set_busy(ui, app, models, true, "Saving…");
    submit(app, job);
}

fn on_saved(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    ticket: u64,
    location: ProjectLocation,
    result: Result<(), lab_file::FileError>,
) {
    let pending = {
        let mut a = app.borrow_mut();
        match a.pending_save.take_if(|p| p.ticket == ticket) {
            Some(pending) => pending,
            None => return,
        }
    };
    set_busy(ui, app, models, false, "");
    match result {
        Ok(()) => {
            let clean = {
                let mut a = app.borrow_mut();
                a.session.mark_saved(pending.state, location.clone());
                let clean = !a.session.is_dirty();
                if clean {
                    a.session.autosaved_state = None;
                    a.timers.autosave.stop();
                }
                clean
            };
            remember_dir(app, &location);
            if clean {
                submit(app, IoJob::ClearAutosave);
            }
            app.borrow_mut().toast_undo = false;
            controller::show_toast(ui, &app.borrow(), "Saved", false);
            refresh(ui, app, models);
            match pending.then {
                AfterSave::Nothing => arm_autosave(app),
                // Edits made while the save ran are asked about again.
                AfterSave::Continue(action) => request(ui, app, models, action),
                AfterSave::Quit => request(ui, app, models, PendingAction::Close),
            }
        }
        Err(error) => {
            // The document stays dirty; make sure its latest state is in the autosave.
            let available = app.borrow().autosave_available;
            autosave_now(app, None);
            let title = format!("Couldn't save “{}”", app.borrow().session.title());
            let advice = if available {
                "Your changes are kept in the autosave. Try Save As to choose another location."
            } else {
                "Try Save As to choose another location."
            };
            let message = format!("{error}\n\n{advice}");
            controller::show_sheet(ui, &mut app.borrow_mut(), SheetKind::SaveFailed, &title, &message, &["OK", "Save As…"], 0, -1);
        }
    }
}

// ── Autosave ───────────────────────────────────────────────────────────────────────────────

/// Submits an autosave of the current document if it needs one, or unconditionally when
/// `candidate` is a restored crash candidate (discarded once the autosave succeeds).
fn autosave_now(app: &Rc<RefCell<AppState>>, candidate: Option<RecoveryCandidate>) {
    let mut a = app.borrow_mut();
    a.timers.autosave.stop();
    if !a.autosave_available || a.io.is_none() {
        if let Some(candidate) = candidate {
            candidate.release(); // keep its files: this run cannot hold the document safely
        }
        return;
    }
    if candidate.is_none() && !a.session.needs_autosave() {
        return;
    }
    let ticket = a.ticket();
    let state = a.session.history.state_id();
    let generation = flow(|f| f.generation);
    flow(|f| f.autosaves.insert(ticket, PendingAutosave { generation, state, candidate }));
    let job = IoJob::Autosave {
        ticket,
        doc: Box::new(a.session.doc.clone()),
        title: a.session.title(),
        location: a.session.location.clone(),
    };
    if let Some(io) = &a.io {
        io.submit(job);
    }
}

fn on_autosaved(ui: &AppWindow, app: &Rc<RefCell<AppState>>, ticket: u64, result: Result<(), lab_file::FileError>) {
    let Some(pending) = flow(|f| f.autosaves.remove(&ticket)) else { return };
    match result {
        Ok(()) => {
            if pending.generation == flow(|f| f.generation) {
                app.borrow_mut().session.autosaved_state = Some(pending.state);
            }
            if let Some(candidate) = pending.candidate
                && let Err(error) = candidate.discard()
            {
                eprintln!("recovered autosave could not be removed: {error}");
            }
            // Edits made while the autosave ran start the next round.
            arm_autosave(app);
        }
        Err(error) => {
            eprintln!("autosave failed: {error}");
            if let Some(candidate) = pending.candidate {
                candidate.release();
            }
            ui.set_status("Autosave failed".into());
            arm_autosave(app);
        }
    }
}

// ── Crash recovery ─────────────────────────────────────────────────────────────────────────

/// Shows the sheet for the next recovery candidate, if any and if no other sheet is open.
fn show_next_recovery(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (title, message, buttons) = {
        let a = app.borrow();
        if a.sheet.is_some() {
            return;
        }
        let Some(candidate) = a.recovery.first() else { return };
        let info = candidate.info.as_ref();
        let name = info.map_or_else(|| "Untitled".to_string(), |i| i.title.clone());
        let mut message = format!("“{name}” — autosaved {}", clock_time(candidate.autosaved_at));
        if let Some(location) = info.and_then(|i| i.location.as_ref()) {
            message.push_str(&format!(", original file: {}", location.path.display()));
        }
        let buttons: &[(&str, RecoveryButton)] = if candidate.newer_format() {
            message.push_str("\n\nIt was saved by a newer LabSetup. Update LabSetup to restore it.");
            &[("Later", RecoveryButton::Later)]
        } else if a.session.is_dirty() {
            // Restoring would replace unsaved work (an earlier restored document).
            message.push_str("\n\nSave the current document and start LabSetup again to restore this one.");
            &[("Discard", RecoveryButton::Discard), ("Later", RecoveryButton::Later)]
        } else {
            &[("Restore", RecoveryButton::Restore), ("Discard", RecoveryButton::Discard), ("Later", RecoveryButton::Later)]
        };
        ("Recover unsaved changes?".to_string(), message, buttons)
    };
    show_recovery_sheet(ui, app, &title, &message, buttons, 0);
}

fn show_recovery_sheet(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    title: &str,
    message: &str,
    buttons: &[(&str, RecoveryButton)],
    default: i32,
) {
    flow(|f| f.recovery_buttons = buttons.iter().map(|(_, meaning)| *meaning).collect());
    let labels: Vec<&str> = buttons.iter().map(|(label, _)| *label).collect();
    let destructive = buttons.iter().position(|(_, m)| *m == RecoveryButton::Discard).map_or(-1, |i| i as i32);
    controller::show_sheet(ui, &mut app.borrow_mut(), SheetKind::Recovery, title, message, &labels, default, destructive);
}

fn answer_recovery(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, choice: RecoveryButton) {
    let candidate = {
        let mut a = app.borrow_mut();
        if a.recovery.is_empty() {
            return;
        }
        a.recovery.remove(0)
    };
    match choice {
        RecoveryButton::Restore => match candidate.load() {
            Ok(project) => {
                let location = candidate.info.as_ref().and_then(|i| i.location.clone());
                let mut session = Session::from_recovered(project);
                // An untitled document stays untitled (not the autosave file itself).
                session.location = location;
                replace(ui, app, models, session);
                // Move the document into this run's autosave before the candidate is deleted.
                autosave_now(app, Some(candidate));
            }
            Err(error) => {
                candidate.release();
                show_recovery_sheet(
                    ui,
                    app,
                    "Couldn't recover the document",
                    &format!("{error}\n\nThe autosave is kept and will be offered again next time."),
                    &[("OK", RecoveryButton::Dismiss)],
                    0,
                );
            }
        },
        RecoveryButton::Discard => {
            if let Err(error) = candidate.discard() {
                eprintln!("autosave could not be discarded: {error}");
            }
        }
        RecoveryButton::Later | RecoveryButton::Dismiss => candidate.release(),
    }
}

/// "HH:MM" in local time.
fn clock_time(time: SystemTime) -> String {
    let (hour, minute) = local_hour_minute(time);
    format!("{hour:02}:{minute:02}")
}

#[cfg(windows)]
fn local_hour_minute(time: SystemTime) -> (u16, u16) {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeW {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToSystemTime(file_time: *const FileTime, system_time: *mut SystemTimeW) -> i32;
        fn SystemTimeToTzSpecificLocalTime(zone: *const u8, utc: *const SystemTimeW, local: *mut SystemTimeW) -> i32;
    }

    let utc_fallback = || {
        let secs = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        (((secs / 3600) % 24) as u16, ((secs / 60) % 60) as u16)
    };
    // FILETIME: 100 ns intervals since 1601-01-01.
    let Ok(since_1601) = time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d + Duration::from_secs(11_644_473_600))
    else {
        return utc_fallback();
    };
    let ticks = (since_1601.as_nanos() / 100) as u64;
    let file_time = FileTime { low: ticks as u32, high: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SystemTimeW::default(), SystemTimeW::default());
    // SAFETY: plain FFI calls with valid pointers to correctly laid out FILETIME/SYSTEMTIME
    // values; a null zone means the current time zone.
    let ok = unsafe {
        FileTimeToSystemTime(&file_time, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
    };
    if ok { (local.hour, local.minute) } else { utc_fallback() }
}

#[cfg(not(windows))]
fn local_hour_minute(time: SystemTime) -> (u16, u16) {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    (((secs / 3600) % 24) as u16, ((secs / 60) % 60) as u16)
}

// ── Helpers ────────────────────────────────────────────────────────────────────────────────

/// Replaces the document (see `controller::replace_document`) and starts a new generation,
/// so late autosave results of the old document are not applied to the new one.
fn replace(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, session: Session) {
    flow(|f| f.generation += 1);
    controller::replace_document(ui, app, models, session);
}

fn submit(app: &Rc<RefCell<AppState>>, job: IoJob) {
    if let Some(io) = &app.borrow().io {
        io.submit(job);
    }
}

fn set_busy(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, busy: bool, note: &str) {
    {
        let mut a = app.borrow_mut();
        a.busy = busy;
        a.busy_note = note.to_string();
    }
    refresh(ui, app, models);
}

fn refresh(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    app.borrow_mut().update_hole_cache();
    controller::refresh_all(ui, &app.borrow(), models);
}

/// A sheet with a single OK button. During the recovery flow it is part of that flow, so the
/// next candidate follows it.
fn show_message(ui: &AppWindow, app: &Rc<RefCell<AppState>>, title: &str, message: &str) {
    if app.borrow().recovery.is_empty() {
        controller::show_sheet(ui, &mut app.borrow_mut(), SheetKind::Message, title, message, &["OK"], 0, -1);
    } else {
        show_recovery_sheet(ui, app, title, message, &[("OK", RecoveryButton::Dismiss)], 0);
    }
}

/// Where dialogs start: the current project's folder, else the last folder used.
fn initial_dir(app: &AppState) -> Option<PathBuf> {
    app.session
        .location
        .as_ref()
        .and_then(|l| l.path.parent().map(Path::to_path_buf))
        .or_else(|| app.settings.last_dir.clone())
}

/// Remembers the folder containing `location` as `settings.last_dir`.
fn remember_dir(app: &Rc<RefCell<AppState>>, location: &ProjectLocation) {
    let Some(dir) = location.path.parent().map(Path::to_path_buf) else { return };
    let settings = {
        let mut a = app.borrow_mut();
        if a.settings.last_dir.as_ref() == Some(&dir) {
            return;
        }
        a.settings.last_dir = Some(dir);
        a.settings.clone()
    };
    if let Err(error) = save_settings(&settings) {
        eprintln!("settings could not be saved: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        assert_eq!(display_name(Path::new(r"C:\Labs\Bench.labsetup")), "Bench.labsetup");
        assert_eq!(display_name(Path::new(r"C:\Labs\Bench\project.json")), "Bench");
        assert_eq!(display_name(Path::new(r"C:\Labs\Bench\Project.JSON")), "Bench");
    }

    #[test]
    fn clock_times_are_hh_mm() {
        let text = clock_time(SystemTime::now());
        assert_eq!(text.len(), 5, "{text}");
        assert_eq!(&text[2..3], ":");
    }

    #[test]
    fn initial_session_falls_back_to_the_demo() {
        let settings = Settings::default();
        let (session, error) = initial_session(&["labsetup".into()], &settings);
        assert!(error.is_none());
        assert!(!session.is_dirty() && session.location.is_none());

        let missing = std::env::temp_dir().join("labsetup-T6b-missing.labsetup");
        let (session, error) = initial_session(&["labsetup".into(), missing.display().to_string()], &settings);
        assert!(error.is_some_and(|e| e.contains("does not exist")));
        assert!(!session.is_dirty() && session.location.is_none());
    }
}
