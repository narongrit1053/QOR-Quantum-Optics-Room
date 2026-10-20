//! Scene-tree handlers: rename, delete, lock and expand from the left sidebar (M2-PLAN POLA-47).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::{Command, NodeId, PlatformId};
use lab_ui::AppWindow;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_scene_row_renamed(move |kind, id, text| {
        let Some(ui) = w.upgrade() else { return };
        let name = text.trim().to_string();
        let command = {
            let app = a.borrow();
            let doc = &app.session.doc;
            match kind {
                0 => doc
                    .node(NodeId(id as u64))
                    .filter(|n| n.name != name)
                    .map(|n| (Command::Rename { id: n.id, name: name.clone() }, "Rename")),
                1 => doc.platform(PlatformId(id as u64)).filter(|p| p.name != name).map(|p| {
                    let mut platform = p.clone();
                    platform.name = name.clone();
                    (Command::UpdatePlatform { platform: Box::new(platform) }, "Rename Platform")
                }),
                _ => None,
            }
        };
        if name.is_empty() {
            return;
        }
        if let Some((command, label)) = command {
            controller::execute(&ui, &a, &m, "scene", command, label.into(), None, Vec::new());
        }
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_scene_row_action(move |kind, id, action| {
        let Some(ui) = w.upgrade() else { return };
        let command = match (kind, action.as_str()) {
            (_, "toggle") => {
                let mut app = a.borrow_mut();
                if !app.scene_collapsed.remove(&(kind, id)) {
                    app.scene_collapsed.insert((kind, id));
                }
                controller::refresh_all(&ui, &app, &m);
                return;
            }
            (1, "isolate") => {
                a.borrow().send(lab_viewport::ViewportRequest::Isolate(Some(PlatformId(id as u64))));
                return;
            }
            (0, "delete") => (Command::RemoveNodes(vec![NodeId(id as u64)]), "Delete"),
            (1, "delete") => (Command::RemovePlatform(PlatformId(id as u64)), "Delete Platform"),
            (0, "lock") => (Command::SetLocked { id: NodeId(id as u64), locked: true }, "Lock"),
            (0, "unlock") => (Command::SetLocked { id: NodeId(id as u64), locked: false }, "Unlock"),
            _ => return,
        };
        controller::execute(&ui, &a, &m, "scene", command.0, command.1.into(), None, Vec::new());
    });
}
