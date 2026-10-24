//! PNG export of the current view (M5-PLAN POLA-66): size sheet → save dialog → `ExportImage`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lab_ui::AppWindow;
use lab_viewport::ViewportRequest;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::dialogs::{self, DialogKind, DialogResult};

/// Connects the export sheet's callbacks. Called once at startup.
pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (ui_weak, a) = (ui.as_weak(), app.clone());
    ui.on_export_png(move |width, height| {
        let Some(ui) = ui_weak.upgrade() else { return };
        ui.set_export_open(false);
        choose_file(&ui, &a, width.max(1) as u32, height.max(1) as u32);
    });
    let ui_weak = ui.as_weak();
    ui.on_export_cancel(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_export_open(false);
        }
    });
}

/// Opens the export dialog (size), then the save dialog, then sends `ExportImage`.
pub fn start(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (w, h) = app.borrow().viewport_size;
    ui.set_export_screen_width(w.max(16) as i32);
    ui.set_export_screen_height(h.max(16) as i32);
    ui.set_export_open(true);
}

fn choose_file(ui: &AppWindow, app: &Rc<RefCell<AppState>>, width: u32, height: u32) {
    let (suggested, initial_dir) = {
        let a = app.borrow();
        (format!("{}.png", a.session.title()), crate::files::initial_dir(&a))
    };
    let (ui_weak, a) = (ui.as_weak(), app.clone());
    dialogs::show_file_dialog_local(dialogs::owner_hwnd(ui.window()), DialogKind::SavePng { suggested }, initial_dir, move |result| {
        let Some(ui) = ui_weak.upgrade() else { return };
        match result {
            DialogResult::Chosen(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let (ui_weak, app) = (ui.as_weak(), a.clone());
                let done = move |ok: bool| {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    let text = if ok { format!("Exported {name}") } else { "The image could not be written.".into() };
                    controller::show_toast(&ui, &app.borrow(), &text, false);
                };
                render_to(&a, path, width, height, Box::new(done));
            }
            DialogResult::Cancelled | DialogResult::ChosenMany(_) => {}
            DialogResult::Failed(message) => controller::show_toast(&ui, &a.borrow(), &message, false),
        }
    });
}

/// Sends `ExportImage` and calls `done(ok)` on the UI thread when `ImageExported` for `path`
/// arrives. If the viewport is not `Ready`, calls `done(false)` at once.
pub fn render_to(app: &Rc<RefCell<AppState>>, path: PathBuf, width: u32, height: u32, done: Box<dyn FnOnce(bool)>) {
    let mut a = app.borrow_mut();
    if !a.viewport_ready {
        drop(a);
        done(false);
        return;
    }
    a.pending_exports.push((path.clone(), done));
    a.send(ViewportRequest::ExportImage { path, width, height });
}

/// Routes `ImageExported` to its pending `done`.
pub fn finished(app: &Rc<RefCell<AppState>>, path: &Path, ok: bool) {
    let done = {
        let mut a = app.borrow_mut();
        let Some(i) = a.pending_exports.iter().position(|(p, _)| p == path) else { return };
        a.pending_exports.remove(i).1
    };
    done(ok);
}
