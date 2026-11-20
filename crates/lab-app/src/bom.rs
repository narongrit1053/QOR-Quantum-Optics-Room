//! Bill of materials export: the save dialog and CSV write. Rows and CSV live in `lab_app::bom_rows`.

use std::cell::RefCell;
use std::rc::Rc;

use lab_app::bom_rows::{rows, to_csv};
use lab_file::atomic_write;
use lab_ui::AppWindow;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::dialogs::{self, DialogKind, DialogResult};

/// Save dialog (`*.csv`, default `<project name> BOM.csv`), atomic write via lab-file's helper, toast.
pub fn export(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (suggested, initial_dir) = {
        let a = app.borrow();
        (format!("{} BOM.csv", a.session.title()), a.settings.last_dir.clone())
    };
    let (ui_weak, app) = (ui.as_weak(), app.clone());
    dialogs::show_file_dialog_local(
        dialogs::owner_hwnd(ui.window()),
        DialogKind::SaveCsv { suggested },
        initial_dir,
        move |result| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let text = match result {
                DialogResult::Chosen(path) => {
                    let csv = {
                        let a = app.borrow();
                        to_csv(&rows(&a.session.doc, &a.catalog))
                    };
                    match atomic_write(&path, csv.as_bytes()) {
                        Ok(()) => format!("Exported {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into())),
                        Err(_) => "The BOM could not be written.".to_string(),
                    }
                }
                DialogResult::Cancelled | DialogResult::ChosenMany(_) => return,
                DialogResult::Failed(message) => message,
            };
            controller::show_toast(&ui, &app.borrow(), &text, false);
        },
    );
}
