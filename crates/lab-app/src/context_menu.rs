//! M9 D. The viewport's right-click menu: rows for the clicked node, platform or empty space,
//! each dispatching an action id.

use lab_core::SelectionTarget;

use crate::app_state::AppState;
use lab_ui::{AppWindow, PaletteItem};
use lab_viewport::Vec2;
use slint::{ComponentHandle, ModelRc, VecModel};

/// Rows (action id, title) for what was right-clicked.
fn rows(app: &AppState, target: Option<SelectionTarget>) -> Vec<(&'static str, &'static str)> {
    let doc = &app.session.doc;
    let lock = |locked: bool| if locked { ("selection.unlock", "Unlock") } else { ("selection.lock", "Lock") };
    match target {
        Some(SelectionTarget::Node(id)) => {
            let mut rows = vec![
                ("tool.joint", "Joint"),
                ("tool.move", "Move"),
                ("edit.rotate_90", "Rotate 90°"),
                ("edit.duplicate", "Duplicate"),
                ("view.hide", "Hide"),
                ("view.isolate_selection", "Isolate"),
            ];
            if crate::beams::is_laser(app, id) {
                rows.push(("beam.toggle", if crate::beams::is_on(app, id) { "Turn Laser Off" } else { "Turn Laser On" }));
            }
            rows.push(lock(doc.node(id).is_some_and(|n| n.select_lock)));
            rows.push(("library.save_bundle", "Save as Bundle…"));
            rows.push(("selection.delete", "Delete"));
            rows
        }
        Some(SelectionTarget::Platform(id)) => vec![
            ("view.isolate", "Isolate Plate"),
            lock(doc.platform(id).is_some_and(|p| p.select_lock)),
            ("view.walls", "Hide Room Walls"),
        ],
        _ => vec![("view.walls", "Hide Room Walls"), ("view.show_all", "Show All")],
    }
}

/// Opens the menu at `pos` (physical px, viewport-relative).
pub fn open(ui: &AppWindow, app: &AppState, target: Option<SelectionTarget>, pos: Vec2) {
    let items: Vec<PaletteItem> = rows(app, target)
        .into_iter()
        .map(|(id, title)| PaletteItem { id: id.into(), title: title.into(), group: "".into(), shortcut: "".into() })
        .collect();
    let scale = ui.window().scale_factor();
    ui.set_context_rows(ModelRc::new(VecModel::from(items)));
    ui.set_context_x(pos.x / scale);
    ui.set_context_y(pos.y / scale);
    ui.invoke_open_context_menu();
}
