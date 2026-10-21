//! Command palette (POLA-65): searches `actions::ACTIONS` and runs the chosen id through
//! the normal `on_action` path.

use std::cell::RefCell;
use std::rc::Rc;

use lab_ui::{AppWindow, PaletteItem};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::actions;
use crate::app_state::AppState;
use crate::ui_models::UiModels;

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let items = Rc::new(VecModel::<PaletteItem>::default());
    ui.set_palette_items(ModelRc::from(items.clone()));

    ui.on_palette_query(move |query| {
        let rows = actions::search(&query)
            .into_iter()
            .map(|a| PaletteItem { id: a.id.into(), title: a.title.into(), group: a.group.into(), shortcut: a.shortcut.into() })
            .collect::<Vec<_>>();
        items.set_vec(rows);
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_palette_run(move |id| {
        let Some(ui) = w.upgrade() else { return };
        ui.set_palette_open(false);
        crate::controller::run_action(&ui, &a, &m, &id);
    });
}
