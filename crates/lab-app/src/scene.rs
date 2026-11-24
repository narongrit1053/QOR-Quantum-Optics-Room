//! Scene-tree handlers: rename, delete, lock and expand from the left sidebar (M2-PLAN POLA-47).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::{Command, GroupId, NodeId, PlatformId, remove_nodes_command, remove_platform_command};
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
        if name.is_empty() {
            return;
        }
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
                3 => doc
                    .group(GroupId(id as u64))
                    .filter(|g| g.name != name)
                    .map(|g| (Command::RenameGroup { id: g.id, name: name.clone() }, "Rename Group")),
                _ => None,
            }
        };
        if let Some((command, label)) = command {
            controller::execute(&ui, &a, &m, "scene", command, label.into(), None, Vec::new());
        }
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_scene_row_action(move |kind, id, action| {
        let Some(ui) = w.upgrade() else { return };
        let command: (Command, String) = match (kind, action.as_str()) {
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
            (3, "save_bundle") => {
                let sel = {
                    let app = a.borrow();
                    let Some(group) = app.session.doc.group(GroupId(id as u64)) else { return };
                    lab_app::multi_select::Sel::from_nodes(&group.members)
                };
                controller::select_sel(&ui, &a, &m, sel);
                controller::run_action(&ui, &a, &m, "library.save_bundle");
                return;
            }
            (3, "ungroup") => {
                let app = a.borrow();
                let Some(group) = app.session.doc.group(GroupId(id as u64)) else { return };
                (Command::RemoveGroup(group.id), format!("Ungroup {}", group.name))
            }
            (3, "delete") => {
                let app = a.borrow();
                let Some(group) = app.session.doc.group(GroupId(id as u64)) else { return };
                (remove_nodes_command(&app.session.doc, group.members.clone()), format!("Delete {}", group.name))
            }
            (0, "delete") => (remove_nodes_command(&a.borrow().session.doc, vec![NodeId(id as u64)]), "Delete".into()),
            (1, "delete") => (remove_platform_command(&a.borrow().session.doc, PlatformId(id as u64)), "Delete Platform".into()),
            (0, "lock") => (Command::SetLocked { id: NodeId(id as u64), locked: true }, "Lock".into()),
            (0, "unlock") => (Command::SetLocked { id: NodeId(id as u64), locked: false }, "Unlock".into()),
            _ => return,
        };
        controller::execute(&ui, &a, &m, "scene", command.0, command.1, None, Vec::new());
    });
}
