//! A small demo setup: one metric optical table with a laser, two steering mirrors, a lens,
//! a half-wave plate, a PBS and two detectors at 75 mm beam height.

use std::f64::consts::FRAC_PI_2;

use glam::{DQuat, DVec2, DVec3};

use crate::holes::{HoleGrid, HoleRef, HoleSet, compute_holes};
use crate::model::*;
use crate::outline::Outline;
use crate::units::Mm;

/// Beam height above the table surface.
pub const BEAM_HEIGHT: Mm = 75.0;

const BASE_THICKNESS: Mm = 10.0;
const HOLDER_TOP: Mm = BASE_THICKNESS + 40.0;
const POST_BOTTOM: Mm = 20.0;
const POST_RADIUS: Mm = 6.35;

/// Rotation that points local +Z along the horizontal direction `dir`, keeping local X vertical
/// (down). Square plates stay upright instead of turning into diamonds.
pub fn facing(dir: DVec3) -> DQuat {
    DQuat::from_rotation_z(dir.y.atan2(dir.x)) * DQuat::from_rotation_y(FRAC_PI_2)
}

struct Builder<'a> {
    doc: &'a mut Document,
    platform: PlatformId,
    /// The platform's holes (computed once; the builder never edits the platform).
    holes: HoleSet,
}

impl Builder<'_> {
    fn add(
        &mut self,
        name: &str,
        part: &str,
        position: DVec3,
        rotation: DQuat,
        shape: Shape,
        finish: Finish,
    ) -> NodeId {
        let id = NodeId(self.doc.allocate_id());
        self.doc.nodes.push(Node {
            id,
            name: name.into(),
            part: part.into(),
            platform: self.platform,
            position,
            rotation,
            shape,
            pickable: !matches!(finish, Finish::Beam { .. }),
            finish,
        });
        id
    }

    fn hole(&self, col: u32, row: u32) -> DVec2 {
        self.holes.hole_at(HoleRef { col, row }).expect("INVARIANT: the demo hole exists")
    }

    /// Base + post holder + post, ending at `post_top` above the table.
    fn post_stack(&mut self, label: &str, at: DVec2, yaw: f64, post_top: Mm) {
        let rot = DQuat::from_rotation_z(yaw);
        self.add(
            &format!("{label} · base"),
            "generic:base-25x75",
            at.extend(BASE_THICKNESS / 2.0),
            rot,
            Shape::Box { size: DVec3::new(25.0, 75.0, BASE_THICKNESS) },
            Finish::AnodizedBlack,
        );
        self.add(
            &format!("{label} · post holder"),
            "generic:post-holder-40",
            at.extend((BASE_THICKNESS + HOLDER_TOP) / 2.0),
            DQuat::IDENTITY,
            Shape::Cylinder { radius: 12.7, length: HOLDER_TOP - BASE_THICKNESS },
            Finish::AnodizedBlack,
        );
        let length = post_top - POST_BOTTOM;
        self.add(
            &format!("{label} · post"),
            &format!("generic:post-12.7-{length:.0}"),
            at.extend(POST_BOTTOM + length / 2.0),
            DQuat::IDENTITY,
            Shape::Cylinder { radius: POST_RADIUS, length },
            Finish::Stainless,
        );
    }

    /// A Ø1" mirror in a kinematic mount. `normal` is the mirror's reflecting direction.
    fn mirror(&mut self, label: &str, col: u32, row: u32, normal: DVec3) {
        let at = self.hole(col, row);
        let normal = normal.normalize();
        let centre = at.extend(BEAM_HEIGHT);
        self.post_stack(label, at, normal.y.atan2(normal.x), BEAM_HEIGHT - 19.0);
        self.add(
            &format!("{label} · mount"),
            "generic:kinematic-mount-1in",
            centre - normal * 9.5,
            facing(normal),
            Shape::Box { size: DVec3::new(38.0, 38.0, 13.0) },
            Finish::AnodizedBlack,
        );
        self.add(
            label,
            "generic:mirror-1in",
            centre,
            facing(normal),
            Shape::Cylinder { radius: 12.7, length: 6.0 },
            Finish::Mirror,
        );
    }

    /// A round optic (lens or waveplate) in a ring mount, axis along `axis`.
    #[allow(clippy::too_many_arguments)] // a private demo builder, kept as in M0
    fn ring_optic(
        &mut self,
        label: &str,
        part: &str,
        col: u32,
        row: u32,
        axis: DVec3,
        ring_radius: Mm,
        ring_length: Mm,
    ) {
        let at = self.hole(col, row);
        let rot = facing(axis);
        self.post_stack(label, at, axis.y.atan2(axis.x) + FRAC_PI_2, BEAM_HEIGHT - ring_radius);
        self.add(
            &format!("{label} · mount"),
            "generic:ring-mount-1in",
            at.extend(BEAM_HEIGHT),
            rot,
            Shape::Cylinder { radius: ring_radius, length: ring_length },
            Finish::AnodizedBlack,
        );
        self.add(
            label,
            part,
            at.extend(BEAM_HEIGHT),
            rot,
            Shape::Cylinder { radius: 12.7, length: ring_length + 1.0 },
            Finish::Glass,
        );
    }

    fn detector(&mut self, label: &str, col: u32, row: u32, facing_dir: DVec3) {
        let at = self.hole(col, row);
        self.post_stack(label, at, facing_dir.y.atan2(facing_dir.x), BEAM_HEIGHT - 20.0);
        self.add(
            label,
            "generic:photodiode",
            at.extend(BEAM_HEIGHT),
            facing(facing_dir),
            Shape::Box { size: DVec3::new(40.0, 40.0, 25.0) },
            Finish::Painted { rgb: [44, 48, 56] },
        );
    }

    fn beam(&mut self, label: &str, from: DVec2, to: DVec2) {
        let (a, b) = (from.extend(BEAM_HEIGHT), to.extend(BEAM_HEIGHT));
        self.add(
            label,
            "beam",
            (a + b) / 2.0,
            facing(b - a),
            Shape::Cylinder { radius: 0.75, length: a.distance(b) },
            Finish::Beam { wavelength_nm: 780.0 },
        );
    }
}

pub fn demo_document() -> Document {
    let mut doc = Document::new("Demo — Rb-87 beam path");
    let table = PlatformId(doc.allocate_id());
    let platform = Platform {
        id: table,
        name: "Table A".into(),
        kind: PlatformKind::OpticalTable,
        surface: Surface::Stainless,
        outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
        thickness: 210.0,
        top_height: 800.0,
        position: DVec2::ZERO,
        rotation_z: 0.0,
        hole_grid: Some(HoleGrid::metric()),
        support: Support::Legs { size: 120.0, inset: 120.0 },
    };
    let holes = compute_holes(&platform).expect("INVARIANT: the demo table is valid");
    doc.platforms.push(platform);

    let mut b = Builder { doc: &mut doc, platform: table, holes };

    // Laser head sitting directly on the table, beam leaving its +X face.
    let laser_centre = b.hole(6, 8);
    b.add(
        "Laser (ECDL, 780 nm)",
        "generic:laser-head",
        laser_centre.extend(45.0),
        DQuat::IDENTITY,
        Shape::Box { size: DVec3::new(300.0, 120.0, 90.0) },
        Finish::Painted { rgb: [58, 64, 74] },
    );
    let exit = laser_centre + DVec2::new(150.0, 0.0);

    let x = DVec3::X;
    let y = DVec3::Y;
    b.mirror("M1", 18, 8, -x + y);
    b.mirror("M2", 18, 24, x - y);
    b.ring_optic("L1 (f = 100 mm)", "generic:lens-1in-f100", 26, 24, x, 19.0, 8.0);
    b.ring_optic("HWP", "generic:half-wave-plate-1in", 30, 24, x, 22.0, 14.0);

    let pbs_at = b.hole(34, 24);
    b.post_stack("PBS", pbs_at, 0.0, BEAM_HEIGHT - 12.7 - 5.0);
    b.add(
        "PBS · platform",
        "generic:prism-platform",
        pbs_at.extend(BEAM_HEIGHT - 12.7 - 2.5),
        DQuat::IDENTITY,
        Shape::Box { size: DVec3::new(30.0, 30.0, 5.0) },
        Finish::AnodizedBlack,
    );
    b.add(
        "PBS",
        "generic:pbs-cube-1in",
        pbs_at.extend(BEAM_HEIGHT),
        DQuat::IDENTITY,
        Shape::Box { size: DVec3::splat(25.4) },
        Finish::Glass,
    );

    b.detector("D1 (transmitted)", 42, 24, -x);
    b.detector("D2 (reflected)", 34, 12, y);

    let m1 = b.hole(18, 8);
    let m2 = b.hole(18, 24);
    let d1 = b.hole(42, 24) - DVec2::new(12.5, 0.0);
    let d2 = b.hole(34, 12) + DVec2::new(0.0, 12.5);
    b.beam("Beam · laser → M1", exit, m1);
    b.beam("Beam · M1 → M2", m1, m2);
    b.beam("Beam · M2 → PBS", m2, pbs_at);
    b.beam("Beam · PBS → D1", pbs_at, d1);
    b.beam("Beam · PBS → D2", pbs_at, d2);

    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_document_is_consistent() {
        let doc = demo_document();
        assert_eq!(doc.validate(), Ok(()));
        assert!(doc.room.openings.is_empty());
        assert!(doc.nodes.len() > 30);
        let mut ids: Vec<_> = doc.nodes.iter().map(|n| n.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), doc.nodes.len(), "node ids are unique");
        assert!(doc.nodes.iter().all(|n| doc.platform(n.platform).is_some()));
    }

    #[test]
    fn optics_sit_at_beam_height() {
        let doc = demo_document();
        for name in ["M1", "M2", "PBS", "HWP"] {
            let node = doc.nodes.iter().find(|n| n.name == name).unwrap();
            assert!((node.position.z - BEAM_HEIGHT).abs() < 1e-9, "{name}");
        }
    }

    #[test]
    fn posts_reach_their_mounts() {
        let doc = demo_document();
        for post in doc.nodes.iter().filter(|n| n.name.ends_with("· post")) {
            let top = post.position.z + post.shape.half_height();
            assert!(top > HOLDER_TOP && top < BEAM_HEIGHT, "{} top at {top}", post.name);
        }
    }

    #[test]
    fn json_round_trip() {
        let doc = demo_document();
        let json = doc.to_json().unwrap();
        let (loaded, report) = Document::from_json(&json).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(report, crate::migrate::LoadReport::default());
    }

    #[test]
    fn rejects_newer_schema() {
        let mut doc = demo_document();
        doc.schema_version = SCHEMA_VERSION + 1;
        let json = doc.to_json().unwrap();
        assert!(matches!(
            Document::from_json(&json),
            Err(crate::migrate::LoadError::NewerSchema { found: 3, supported: 2 })
        ));
    }

    #[test]
    fn facing_keeps_plates_upright() {
        let q = facing(DVec3::new(-1.0, 1.0, 0.0).normalize());
        let z = q * DVec3::Z;
        assert!((z - DVec3::new(-1.0, 1.0, 0.0).normalize()).length() < 1e-9);
        // Local X ends up vertical, so square plates have vertical edges.
        assert!((q * DVec3::X).truncate().length() < 1e-9);
    }
}
