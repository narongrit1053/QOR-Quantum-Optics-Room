//! Keyboard nudge and rotate of the selected set roots (docs/M7-PLAN.md, task B).

use lab_core::glam::{DQuat, DVec2, DVec3};
use lab_core::holes::compute_holes;
use lab_core::holes::LatticeIndex;
use lab_core::model::{Attach, Document, NodeId, Parent};
use lab_core::Command;

#[derive(Clone, Copy, Debug)]
pub enum Nudge {
    Step { dx: i32, dy: i32 },
    Fine { dx: f64, dy: f64 },
    Turn { degrees: f64 },
}

#[derive(Debug, PartialEq)]
pub enum NudgeError {
    NoHole,
    Locked,
}

/// One SetAttach / SetPlacement per selected root, grouped as one undo step.
pub fn nudge(doc: &Document, selection: &[NodeId], how: Nudge) -> Result<Option<Command>, NudgeError> {
    let mut roots: Vec<NodeId> = selection.iter().map(|&id| lab_app::views::set_root(doc, id)).collect();
    roots.dedup();
    let mut commands = Vec::new();
    for id in roots {
        let Some(node) = doc.node(id) else { continue };
        if node.locked {
            return Err(NudgeError::Locked);
        }
        let (position, rotation) = (node.position, node.rotation);
        let command = match how {
            Nudge::Turn { degrees } => {
                let rotation = DQuat::from_rotation_z(degrees.to_radians()) * rotation;
                Command::SetPlacement { id, position, rotation }
            }
            Nudge::Fine { dx, dy } => Command::SetAttach {
                id,
                parent: node.parent,
                attach: Attach::Free,
                position: position + DVec3::new(dx, dy, 0.0),
                rotation,
            },
            Nudge::Step { dx, dy } => {
                let platform = match node.parent {
                    Parent::Platform(p) => doc.platform(p),
                    _ => None,
                };
                match (&node.attach, platform) {
                    (Attach::Hole { hole }, Some(platform)) => {
                        let holes = compute_holes(platform).map_err(|_| NudgeError::NoHole)?;
                        let target = LatticeIndex { i: hole.i + dx, j: hole.j + dy };
                        if !holes.contains(target) {
                            return Err(NudgeError::NoHole);
                        }
                        let delta = holes.position(target) - holes.position(*hole);
                        Command::SetAttach {
                            id,
                            parent: node.parent,
                            attach: Attach::Hole { hole: target },
                            position: position + delta.extend(0.0),
                            rotation,
                        }
                    }
                    _ => {
                        // Free (or room) roots step by the grid pitch, 25 mm without a grid.
                        let pitch = platform.and_then(|p| p.hole_grid.as_ref()).map_or(DVec2::splat(25.0), |g| g.pitch);
                        let delta = DVec2::new(f64::from(dx) * pitch.x, f64::from(dy) * pitch.y);
                        Command::SetPlacement { id, position: position + delta.extend(0.0), rotation }
                    }
                }
            }
        };
        // A constrained root keeps its constraints: re-solve the requested pose, as gizmo drags do.
        let command = match (&node.attach, command) {
            (
                Attach::Constrained { constraints },
                Command::SetPlacement { position, rotation, .. } | Command::SetAttach { position, rotation, .. },
            ) => {
                let (position, rotation) = lab_mates::constraints::solve((position, rotation), constraints);
                Command::SetAttach { id, parent: node.parent, attach: node.attach.clone(), position, rotation }
            }
            (_, command) => command,
        };
        commands.push(command);
    }
    Ok(match commands.len() {
        0 => None,
        1 => commands.pop(),
        _ => Some(Command::Batch(commands)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::model::{Finish, Node, Platform, PlatformId, PlatformKind, Shape, Support, Surface};
    use lab_core::outline::Outline;

    fn node(id: u64, parent: Parent, attach: Attach) -> Node {
        Node {
            id: NodeId(id),
            name: format!("n{id}"),
            part: "p".into(),
            parent,
            attach,
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            shape: Shape::Box { size: DVec3::ONE },
            finish: Finish::Stainless,
            params: Default::default(),
            pickable: true,
            locked: false,
            note: String::new(),
        }
    }

    #[test]
    fn step_moves_root_one_hole() {
        let mut doc = Document::new("t");
        doc.platforms.push(Platform {
            id: PlatformId(1),
            name: "T".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(lab_core::holes::HoleGrid::metric()),
            support: Support::Fixed,
        });
        let holes = compute_holes(&doc.platforms[0]).unwrap();
        let hole = holes.iter().nth(50).unwrap();
        let mut root = node(1, Parent::Platform(PlatformId(1)), Attach::Hole { hole });
        root.position = holes.position(hole).extend(0.0);
        let mate = Attach::Mate { self_conn: "a".into(), parent_conn: "b".into(), slide: 0.0, twist: 0.0 };
        doc.nodes.push(root);
        doc.nodes.push(node(2, Parent::Node(NodeId(1)), mate));

        // The mated child in the selection moves its root.
        let Ok(Some(Command::SetAttach { id, attach: Attach::Hole { hole: to }, position, .. })) =
            nudge(&doc, &[NodeId(2)], Nudge::Step { dx: 1, dy: 0 })
        else {
            panic!("expected a hole SetAttach")
        };
        assert_eq!(id, NodeId(1));
        assert_eq!(to, LatticeIndex { i: hole.i + 1, j: hole.j });
        assert_eq!(position.truncate(), holes.position(to));
    }

    #[test]
    fn turn_and_fine_keep_constraints() {
        use lab_core::model::{Constraint, ConstraintKind};
        use lab_core::part::Feature;
        let axis = |origin| Feature::Axis { origin, dir: DVec3::Z, radius: 6.35, hole: false };
        let constraints = vec![Constraint {
            kind: ConstraintKind::Concentric,
            child: axis(DVec3::new(3.0, 0.0, 0.0)),
            parent: axis(DVec3::new(10.0, 5.0, 0.0)),
            offset: 0.0,
            angle: 0.0,
            flip: false,
            lock_rotation: false,
        }];
        let mut doc = Document::new("t");
        let mut part = node(1, Parent::Room, Attach::Constrained { constraints: constraints.clone() });
        part.position = DVec3::new(7.0, 5.0, 20.0);
        doc.nodes.push(part);

        let Ok(Some(Command::SetAttach { attach: Attach::Constrained { .. }, position, rotation, .. })) =
            nudge(&doc, &[NodeId(1)], Nudge::Turn { degrees: 15.0 })
        else {
            panic!("expected a constrained SetAttach")
        };
        let turned = (DVec3::new(7.0, 5.0, 20.0), DQuat::from_rotation_z(15f64.to_radians()));
        let (p, q) = lab_mates::constraints::solve(turned, &constraints);
        assert!((position - p).length() < 1e-9 && rotation.angle_between(q) < 1e-9);
        // The child axis (at local x = 3) still sits on the parent axis.
        assert!(((position + rotation * DVec3::new(3.0, 0.0, 0.0)).truncate() - DVec2::new(10.0, 5.0)).length() < 1e-6);

        let fine = nudge(&doc, &[NodeId(1)], Nudge::Fine { dx: 1.0, dy: 0.0 });
        assert!(matches!(fine, Ok(Some(Command::SetAttach { attach: Attach::Constrained { .. }, .. }))));
    }
}
