//! Feature constraints (Mate, Flush, Concentric, Angle) and their solver (docs/M8-PLAN.md B).
//!
//! Solve = sequential projection: each constraint moves the child the least to satisfy itself,
//! repeated until nothing moves. Free degrees of freedom keep the current pose.

use glam::{DQuat, DVec3};
use lab_core::part::Feature;
use lab_core::{Attach, Command, Constraint, ConstraintKind, Document, NodeId, Parent};

const ROUNDS: usize = 20;
const EPS: f64 = 1e-6;

/// Child pose relative to its parent satisfying `cons`, moving the least from `current`.
pub fn solve(current: (DVec3, DQuat), cons: &[Constraint]) -> (DVec3, DQuat) {
    let (mut p, mut q) = current;
    for _ in 0..ROUNDS {
        let mut moved = 0.0f64;
        for c in cons {
            let (np, nq) = project(p, q, c);
            moved = moved.max((np - p).length()).max(nq.angle_between(q));
            (p, q) = (np, nq.normalize());
        }
        if moved < EPS {
            break;
        }
    }
    (p, q)
}

/// What a constrained child may still do, in the parent frame (POLA-160).
#[derive(Clone, Debug, PartialEq)]
pub struct FreeMotion {
    /// Orthonormal basis of the allowed slides (0 to 3 vectors).
    pub slides: Vec<DVec3>,
    /// The one allowed turn: (point on the axis, unit axis).
    pub turn: Option<(DVec3, DVec3)>,
}

/// Free slides and turn of the child at `pose` (parent frame) under `cons`.
pub fn free_motion(pose: (DVec3, DQuat), cons: &[Constraint]) -> FreeMotion {
    let (p, q) = pose;
    // Directions the child may not slide along; turn candidates (point, axis, is_line).
    let mut blocked = Vec::new();
    let mut turns: Vec<(Option<DVec3>, DVec3, bool)> = Vec::new();
    let mut no_turn = false;
    for c in cons {
        let (lo, _) = ray(&c.child);
        let (po, pd) = ray(&c.parent);
        match c.kind {
            ConstraintKind::Mate | ConstraintKind::Flush => {
                blocked.push(pd);
                turns.push((Some(p + q * lo), pd, false));
            }
            ConstraintKind::Concentric => {
                blocked.extend([perp(pd), pd.cross(perp(pd))]);
                if c.lock_rotation {
                    no_turn = true;
                } else {
                    turns.push((Some(po), pd, true));
                }
            }
            ConstraintKind::Angle => turns.push((None, pd, false)),
        }
    }
    let basis = |dirs: &[DVec3], keep: &mut Vec<DVec3>| {
        for d in dirs {
            let r = keep.iter().fold(*d, |r, k| r - *k * r.dot(*k));
            if r.length() > 1e-6 {
                keep.push(r.normalize());
            }
        }
    };
    let mut span = Vec::new();
    basis(&blocked, &mut span);
    let n = span.len();
    basis(&[DVec3::X, DVec3::Y, DVec3::Z], &mut span);
    let slides = span.split_off(n);
    let turn = match turns.first() {
        _ if no_turn => None,
        None => Some((p, DVec3::Z)),
        Some(&(_, axis, _)) if turns.iter().any(|t| t.1.cross(axis).length() > 1e-6) => None,
        Some(&(_, axis, _)) => {
            let line = turns.iter().find(|t| t.2).and_then(|t| t.0);
            Some((line.or_else(|| turns.iter().find_map(|t| t.0)).unwrap_or(p), axis))
        }
    };
    FreeMotion { slides, turn }
}

/// Default kind for a picked pair; None for plane+axis.
pub fn default_kind(child: &Feature, parent: &Feature) -> Option<ConstraintKind> {
    match (child, parent) {
        (Feature::Plane { .. }, Feature::Plane { .. }) => Some(ConstraintKind::Mate),
        (Feature::Axis { .. }, Feature::Axis { .. }) => Some(ConstraintKind::Concentric),
        _ => None,
    }
}

/// `SetAttach` constraining `id` to `parent`, solved from the node's current world pose
/// re-expressed in the new parent's frame. Deviation from the plan: lives here, not on
/// `Command`, because lab-core cannot depend on lab-mates. `None` for unknown ids.
pub fn constrain(doc: &Document, id: NodeId, parent: Parent, constraints: Vec<Constraint>) -> Option<Command> {
    let (cp, cq) = world_pose(doc, Parent::Node(id))?;
    let (pp, pq) = world_pose(doc, parent)?;
    let inv = pq.inverse();
    let (position, rotation) = solve((inv * (cp - pp), inv * cq), &constraints);
    Some(Command::SetAttach { id, parent, attach: Attach::Constrained { constraints }, position, rotation })
}

/// Room-frame pose of a parent frame: room, platform frame (top surface) or a node's own frame.
pub fn world_pose(doc: &Document, at: Parent) -> Option<(DVec3, DQuat)> {
    match at {
        Parent::Room => Some((DVec3::ZERO, DQuat::IDENTITY)),
        Parent::Platform(p) => doc.platform_pose(p).map(|(o, r)| (o, DQuat::from_rotation_z(r))),
        Parent::Node(n) => {
            let (lp, lq) = doc.node_pose(n)?;
            let (rp, rq) = match doc.root_platform(n) {
                Some(p) => world_pose(doc, Parent::Platform(p))?,
                None => (DVec3::ZERO, DQuat::IDENTITY),
            };
            Some((rp + rq * lp, rq * lq))
        }
    }
}

/// (origin, direction) of a feature: plane normal or axis direction.
fn ray(f: &Feature) -> (DVec3, DVec3) {
    match f {
        Feature::Plane { origin, normal } => (*origin, normal.normalize()),
        Feature::Axis { origin, dir, .. } => (*origin, dir.normalize()),
    }
}

/// Rotates the pose about the world point `c`.
fn rotate_about(p: DVec3, q: DQuat, c: DVec3, r: DQuat) -> (DVec3, DQuat) {
    (c + r * (p - c), r * q)
}

/// A unit vector perpendicular to `d` (twist reference for axes).
fn perp(d: DVec3) -> DVec3 {
    let a = if d.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    a.cross(d).normalize()
}

/// Signed angle from `a` to `b` about `axis` (all perpendicular-ish to `axis`).
fn signed_angle(a: DVec3, b: DVec3, axis: DVec3) -> f64 {
    let a = (a - axis * a.dot(axis)).normalize_or_zero();
    let b = (b - axis * b.dot(axis)).normalize_or_zero();
    a.cross(b).dot(axis).atan2(a.dot(b))
}

/// The least move of the child pose `(p, q)` that satisfies `c` alone.
fn project(p: DVec3, q: DQuat, c: &Constraint) -> (DVec3, DQuat) {
    let (lo, ld) = ray(&c.child);
    let (po, pd) = ray(&c.parent);
    let (co, cd) = (p + q * lo, q * ld);
    match c.kind {
        ConstraintKind::Mate | ConstraintKind::Flush => {
            let target = if c.kind == ConstraintKind::Mate { -pd } else { pd };
            let (p, q) = rotate_about(p, q, co, DQuat::from_rotation_arc(cd, target));
            let d = (co - po).dot(pd) - c.offset;
            (p - pd * d, q)
        }
        ConstraintKind::Concentric => {
            let target = if c.flip { -pd } else { pd };
            let (mut p, mut q) = rotate_about(p, q, co, DQuat::from_rotation_arc(cd, target));
            if c.lock_rotation {
                let twist = signed_angle(q * perp(ld), perp(pd), target) - c.angle;
                (p, q) = rotate_about(p, q, co, DQuat::from_axis_angle(target, twist));
            }
            let off = co - po;
            (p - (off - pd * off.dot(pd)), q)
        }
        ConstraintKind::Angle => {
            let now = cd.angle_between(pd);
            let axis = cd.cross(pd).try_normalize().unwrap_or_else(|| perp(cd));
            rotate_about(p, q, co, DQuat::from_axis_angle(axis, now - c.angle))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(origin: DVec3) -> Feature {
        Feature::Axis { origin, dir: DVec3::Z, radius: 6.35, hole: false }
    }

    fn plane(origin: DVec3, normal: DVec3) -> Feature {
        Feature::Plane { origin, normal }
    }

    fn con(kind: ConstraintKind, child: Feature, parent: Feature, offset: f64) -> Constraint {
        Constraint { kind, child, parent, offset, angle: 0.0, flip: false, lock_rotation: false }
    }

    fn concentric() -> Constraint {
        // Post axis at its own origin; holder bore at (10, 5) along +Z.
        con(ConstraintKind::Concentric, axis(DVec3::ZERO), axis(DVec3::new(10.0, 5.0, 0.0)), 0.0)
    }

    #[test]
    fn concentric_aligns_axes_and_keeps_height() {
        let tilt = DQuat::from_rotation_x(0.2);
        let (p, q) = solve((DVec3::new(0.0, 0.0, 30.0), tilt), &[concentric()]);
        let dir = q * DVec3::Z;
        assert!(dir.angle_between(DVec3::Z) < 1e-6, "{dir}");
        assert!((p.truncate() - glam::DVec2::new(10.0, 5.0)).length() < 1e-6, "{p}");
        assert!((p.z - 30.0).abs() < 1e-6, "{p}");
    }

    #[test]
    fn mate_offset_moves_along_axis() {
        // Post end face (bottom, -Z) onto the holder bottom face (+Z up from the parent frame at z=0).
        let mate = |offset| {
            con(ConstraintKind::Mate, plane(DVec3::ZERO, -DVec3::Z), plane(DVec3::new(10.0, 5.0, 0.0), DVec3::Z), offset)
        };
        let start = (DVec3::new(3.0, 1.0, 40.0), DQuat::IDENTITY);
        let a = solve(start, &[concentric(), mate(10.0)]);
        let b = solve(a, &[concentric(), mate(20.0)]);
        assert!((a.0 - DVec3::new(10.0, 5.0, 10.0)).length() < 1e-6, "{}", a.0);
        assert!((b.0 - a.0 - DVec3::new(0.0, 0.0, 10.0)).length() < 1e-6, "{}", b.0);
    }

    /// Child side face (-X) on a vertical parent face at x = 20 (normal +X).
    fn side_mate() -> Constraint {
        con(ConstraintKind::Mate, plane(DVec3::new(-5.0, 0.0, 10.0), -DVec3::X), plane(DVec3::new(20.0, 0.0, 0.0), DVec3::X), 0.0)
    }

    fn parallel(a: DVec3, b: DVec3) -> bool {
        a.normalize().cross(b.normalize()).length() < 1e-9
    }

    #[test]
    fn free_motion_follows_the_constraints() {
        let pose = (DVec3::new(25.0, 3.0, 0.0), DQuat::IDENTITY);
        let m = free_motion(pose, &[side_mate()]);
        assert_eq!(m.slides.len(), 2);
        assert!(m.slides.iter().all(|s| s.dot(DVec3::X).abs() < 1e-9), "{m:?}");
        let (point, axis) = m.turn.unwrap();
        assert!(parallel(axis, DVec3::X) && (point - DVec3::new(20.0, 3.0, 10.0)).length() < 1e-9, "{m:?}");

        let m = free_motion((DVec3::new(10.0, 5.0, 30.0), DQuat::IDENTITY), &[concentric()]);
        assert!(m.slides.len() == 1 && parallel(m.slides[0], DVec3::Z), "{m:?}");
        assert!(m.turn.is_some_and(|(o, a)| parallel(a, DVec3::Z) && (o - DVec3::new(10.0, 5.0, 0.0)).length() < 1e-9));
        let locked = Constraint { lock_rotation: true, ..concentric() };
        assert_eq!(free_motion((DVec3::ZERO, DQuat::IDENTITY), &[locked]).turn, None);

        // Post in a holder: seated and coaxial → only the turn about the post axis is left.
        let seat = con(ConstraintKind::Mate, plane(DVec3::ZERO, -DVec3::Z), plane(DVec3::new(10.0, 5.0, 0.0), DVec3::Z), 0.0);
        let m = free_motion((DVec3::new(10.0, 5.0, 0.0), DQuat::IDENTITY), &[seat, concentric()]);
        assert!(m.slides.is_empty(), "{m:?}");
        let (point, axis) = m.turn.unwrap();
        assert!(parallel(axis, DVec3::Z) && (point - DVec3::new(10.0, 5.0, 0.0)).length() < 1e-9, "{m:?}");
    }

    #[test]
    fn solve_keeps_free_motion() {
        let start = solve((DVec3::new(25.0, 3.0, 0.0), DQuat::IDENTITY), &[side_mate()]);
        let (o, a) = free_motion(start, &[side_mate()]).turn.unwrap();
        let turned = rotate_about(start.0, start.1, o, DQuat::from_axis_angle(a, 30f64.to_radians()));
        let back = solve(turned, &[side_mate()]);
        assert!((back.0 - turned.0).length() < 1e-6 && back.1.angle_between(turned.1) < 1e-6, "{back:?}");

        let slid = (DVec3::new(10.0, 5.0, 45.0), DQuat::IDENTITY);
        let back = solve(slid, &[concentric()]);
        assert!((back.0 - slid.0).length() < 1e-6, "{back:?}");
    }

    #[test]
    fn constrained_attach_round_trips_through_project_file() {
        let mut doc = lab_core::sample::demo_document();
        let node = doc.nodes.first_mut().expect("demo has a node");
        node.attach = Attach::Constrained { constraints: vec![concentric()] };
        let (back, _) = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert!(back.content_eq(&doc));
    }
}
