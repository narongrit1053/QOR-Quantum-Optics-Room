//! Gizmo drags: each `TransformRequested` becomes a coalesced `SetAttach`, so a whole drag undoes
//! in one step (M2-PLAN POLA-45).

use std::cell::RefCell;
use std::rc::Rc;

use lab_app::block_move::{BlockMoveError, block_move};
use lab_core::glam::{DQuat, DVec3};
use lab_core::{Attach, ChangeSet, Command, CommandError, Document, NodeId, Parent};
use lab_ui::AppWindow;

use crate::app_state::AppState;
use crate::controller;
use crate::files;
use crate::ui_models::UiModels;

const GESTURE: &str = "gizmo";

enum TransformOutcome {
    Changed(Box<Document>, ChangeSet),
    Unchanged,
    BlockRefused { err: BlockMoveError, snapshot: Box<Document>, show_toast: bool },
    CommandError(CommandError),
}

#[allow(clippy::too_many_arguments)]
fn apply_transform(
    session: &mut lab_app::Session,
    dragging: &mut bool,
    drag_refusal_shown: &mut bool,
    id: NodeId,
    parent: Parent,
    attach: Attach,
    position: DVec3,
    rotation: DQuat,
    done: bool,
) -> Option<TransformOutcome> {
    *dragging = !done;
    let was_refused = *drag_refusal_shown;
    if done {
        *drag_refusal_shown = false;
    }
    let node = session.doc.node(id)?;
    // A constrained node keeps its parent and constraints (the viewport's hole snap is ignored)
    // and slides along its free degrees of freedom only.
    let (parent, attach, position, rotation) = match &node.attach {
        Attach::Constrained { constraints } => {
            let world = lab_mates::constraints::world_pose;
            let (Some((sp, sq)), Some((pp, pq))) = (world(&session.doc, parent), world(&session.doc, node.parent))
            else {
                return None;
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
    let primary_root = session.doc.chain_root(id);
    let others: Vec<NodeId> = lab_app::multi_select::chain_roots(&session.doc, &session.selected_nodes())
        .into_iter()
        .filter(|&root| root != primary_root)
        .collect();
    let command = if others.is_empty() {
        Ok(command)
    } else {
        block_move(&session.doc, id, command, &others)
    };
    Some(match command {
        Err(err) => {
            if done {
                session.history.end_gesture();
            } else {
                *drag_refusal_shown = true;
            }
            TransformOutcome::BlockRefused { err, snapshot: Box::new(session.doc.clone()), show_toast: !was_refused }
        }
        Ok(command) => {
            let result = session.history.execute_coalescing(&mut session.doc, command, label, GESTURE);
            if done {
                session.history.end_gesture();
            }
            match result {
                Ok(changes) if !changes.is_empty() => {
                    TransformOutcome::Changed(Box::new(session.doc.clone()), changes)
                }
                Ok(_) => TransformOutcome::Unchanged,
                Err(error) => TransformOutcome::CommandError(error),
            }
        }
    })
}

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
    let outcome = {
        let mut a = app.borrow_mut();
        let AppState { session, dragging, drag_refusal_shown, .. } = &mut *a;
        let Some(outcome) =
            apply_transform(session, dragging, drag_refusal_shown, id, parent, attach, position, rotation, done)
        else {
            return;
        };
        outcome
    };
    match outcome {
        TransformOutcome::Changed(snapshot, changes) => {
            app.borrow().send(lab_viewport::ViewportRequest::ApplyChanges { snapshot, changes });
            controller::refresh(ui, app, models);
            files::arm_autosave(app);
        }
        TransformOutcome::Unchanged => {}
        TransformOutcome::BlockRefused { err, snapshot, show_toast } => {
            let text = match err {
                BlockMoveError::OffPlate => "Can't move: a part would leave the plate",
                BlockMoveError::Locked => "Part is locked",
                BlockMoveError::MixedParents => "Can't move parts on different platforms together",
            };
            // Send the unchanged snapshot so the viewport snaps the gizmo back.
            app.borrow().send(lab_viewport::ViewportRequest::ApplyChanges {
                snapshot,
                changes: ChangeSet::default(),
            });
            if show_toast {
                controller::show_toast(ui, &app.borrow(), text, false);
            }
        }
        TransformOutcome::CommandError(error) => {
            eprintln!("gizmo: {error}");
            controller::show_toast(ui, &app.borrow(), &error.to_string(), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_app::Session;
    use lab_core::glam::DVec2;
    use lab_core::holes::{HoleGrid, HoleSet, LatticeIndex, compute_holes};
    use lab_core::model::{Finish, Node, Platform, PlatformId, PlatformKind, Shape, Support, Surface};
    use lab_core::outline::Outline;
    use lab_core::SelectionTarget;

    const PLAT: PlatformId = PlatformId(1);

    fn setup_pair_near_edge() -> (Session, HoleSet, LatticeIndex, LatticeIndex) {
        let mut doc = Document::new("t");
        doc.platforms.push(Platform {
            id: PLAT,
            name: "Table".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(HoleGrid::metric()),
            support: Support::Fixed,
            select_lock: false,
        });
        let holes = compute_holes(&doc.platforms[0]).unwrap();
        let h_a = LatticeIndex { i: 10, j: 10 };
        let last_col = holes.first.i + holes.cols as i32 - 1;
        let h_b = LatticeIndex { i: last_col, j: 10 };
        assert!(holes.contains(h_b));

        let make = |id, hole| Node {
            id: NodeId(id),
            name: format!("n{id}"),
            part: "p".into(),
            parent: Parent::Platform(PLAT),
            attach: Attach::Hole { hole },
            position: holes.position(hole).extend(0.0),
            rotation: DQuat::IDENTITY,
            shape: Shape::Box { size: DVec3::ONE },
            finish: Finish::Stainless,
            params: Default::default(),
            pickable: true,
            locked: false,
            select_lock: false,
            note: String::new(),
        };
        doc.nodes.push(make(1, h_a));
        doc.nodes.push(make(2, h_b));
        let mut session = Session::new_clean(doc, None);
        session.selection = Some(SelectionTarget::Node(NodeId(1)));
        session.extra = vec![NodeId(2)];
        (session, holes, h_a, h_b)
    }

    #[test]
    fn refusal_toast_fires_once_per_drag_and_resets_on_done() {
        let (mut session, holes, h_a, _) = setup_pair_near_edge();
        let (mut dragging, mut shown) = (false, false);
        let step_x = |dx: i32| {
            let target = LatticeIndex { i: h_a.i + dx, j: h_a.j };
            (Attach::Hole { hole: target }, holes.position(target).extend(0.0))
        };

        // First refusal in the drag shows the toast.
        let (att, pos) = step_x(1);
        let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, false);
        assert!(matches!(out, Some(TransformOutcome::BlockRefused { err: BlockMoveError::OffPlate, show_toast: true, .. })));

        // Subsequent refusal in the same drag suppresses the toast.
        let (att, pos) = step_x(2);
        let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, false);
        assert!(matches!(out, Some(TransformOutcome::BlockRefused { err: BlockMoveError::OffPlate, show_toast: false, .. })));

        // Releasing while still refused suppresses the toast and resets for the next drag.
        let (att, pos) = step_x(2);
        let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, true);
        assert!(matches!(out, Some(TransformOutcome::BlockRefused { err: BlockMoveError::OffPlate, show_toast: false, .. })));
        assert!(!shown);

        // Next drag shows the toast again on its first refusal.
        let (att, pos) = step_x(1);
        let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, false);
        assert!(matches!(out, Some(TransformOutcome::BlockRefused { err: BlockMoveError::OffPlate, show_toast: true, .. })));
    }

    #[test]
    fn drag_refused_first_then_accepted_undoes_in_one_step() {
        let (mut session, holes, h_a, h_b) = setup_pair_near_edge();
        let start = session.doc.clone();
        let (mut dragging, mut shown) = (false, false);
        let step_x = |dx: i32| {
            let target = LatticeIndex { i: h_a.i + dx, j: h_a.j };
            (Attach::Hole { hole: target }, holes.position(target).extend(0.0))
        };

        // Event 1: refused (off plate).
        let (att, pos) = step_x(1);
        let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, false);
        assert!(matches!(out, Some(TransformOutcome::BlockRefused { err: BlockMoveError::OffPlate, .. })));
        assert!(session.doc.content_eq(&start));

        // Events 2..4: accepted steps left, ending on done = true.
        for (dx, done) in [(-1, false), (-2, false), (-3, true)] {
            let (att, pos) = step_x(dx);
            let out = apply_transform(&mut session, &mut dragging, &mut shown, NodeId(1), Parent::Platform(PLAT), att, pos, DQuat::IDENTITY, done);
            assert!(matches!(out, Some(TransformOutcome::Changed(..))));
        }
        assert_eq!(session.doc.node(NodeId(1)).unwrap().attach, Attach::Hole { hole: LatticeIndex { i: h_a.i - 3, j: h_a.j } });
        assert_eq!(session.doc.node(NodeId(2)).unwrap().attach, Attach::Hole { hole: LatticeIndex { i: h_b.i - 3, j: h_b.j } });

        // Exactly one undo entry restores the pre-drag document.
        session.undo().unwrap().unwrap();
        assert!(!session.history.can_undo());
        assert!(session.doc.content_eq(&start));
    }
}
