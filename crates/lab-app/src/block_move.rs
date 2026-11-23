//! Rigid block moves for multi-selection and group gizmo drags.

use lab_core::glam::DVec3;
use lab_core::holes::{HoleSet, LatticeIndex, compute_holes};
use lab_core::{Attach, Command, Document, Node, NodeId, Parent, PlatformId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockMoveError {
    OffPlate,
    Locked,
    MixedParents,
}

/// Extends `primary_cmd` (a `SetAttach` for `primary`) into a rigid `Command::Batch` covering
/// `primary` and `others`. Rotation drags apply to `primary` only.
pub fn block_move(
    doc: &Document,
    primary: NodeId,
    primary_cmd: Command,
    others: &[NodeId],
) -> Result<Command, BlockMoveError> {
    let Command::SetAttach { parent: new_parent, position: new_pos, rotation: new_rot, .. } = &primary_cmd else {
        return Ok(primary_cmd);
    };
    let Some(primary_node) = doc.node(primary) else {
        return Ok(primary_cmd);
    };
    if primary_node.locked {
        return Err(BlockMoveError::Locked);
    }
    // Rotation drags apply to the primary only (no block rotation in v1).
    if new_rot.angle_between(primary_node.rotation) > 1e-9 {
        return Ok(primary_cmd);
    }

    let mut members: Vec<&Node> = Vec::with_capacity(1 + others.len());
    members.push(primary_node);
    for &id in others {
        if id == primary || members.iter().any(|n| n.id == id) {
            continue;
        }
        if let Some(node) = doc.node(id) {
            members.push(node);
        }
    }
    if members.iter().any(|n| n.locked) {
        return Err(BlockMoveError::Locked);
    }
    let primary_parent = doc.top_parent(primary);
    if members.iter().any(|n| doc.top_parent(n.id) != primary_parent) {
        return Err(BlockMoveError::MixedParents);
    }

    let cur_root_pos = doc.node_pose(primary).map_or(primary_node.position, |(p, _)| p);
    let new_root_pos = match *new_parent {
        Parent::Node(pid) => doc.node_pose(pid).map_or(*new_pos, |(pp, pq)| pp + pq * *new_pos),
        Parent::Platform(_) | Parent::Room => *new_pos,
    };
    let delta = new_root_pos - cur_root_pos;

    if !members.iter().any(|n| matches!(n.attach, Attach::Hole { .. })) {
        let cmds = members
            .iter()
            .map(|n| Command::SetAttach {
                id: n.id,
                parent: n.parent,
                attach: n.attach.clone(),
                position: n.position + local_shift(doc, n, delta),
                rotation: n.rotation,
            })
            .collect();
        return Ok(Command::Batch(cmds));
    }

    let mut hole_sets: Vec<(PlatformId, HoleSet)> = Vec::new();
    let ref_pid = members
        .iter()
        .find_map(|n| match (&n.attach, n.parent) {
            (Attach::Hole { .. }, Parent::Platform(pid)) => Some(pid),
            _ => None,
        })
        .ok_or(BlockMoveError::OffPlate)?;
    let pitch = holes_for(doc, ref_pid, &mut hole_sets)?.pitch;
    let di = to_step(delta.x / pitch.x)?;
    let dj = to_step(delta.y / pitch.y)?;
    let free_shift = DVec3::new(f64::from(di) * pitch.x, f64::from(dj) * pitch.y, 0.0);

    let mut cmds = Vec::with_capacity(members.len());
    for n in &members {
        match n.attach {
            Attach::Hole { hole } => {
                let Parent::Platform(pid) = n.parent else {
                    return Err(BlockMoveError::OffPlate);
                };
                let holes = holes_for(doc, pid, &mut hole_sets)?;
                let (Some(ti), Some(tj)) = (hole.i.checked_add(di), hole.j.checked_add(dj)) else {
                    return Err(BlockMoveError::OffPlate);
                };
                let target = LatticeIndex { i: ti, j: tj };
                if !holes.contains(target) {
                    return Err(BlockMoveError::OffPlate);
                }
                let shift = (holes.position(target) - holes.position(hole)).extend(0.0);
                cmds.push(Command::SetAttach {
                    id: n.id,
                    parent: n.parent,
                    attach: Attach::Hole { hole: target },
                    position: n.position + shift,
                    rotation: n.rotation,
                });
            }
            _ => {
                cmds.push(Command::SetAttach {
                    id: n.id,
                    parent: n.parent,
                    attach: n.attach.clone(),
                    position: n.position + local_shift(doc, n, free_shift),
                    rotation: n.rotation,
                });
            }
        }
    }
    Ok(Command::Batch(cmds))
}

fn to_step(v: f64) -> Result<i32, BlockMoveError> {
    let r = v.round();
    if r.is_finite() && r >= f64::from(i32::MIN) && r <= f64::from(i32::MAX) {
        Ok(r as i32)
    } else {
        Err(BlockMoveError::OffPlate)
    }
}

fn local_shift(doc: &Document, node: &Node, root_delta: DVec3) -> DVec3 {
    match node.parent {
        Parent::Node(pid) => doc.node_pose(pid).map_or(root_delta, |(_, pq)| pq.inverse() * root_delta),
        Parent::Platform(_) | Parent::Room => root_delta,
    }
}

fn holes_for<'a>(
    doc: &Document,
    pid: PlatformId,
    cache: &'a mut Vec<(PlatformId, HoleSet)>,
) -> Result<&'a HoleSet, BlockMoveError> {
    if let Some(pos) = cache.iter().position(|(id, _)| *id == pid) {
        return Ok(&cache[pos].1);
    }
    let platform = doc.platform(pid).ok_or(BlockMoveError::OffPlate)?;
    let set = compute_holes(platform).map_err(|_| BlockMoveError::OffPlate)?;
    if set.is_empty() || set.pitch.x <= 0.0 || set.pitch.y <= 0.0 {
        return Err(BlockMoveError::OffPlate);
    }
    cache.push((pid, set));
    Ok(&cache.last().unwrap().1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::glam::{DQuat, DVec2};
    use lab_core::holes::HoleGrid;
    use lab_core::model::{Finish, Platform, PlatformKind, Shape, Support, Surface};
    use lab_core::outline::Outline;
    use lab_core::History;

    const PLAT: PlatformId = PlatformId(1);

    fn table_doc() -> (Document, HoleSet) {
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
        (doc, holes)
    }

    fn make_node(id: u64, attach: Attach, position: DVec3) -> Node {
        Node {
            id: NodeId(id),
            name: format!("n{id}"),
            part: "p".into(),
            parent: Parent::Platform(PLAT),
            attach,
            position,
            rotation: DQuat::IDENTITY,
            shape: Shape::Box { size: DVec3::ONE },
            finish: Finish::Stainless,
            params: Default::default(),
            pickable: true,
            locked: false,
            select_lock: false,
            note: String::new(),
        }
    }

    fn drag_cmd(node: &Node, attach: Attach, delta: DVec3) -> Command {
        Command::SetAttach {
            id: node.id,
            parent: node.parent,
            attach,
            position: node.position + delta,
            rotation: node.rotation,
        }
    }

    #[test]
    fn block_step_moves_hole_and_free_members_rigidly() {
        let (mut doc, holes) = table_doc();
        let pitch = holes.pitch.x;
        let h_a = LatticeIndex { i: 10, j: 10 };
        let pos_a = holes.position(h_a).extend(0.0);
        let pos_b = pos_a + DVec3::new(5.0, 5.0, 0.0);
        let a = make_node(1, Attach::Hole { hole: h_a }, pos_a);
        let b = make_node(2, Attach::Free, pos_b);
        let primary_cmd = drag_cmd(
            &a,
            Attach::Hole { hole: LatticeIndex { i: 12, j: 10 } },
            DVec3::new(2.0 * pitch, 0.0, 0.0),
        );
        doc.nodes.push(a);
        doc.nodes.push(b);

        let Command::Batch(cmds) = block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]).unwrap() else {
            panic!("expected Batch");
        };
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(1));
                assert_eq!(*attach, Attach::Hole { hole: LatticeIndex { i: 12, j: 10 } });
                assert!((*position - (pos_a + DVec3::new(2.0 * pitch, 0.0, 0.0))).length() < 1e-9);
            }
            other => panic!("expected SetAttach for primary, got {other:?}"),
        }
        match &cmds[1] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(2));
                assert_eq!(*attach, Attach::Free);
                assert!((*position - (pos_b + DVec3::new(2.0 * pitch, 0.0, 0.0))).length() < 1e-9);
            }
            other => panic!("expected SetAttach for other, got {other:?}"),
        }
    }

    #[test]
    fn block_snaps_free_primary_when_any_member_on_hole() {
        let (mut doc, holes) = table_doc();
        let pitch = holes.pitch.x;
        let pos_a = DVec3::new(10.0, 20.0, 0.0);
        let h_b = LatticeIndex { i: 10, j: 10 };
        let pos_b = holes.position(h_b).extend(0.0);
        let a = make_node(1, Attach::Free, pos_a);
        let b = make_node(2, Attach::Hole { hole: h_b }, pos_b);
        let primary_cmd = drag_cmd(&a, Attach::Free, DVec3::new(1.4 * pitch, 0.0, 0.0));
        doc.nodes.push(a);
        doc.nodes.push(b);

        let Command::Batch(cmds) = block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]).unwrap() else {
            panic!("expected Batch");
        };
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(1));
                assert_eq!(*attach, Attach::Free);
                assert!((*position - (pos_a + DVec3::new(pitch, 0.0, 0.0))).length() < 1e-9);
            }
            other => panic!("expected SetAttach for primary, got {other:?}"),
        }
        match &cmds[1] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(2));
                assert_eq!(*attach, Attach::Hole { hole: LatticeIndex { i: 11, j: 10 } });
                assert!((*position - (pos_b + DVec3::new(pitch, 0.0, 0.0))).length() < 1e-9);
            }
            other => panic!("expected SetAttach for other, got {other:?}"),
        }
    }

    #[test]
    fn block_step_off_plate_refuses_all() {
        let (mut doc, holes) = table_doc();
        let pitch = holes.pitch.x;
        let h_a = LatticeIndex { i: 10, j: 10 };
        let last_col = holes.first.i + holes.cols as i32 - 1;
        let h_b = LatticeIndex { i: last_col, j: 10 };
        assert!(holes.contains(h_b));
        let a = make_node(1, Attach::Hole { hole: h_a }, holes.position(h_a).extend(0.0));
        let b = make_node(2, Attach::Hole { hole: h_b }, holes.position(h_b).extend(0.0));
        let primary_cmd = drag_cmd(
            &a,
            Attach::Hole { hole: LatticeIndex { i: 11, j: 10 } },
            DVec3::new(pitch, 0.0, 0.0),
        );
        doc.nodes.push(a);
        doc.nodes.push(b);

        assert_eq!(
            block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]),
            Err(BlockMoveError::OffPlate)
        );
    }

    #[test]
    fn block_locked_member_refuses() {
        let (mut doc, _holes) = table_doc();
        let a = make_node(1, Attach::Free, DVec3::ZERO);
        let mut b = make_node(2, Attach::Free, DVec3::new(50.0, 0.0, 0.0));
        b.locked = true;
        let primary_cmd = drag_cmd(&a, Attach::Free, DVec3::new(25.0, 0.0, 0.0));
        doc.nodes.push(a);
        doc.nodes.push(b);

        assert_eq!(
            block_move(&doc, NodeId(1), primary_cmd.clone(), &[NodeId(2)]),
            Err(BlockMoveError::Locked)
        );

        // Locking the primary itself is also refused.
        doc.nodes[0].locked = true;
        doc.nodes[1].locked = false;
        assert_eq!(
            block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]),
            Err(BlockMoveError::Locked)
        );
    }

    #[test]
    fn block_free_only_moves_exact_delta() {
        let (mut doc, _holes) = table_doc();
        let pos_a = DVec3::new(10.0, 20.0, 0.0);
        let pos_b = DVec3::new(40.0, -15.0, 5.0);
        let a = make_node(1, Attach::Free, pos_a);
        let b = make_node(2, Attach::Free, pos_b);
        let delta = DVec3::new(12.3, -7.8, 0.0);
        let primary_cmd = drag_cmd(&a, Attach::Free, delta);
        doc.nodes.push(a);
        doc.nodes.push(b);

        let Command::Batch(cmds) = block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]).unwrap() else {
            panic!("expected Batch");
        };
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(1));
                assert_eq!(*attach, Attach::Free);
                assert!((*position - (pos_a + delta)).length() < 1e-9);
            }
            other => panic!("expected SetAttach, got {other:?}"),
        }
        match &cmds[1] {
            Command::SetAttach { id, attach, position, .. } => {
                assert_eq!(*id, NodeId(2));
                assert_eq!(*attach, Attach::Free);
                assert!((*position - (pos_b + delta)).length() < 1e-9);
            }
            other => panic!("expected SetAttach, got {other:?}"),
        }
    }

    #[test]
    fn block_rotation_applies_to_primary_only() {
        let (mut doc, _holes) = table_doc();
        let a = make_node(1, Attach::Free, DVec3::ZERO);
        let b = make_node(2, Attach::Free, DVec3::new(50.0, 0.0, 0.0));
        let rot = DQuat::from_rotation_z(30f64.to_radians());
        let primary_cmd = Command::SetAttach {
            id: a.id,
            parent: a.parent,
            attach: Attach::Free,
            position: a.position,
            rotation: rot,
        };
        doc.nodes.push(a);
        doc.nodes.push(b);

        let cmd = block_move(&doc, NodeId(1), primary_cmd.clone(), &[NodeId(2)]).unwrap();
        assert_eq!(cmd, primary_cmd);
    }

    #[test]
    fn block_move_coalesced_drag_undoes_in_one_step() {
        let (mut doc, holes) = table_doc();
        let pitch = holes.pitch.x;
        let h_a = LatticeIndex { i: 10, j: 10 };
        let pos_a = holes.position(h_a).extend(0.0);
        let pos_b = pos_a + DVec3::new(5.0, 5.0, 0.0);
        doc.nodes.push(make_node(1, Attach::Hole { hole: h_a }, pos_a));
        doc.nodes.push(make_node(2, Attach::Free, pos_b));
        let start = doc.clone();

        let mut history = History::default();
        for step in 1..=2 {
            let cur_a = doc.node(NodeId(1)).unwrap();
            let primary_cmd = Command::SetAttach {
                id: NodeId(1),
                parent: cur_a.parent,
                attach: Attach::Hole { hole: LatticeIndex { i: 10 + step, j: 10 } },
                position: pos_a + DVec3::new(f64::from(step) * pitch, 0.0, 0.0),
                rotation: cur_a.rotation,
            };
            let batch = block_move(&doc, NodeId(1), primary_cmd, &[NodeId(2)]).unwrap();
            history.execute_coalescing(&mut doc, batch, "Move n1", "gizmo").unwrap();
        }
        history.end_gesture();

        assert_eq!(doc.node(NodeId(1)).unwrap().attach, Attach::Hole { hole: LatticeIndex { i: 12, j: 10 } });
        assert!((doc.node(NodeId(2)).unwrap().position - (pos_b + DVec3::new(2.0 * pitch, 0.0, 0.0))).length() < 1e-9);

        history.undo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&start));
    }

    #[test]
    fn block_mixed_parents_refuses() {
        let (mut doc, _holes) = table_doc();
        let plat2 = PlatformId(2);
        let mut p2 = doc.platforms[0].clone();
        p2.id = plat2;
        p2.name = "Table 2".into();
        doc.platforms.push(p2);

        let a = make_node(1, Attach::Free, DVec3::ZERO);
        let mut b = make_node(2, Attach::Free, DVec3::new(50.0, 0.0, 0.0));
        b.parent = Parent::Platform(plat2);
        let mut c = make_node(3, Attach::Free, DVec3::new(100.0, 0.0, 0.0));
        c.parent = Parent::Room;
        let primary_cmd = drag_cmd(&a, Attach::Free, DVec3::new(25.0, 0.0, 0.0));
        doc.nodes.extend([a, b, c]);

        assert_eq!(
            block_move(&doc, NodeId(1), primary_cmd.clone(), &[NodeId(2)]),
            Err(BlockMoveError::MixedParents)
        );
        assert_eq!(
            block_move(&doc, NodeId(1), primary_cmd, &[NodeId(3)]),
            Err(BlockMoveError::MixedParents)
        );
    }
}
