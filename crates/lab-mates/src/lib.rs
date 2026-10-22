//! Connector compatibility, mate poses and hole snap (DESIGN.md §4.1, docs/M2-PLAN.md).

pub mod solver;

use glam::{DAffine3, DMat3, DQuat, DVec2, DVec3};
use lab_core::part::{ConnectorKind, Frame, Geometry, ParamValue, PartDef, Params};
use lab_core::{HoleSet, LatticeIndex};

const TOL: f64 = 0.05;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOL
}

/// Same variant, matching spec/diameter (±0.05 mm)/spacing/thread, opposite genders. HoleGridSeat → false.
pub fn compatible(a: &ConnectorKind, b: &ConnectorKind) -> bool {
    use ConnectorKind::*;
    match (a, b) {
        (Thread { spec: s1, gender: g1 }, Thread { spec: s2, gender: g2 }) => s1 == s2 && g1 != g2,
        (Bore { diameter: d1, gender: g1, .. }, Bore { diameter: d2, gender: g2, .. })
        | (OpticBore { diameter: d1, gender: g1 }, OpticBore { diameter: d2, gender: g2 }) => close(*d1, *d2) && g1 != g2,
        (CageRod { spacing: s1, gender: g1 }, CageRod { spacing: s2, gender: g2 }) => close(*s1, *s2) && g1 != g2,
        (LensTube { thread: t1, gender: g1 }, LensTube { thread: t2, gender: g2 }) => t1 == t2 && g1 != g2,
        _ => false,
    }
}

/// (child connector index, parent connector index) for every compatible pair, in index order.
pub fn compatible_pairs(child: &PartDef, parent: &PartDef) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, c) in child.connectors.iter().enumerate() {
        for (j, p) in parent.connectors.iter().enumerate() {
            if compatible(&c.kind, &p.kind) {
                out.push((i, j));
            }
        }
    }
    out
}

/// Slide limits when `parent` is a female bore with a range (else (0, 0)).
pub fn slide_range(parent: &ConnectorKind) -> (f64, f64) {
    match parent {
        ConnectorKind::Bore { gender: lab_core::part::Gender::Female, range: Some([lo, hi]), .. } => (*lo, *hi),
        _ => (0.0, 0.0),
    }
}

fn affine(f: &Frame) -> DAffine3 {
    let z = f.z.normalize();
    let x = (f.x - z * f.x.dot(z)).normalize();
    DAffine3::from_mat3_translation(DMat3::from_cols(x, z.cross(x), z), f.origin)
}

/// Child part pose in the parent part's frame (formula under "Mate pose").
pub fn mate_pose(parent: &Frame, child: &Frame, slide: f64, twist: f64) -> (DVec3, DQuat) {
    let f = affine(parent)
        * DAffine3::from_translation(DVec3::new(0.0, 0.0, slide))
        * DAffine3::from_rotation_z(twist)
        * DAffine3::from_rotation_x(std::f64::consts::PI);
    let pose = f * affine(child).inverse();
    (pose.translation, DQuat::from_mat3(&pose.matrix3).normalize())
}

/// A connector's frame for the given param overrides: for generators `post`, `pedestal_post`,
/// `post_holder` and `riser`, a connector whose origin sits on the part's top (z == default
/// length / height) moves to the overridden length / height. Every other frame is returned unchanged.
pub fn connector_frame(def: &PartDef, conn: &str, params: &Params) -> Option<Frame> {
    let mut frame = def.connector(conn)?.frame;
    if let Geometry::Parametric { generator, params: defaults } = &def.geometry {
        let key = match generator.as_str() {
            "post" | "pedestal_post" => "length",
            "post_holder" | "riser" => "height",
            _ => return Some(frame),
        };
        let get = |p: &Params| p.get(key).and_then(ParamValue::as_f64);
        if let (Some(old), Some(new)) = (get(defaults), get(params)) {
            if close(frame.origin.z, old) {
                frame.origin.z = new;
            }
        }
    }
    Some(frame)
}

/// Part pose in the platform frame when its `seat` connector is bolted to the hole at `hole`,
/// turned by `angle` about +Z. A part without a seat stands with its origin on the hole.
pub fn seat_pose(part: &PartDef, hole: DVec2, angle: f64) -> (DVec3, DQuat) {
    let hole_frame = Frame { origin: hole.extend(0.0), z: DVec3::Z, x: DVec3::X };
    match part.connector("seat") {
        Some(seat) => mate_pose(&hole_frame, &seat.frame, 0.0, angle),
        None => (hole.extend(0.0), DQuat::from_rotation_z(angle)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    pub hole: LatticeIndex,
    pub position: DVec2,
    pub distance: f64,
}

/// Nearest existing hole to `p` (platform-local) within `max_distance`, using HoleSet::nearest
/// (outlines and exclusion zones are already applied there).
pub fn snap(holes: &HoleSet, p: DVec2, max_distance: f64) -> Option<SnapHit> {
    let hole = holes.nearest(p)?;
    let position = holes.position(hole);
    let distance = position.distance(p);
    (distance <= max_distance).then_some(SnapHit { hole, position, distance })
}

#[cfg(test)]
mod tests {
    use lab_core::part::{self, Gender};
    use lab_core::*;

    use super::*;

    fn thread(gender: Gender) -> ConnectorKind {
        ConnectorKind::Thread { spec: "8-32".into(), gender }
    }

    #[test]
    fn thread_genders() {
        assert!(compatible(&thread(Gender::Male), &thread(Gender::Female)));
        assert!(!compatible(&thread(Gender::Male), &thread(Gender::Male)));
        assert!(!compatible(&ConnectorKind::HoleGridSeat, &ConnectorKind::HoleGridSeat));
    }

    #[test]
    fn post_into_holder() {
        let bore = Frame { origin: DVec3::new(0.0, 0.0, 50.0), z: DVec3::Z, x: DVec3::X };
        let bottom = Frame { origin: DVec3::ZERO, z: DVec3::NEG_Z, x: DVec3::X };
        let (p, q) = mate_pose(&bore, &bottom, -10.0, 0.0);
        assert!(p.distance(DVec3::new(0.0, 0.0, 40.0)) < 1e-9, "{p}");
        assert!((q * DVec3::Z).distance(DVec3::Z) < 1e-9);
        assert!((q * DVec3::X).distance(DVec3::X) < 1e-9);
    }

    #[test]
    fn post_top_follows_length() {
        let conn = |name: &str, z: f64| part::ConnectorDef {
            name: name.into(),
            kind: thread(Gender::Male),
            frame: Frame { origin: DVec3::new(0.0, 0.0, z), z: DVec3::Z, x: DVec3::X },
        };
        let def = PartDef {
            id: "thorlabs:TR2".into(),
            version: 1,
            vendor: String::new(),
            part_number: String::new(),
            name: "TR2".into(),
            category: "posts/posts".into(),
            tags: Vec::new(),
            geometry: Geometry::Parametric {
                generator: "post".into(),
                params: Params::from([("length".to_string(), ParamValue::Number(50.8))]),
            },
            connectors: vec![conn("bottom", 0.0), conn("top", 50.8)],
            optical_ports: Vec::new(),
            meta: Default::default(),
            features: Vec::new(),
        };
        let base = connector_frame(&def, "top", &Params::new()).unwrap();
        let params = Params::from([("length".to_string(), ParamValue::Number(50.8 + 25.0))]);
        let top = connector_frame(&def, "top", &params).unwrap();
        assert!((top.origin.z - base.origin.z - 25.0).abs() < 1e-9);
        assert_eq!(connector_frame(&def, "bottom", &params), def.connector("bottom").map(|c| c.frame));
    }

    #[test]
    fn snap_skips_exclusion_zone() {
        let mut grid = HoleGrid::metric();
        grid.exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 50.0 },
        });
        let platform = Platform {
            id: PlatformId(1),
            name: "Table A".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 210.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(grid),
            support: Support::Legs { size: 120.0, inset: 120.0 },
        };
        let holes = compute_holes(&platform).unwrap();
        for p in [DVec2::ZERO, DVec2::new(12.5, 12.5), DVec2::new(-20.0, 30.0)] {
            let hit = snap(&holes, p, 100.0).unwrap();
            assert!(hit.position.length() > 50.0, "{hit:?}");
        }
        assert!(snap(&holes, DVec2::ZERO, 10.0).is_none());
    }
}
