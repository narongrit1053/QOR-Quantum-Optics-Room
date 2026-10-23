//! Closed-form joints (docs/M9-PLAN.md C): child pose = P · offset · angle · motion(values) · flip · C⁻¹,
//! with P and C the picked parent and child origin frames. No solver.

use glam::{DMat3, DQuat, DVec3};
use lab_core::part::Frame;
use lab_core::{Attach, Command, Document, Joint, JointKind, NodeId, Parent};

use crate::constraints::world_pose;

/// One degree of freedom, in the parent origin frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dof {
    RotZ,
    SlideZ,
    SlideX,
    SlideY,
    RotX,
    RotY,
}

/// The DOFs of a kind, in `Joint::values` order.
pub fn dofs(kind: JointKind) -> &'static [Dof] {
    use Dof::*;
    match kind {
        JointKind::Rigid => &[],
        JointKind::Revolute => &[RotZ],
        JointKind::Slider => &[SlideZ],
        JointKind::Cylindrical => &[RotZ, SlideZ],
        JointKind::PinSlot => &[RotZ, SlideX],
        JointKind::Planar => &[SlideX, SlideY, RotZ],
        JointKind::Ball => &[RotX, RotY, RotZ],
    }
}

/// Clamps the used values to their limits.
pub fn clamp(j: &mut Joint) {
    for i in 0..dofs(j.kind).len() {
        if let Some(l) = j.limits[i] {
            j.values[i] = j.values[i].clamp(l.min.min(l.max), l.max.max(l.min));
        }
    }
}

/// Child pose relative to its parent; values clamped to limits.
pub fn joint_pose(j: &Joint) -> (DVec3, DQuat) {
    let mut j = j.clone();
    clamp(&mut j);
    let (mut slide, mut rot) = (DVec3::ZERO, DVec3::ZERO);
    for (d, v) in dofs(j.kind).iter().zip(j.values) {
        match d {
            Dof::SlideX => slide.x = v,
            Dof::SlideY => slide.y = v,
            Dof::SlideZ => slide.z = v,
            Dof::RotX => rot.x = v,
            Dof::RotY => rot.y = v,
            Dof::RotZ => rot.z = v,
        }
    }
    let motion = DQuat::from_rotation_x(rot.x) * DQuat::from_rotation_y(rot.y) * DQuat::from_rotation_z(rot.z);
    let flip = if j.flip { DQuat::from_rotation_x(std::f64::consts::PI) } else { DQuat::IDENTITY };
    let (pp, pq) = frame_pose(&j.parent);
    let (cp, cq) = frame_pose(&j.child);
    // Parent origin frame, then offset + angle, then motion, then flip; child frame inverted last.
    let q = pq * DQuat::from_rotation_z(j.angle) * motion * flip * cq.inverse();
    let at = pp + pq * (j.offset + DQuat::from_rotation_z(j.angle) * slide);
    (at - q * cp, q.normalize())
}

/// `SetAttach` jointing `id` to `parent` (seq kept from `j` or the node's current joint, else
/// assigned), posed by `joint_pose`. `None` for unknown ids.
pub fn joint(doc: &Document, id: NodeId, parent: Parent, mut j: Joint) -> Option<Command> {
    let node = doc.node(id)?;
    world_pose(doc, parent)?;
    if j.seq == 0 {
        j.seq = match &node.attach {
            Attach::Joint(old) if old.seq != 0 => old.seq,
            _ => next_seq(doc),
        };
    }
    clamp(&mut j);
    let (position, rotation) = joint_pose(&j);
    Some(Command::SetAttach { id, parent, attach: Attach::Joint(j), position, rotation })
}

/// Rigid joints from `ids[1..]` to `ids[0]` at their current relative poses; one Batch.
/// Ids that are `ids[0]` or its ancestors are skipped. `None` when nothing is left.
pub fn rigid_group(doc: &Document, ids: &[NodeId]) -> Option<Command> {
    let (&first, rest) = ids.split_first()?;
    let (bp, bq) = world_pose(doc, Parent::Node(first))?;
    let inv = bq.inverse();
    let seq = next_seq(doc);
    let commands: Vec<Command> = rest
        .iter()
        .filter(|&&id| id != first && !doc.descendants(id).contains(&first))
        .filter_map(|&id| world_pose(doc, Parent::Node(id)).map(|pose| (id, pose)))
        .enumerate()
        .map(|(i, (id, (p, q)))| {
            let (rp, rq) = (inv * (p - bp), (inv * q).normalize());
            let j = Joint {
                kind: JointKind::Rigid,
                child: Frame { origin: DVec3::ZERO, z: DVec3::Z, x: DVec3::X },
                parent: Frame { origin: rp, z: rq * DVec3::Z, x: rq * DVec3::X },
                offset: DVec3::ZERO,
                angle: 0.0,
                flip: false,
                values: [0.0; 3],
                limits: [None; 3],
                suppressed: false,
                seq: seq + i as u64,
            };
            Command::SetAttach { id, parent: Parent::Node(first), attach: Attach::Joint(j), position: rp, rotation: rq }
        })
        .collect();
    (!commands.is_empty()).then_some(Command::Batch(commands))
}

/// One more than the largest joint `seq` in the document.
pub fn next_seq(doc: &Document) -> u64 {
    doc.nodes
        .iter()
        .filter_map(|n| match &n.attach {
            Attach::Joint(j) => Some(j.seq),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// A frame as a pose: origin and rotation taking X/Z to `x`/`z` (re-orthogonalised).
pub fn frame_pose(f: &Frame) -> (DVec3, DQuat) {
    let z = f.z.normalize_or(DVec3::Z);
    let x = (f.x - z * f.x.dot(z)).normalize_or(z.any_orthonormal_vector());
    (f.origin, DQuat::from_mat3(&DMat3::from_cols(x, z.cross(x), z)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::Limit;
    use std::f64::consts::FRAC_PI_2;

    fn revolute() -> Joint {
        Joint {
            kind: JointKind::Revolute,
            // Child bore at its own (0, 0, -3), axis +Z; parent post top at (10, 5, 20), axis +Z.
            child: Frame { origin: DVec3::new(0.0, 0.0, -3.0), z: DVec3::Z, x: DVec3::X },
            parent: Frame { origin: DVec3::new(10.0, 5.0, 20.0), z: DVec3::Z, x: DVec3::X },
            offset: DVec3::ZERO,
            angle: 0.0,
            flip: false,
            values: [0.0; 3],
            limits: [None; 3],
            suppressed: false,
            seq: 0,
        }
    }

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn revolute_turns_about_the_axis_and_clamps() {
        let mut j = revolute();
        j.values[0] = FRAC_PI_2;
        let (p, q) = joint_pose(&j);
        assert!(close(p + q * j.child.origin, j.parent.origin), "axis points coincide: {p}");
        assert!(close(q * DVec3::X, DVec3::Y), "turned 90°: {}", q * DVec3::X);
        assert!(close(q * DVec3::Z, DVec3::Z));

        j.limits[0] = Some(Limit { min: 0.0, max: FRAC_PI_2, rest: 0.0 });
        j.values[0] = 2.5;
        let clamped = joint_pose(&j);
        assert!(close(clamped.0, p) && clamped.1.angle_between(q) < 1e-9);
    }

    #[test]
    fn slider_moves_along_the_axis() {
        let mut j = revolute();
        j.kind = JointKind::Slider;
        j.parent.z = DVec3::X; // axis along the parent's +X
        j.parent.x = DVec3::Y;
        let (p0, _) = joint_pose(&j);
        j.values[0] = 10.0;
        let (p1, _) = joint_pose(&j);
        assert!(close(p1 - p0, DVec3::new(10.0, 0.0, 0.0)), "{}", p1 - p0);
    }

    #[test]
    fn joint_attach_round_trips_through_project_file() {
        let mut doc = lab_core::sample::demo_document();
        let mut j = revolute();
        j.limits[0] = Some(Limit { min: 0.0, max: FRAC_PI_2, rest: 0.0 });
        j.values[0] = 0.3;
        j.seq = 4;
        doc.nodes.first_mut().expect("demo has a node").attach = Attach::Joint(j);
        let (back, _) = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert!(back.content_eq(&doc));
    }
}
