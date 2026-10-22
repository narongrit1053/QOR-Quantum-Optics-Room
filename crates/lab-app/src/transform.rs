//! Gizmo drags: each `TransformRequested` becomes a coalesced `SetAttach`, so a whole drag undoes
//! in one step (M2-PLAN POLA-45).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::glam::{DQuat, DVec3};
use lab_core::{Attach, Command, NodeId, Parent};
use lab_ui::AppWindow;

use crate::app_state::AppState;
use crate::controller;
use crate::files;
use crate::ui_models::UiModels;

const GESTURE: &str = "gizmo";

#[allow(clippy::too_many_arguments)]
pub fn on_transform_requested(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    id: NodeId,
    parent: Parent,
    attach: Attach,
    position: DVec3,
    rotation: DQuat,
    done: bool,
) {
    let result = {
        let mut a = app.borrow_mut();
        let session = &mut a.session;
        let Some(node) = session.doc.node(id) else { return };
        // A constrained node keeps its parent and constraints (the viewport's hole snap is ignored)
        // and slides along its free degrees of freedom only.
        let (parent, attach, position, rotation) = match &node.attach {
            Attach::Constrained { constraints } => {
                let world = lab_mates::constraints::world_pose;
                let (Some((sp, sq)), Some((pp, pq))) = (world(&session.doc, parent), world(&session.doc, node.parent))
                else {
                    return;
                };
                let inv = pq.inverse();
                let local = (inv * (sp + sq * position - pp), inv * (sq * rotation));
                let (position, rotation) = lab_mates::constraints::solve(local, constraints);
                (node.parent, node.attach.clone(), position, rotation)
            }
            _ => (parent, attach, position, rotation),
        };
        // SetAttach even when only the placement changes: its inverse restores the attach too,
        // and a drag may snap on and off holes before it ends.
        let command = Command::SetAttach { id, parent, attach, position, rotation };
        let verb = if rotation.angle_between(node.rotation) > 1e-9 { "Rotate" } else { "Move" };
        let label = format!("{verb} {}", node.name);
        let result = session.history.execute_coalescing(&mut session.doc, command, label, GESTURE);
        if done {
            session.history.end_gesture();
        }
        result.map(|changes| (!changes.is_empty()).then(|| (Box::new(session.doc.clone()), changes)))
    };
    match result {
        Ok(Some((snapshot, changes))) => {
            app.borrow().send(lab_viewport::ViewportRequest::ApplyChanges { snapshot, changes });
            controller::refresh(ui, app, models);
            files::arm_autosave(app);
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("gizmo: {error}");
            controller::show_toast(ui, &app.borrow(), &error.to_string(), false);
        }
    }
}
