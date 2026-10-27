//! M9 D. The viewport's right-click menu: rows for the clicked node, platform or empty space,
//! each dispatching an action id.

use lab_core::SelectionTarget;
use lab_ui::{AppWindow, PaletteItem};
use lab_viewport::Vec2;
use slint::{ComponentHandle, ModelRc, VecModel};

/// Rows (action id, title) for what was right-clicked.
fn rows(target: Option<SelectionTarget>) -> &'static [(&'static str, &'static str)] {
    match target {
        Some(SelectionTarget::Node(_)) => &[
            ("tool.joint", "Joint"),
            ("tool.move", "Move"),
            ("edit.rotate_90", "Rotate 90°"),
            ("edit.duplicate", "Duplicate"),
            ("library.save_bundle", "Save as Bundle…"),
            ("view.hide", "Hide"),
            ("view.isolate_selection", "Isolate"),
            ("selection.delete", "Delete"),
        ],
        Some(SelectionTarget::Platform(_)) => &[("view.isolate", "Isolate Plate"), ("view.walls", "Hide Room Walls")],
        _ => &[("view.walls", "Hide Room Walls"), ("view.show_all", "Show All")],
    }
}

/// Opens the menu at `pos` (physical px, viewport-relative).
pub fn open(ui: &AppWindow, target: Option<SelectionTarget>, pos: Vec2) {
    let items: Vec<PaletteItem> = rows(target)
        .iter()
        .map(|&(id, title)| PaletteItem { id: id.into(), title: title.into(), group: "".into(), shortcut: "".into() })
        .collect();
    let scale = ui.window().scale_factor();
    ui.set_context_rows(ModelRc::new(VecModel::from(items)));
    ui.set_context_x(pos.x / scale);
    ui.set_context_y(pos.y / scale);
    ui.invoke_open_context_menu();
}
