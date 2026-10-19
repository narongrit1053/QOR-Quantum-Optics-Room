//! File flows, autosave timer and crash recovery (M1-SPEC §7.6). Stubs until T6b.

use std::cell::RefCell;
use std::rc::Rc;

use lab_app::Session;
use lab_core::sample::demo_document;
use lab_file::{IoResult, Settings};
use lab_ui::AppWindow;

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

/// Handles a `file.*` action (deferred by one event-loop turn). Stub: a toast.
pub fn handle_file_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>, _action: &str) {
    app.borrow_mut().toast_undo = false;
    controller::show_toast(ui, &app.borrow(), "Not available yet", false);
}

/// Handles a button of a file, recovery or close sheet. Stub: closes the sheet.
pub fn handle_sheet_button(ui: &AppWindow, app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>, _button: i32) {
    controller::close_sheet(ui, &mut app.borrow_mut());
}

/// Handles a result from the I/O worker. Stub: ignored.
pub fn handle_io_result(_ui: &AppWindow, _app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>, _result: IoResult) {}

/// Starts the 60 s autosave timer if none is running and the document needs it. Stub: no-op.
pub fn arm_autosave(_app: &Rc<RefCell<AppState>>) {}

/// The window's close request. Stub: closes.
pub fn on_close_requested(
    _ui: &AppWindow,
    _app: &Rc<RefCell<AppState>>,
    _models: &Rc<UiModels>,
) -> slint::CloseRequestResponse {
    slint::CloseRequestResponse::HideWindow
}

/// After the first frame: recovery sheets. Stub: no-op.
pub fn startup(_ui: &AppWindow, _app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>) {}

/// The document to start with, and an error to show once the window appears. Stub: the demo.
pub fn initial_session(_args: &[String], _settings: &Settings) -> (Session, Option<String>) {
    (Session::new_clean(demo_document(), None), None)
}
