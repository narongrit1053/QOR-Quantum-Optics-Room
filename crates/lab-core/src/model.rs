//! Document types: room, platforms (tables, breadboards, custom plates) and placed nodes.

use glam::{DQuat, DVec2, DVec3};
use lab_geom::FlatPath;
use serde::{Deserialize, Serialize};

use crate::holes::HoleGrid;
use crate::limits::{LEG_MAX_SPAN, MAX_LEGS};
use crate::migrate::{LoadError, LoadReport};
use crate::outline::{Outline, OutlineError};
use crate::room::Room;
use crate::units::Mm;

/// Bumped whenever the serialized layout changes; older files are migrated on load.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlatformId(pub u64);

/// A door or window in a room wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OpeningId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    pub room: Room,
    pub platforms: Vec<Platform>,
    pub nodes: Vec<Node>,
    next_id: u64,
}

impl Document {
    /// An empty document: the default room, no platforms, no nodes.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            room: Room::default(),
            platforms: Vec::new(),
            nodes: Vec::new(),
            next_id: 1,
        }
    }

    /// Mutating allocation. Only for building documents outside `History` (samples,
    /// `default_document`, tests); app code uses [`Document::next_free_id`].
    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Non-mutating: the id the next new object should use (plus k for several). Ids are unique
    /// across nodes, platforms and openings, and only grow (M1-SPEC D12).
    pub fn next_free_id(&self) -> u64 {
        self.next_id
    }

    /// `next_id = max(next_id, used + 1)`.
    pub(crate) fn bump_next_id(&mut self, used: u64) {
        self.next_id = self.next_id.max(used.saturating_add(1));
    }

    pub(crate) fn set_next_id(&mut self, next: u64) {
        self.next_id = next;
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn node_index(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    pub fn platform(&self, id: PlatformId) -> Option<&Platform> {
        self.platforms.iter().find(|p| p.id == id)
    }

    pub fn platform_index(&self, id: PlatformId) -> Option<usize> {
        self.platforms.iter().position(|p| p.id == id)
    }

    /// The nodes placed on platform `id`, in document order.
    pub fn nodes_on(&self, id: PlatformId) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |n| n.platform == id)
    }

    /// Whether any node, platform or opening uses `raw` as its id.
    pub fn contains_id(&self, raw: u64) -> bool {
        self.nodes.iter().any(|n| n.id.0 == raw)
            || self.platforms.iter().any(|p| p.id.0 == raw)
            || self.room.openings.iter().any(|o| o.id.0 == raw)
    }

    /// Largest id in use; 0 if the document has no nodes, platforms or openings.
    pub fn max_id(&self) -> u64 {
        let nodes = self.nodes.iter().map(|n| n.id.0);
        let platforms = self.platforms.iter().map(|p| p.id.0);
        let openings = self.room.openings.iter().map(|o| o.id.0);
        nodes.chain(platforms).chain(openings).max().unwrap_or(0)
    }

    /// Equality ignoring `next_id` (which only grows). Use in every undo/redo test.
    pub fn content_eq(&self, other: &Document) -> bool {
        self.schema_version == other.schema_version
            && self.name == other.name
            && self.room == other.room
            && self.platforms == other.platforms
            && self.nodes == other.nodes
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Parses, migrates, repairs and validates a project (M1-SPEC §3.11).
    pub fn from_json(json: &str) -> Result<(Document, LoadReport), LoadError> {
        crate::migrate::load(json)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlatformKind {
    OpticalTable,
    Breadboard,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Surface {
    Stainless,
    BlackAnodized,
}

/// An optical table, breadboard or custom platform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub id: PlatformId,
    pub name: String,
    pub kind: PlatformKind,
    pub surface: Surface,
    /// Top-surface outline in the platform frame.
    pub outline: Outline,
    pub thickness: Mm,
    /// Height of the top surface above the floor.
    pub top_height: Mm,
    /// Frame origin in room coordinates.
    pub position: DVec2,
    /// Rotation about room +Z.
    pub rotation_z: f64,
    pub hole_grid: Option<HoleGrid>,
    pub support: Support,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Support {
    /// Legs inset from the outline, reaching the floor.
    Legs { size: Mm, inset: Mm },
    /// Sits directly on whatever is below it (e.g. a breadboard on a table).
    Fixed,
}

impl Platform {
    fn rotation(&self) -> DVec2 {
        DVec2::from_angle(self.rotation_z)
    }

    /// Platform-local point to room coordinates.
    pub fn local_to_room(&self, p: DVec2) -> DVec2 {
        self.position + self.rotation().rotate(p)
    }

    /// Room point to platform-local coordinates.
    pub fn room_to_local(&self, p: DVec2) -> DVec2 {
        let r = self.rotation();
        DVec2::new(r.x, -r.y).rotate(p - self.position)
    }

    /// The flattened outline (fillets as arcs within FLATTEN_TOLERANCE), validated.
    pub fn outline_path(&self) -> Result<FlatPath, OutlineError> {
        self.outline.flatten()
    }

    /// Leg centres in the platform frame (M1-SPEC §3.3); empty for `Support::Fixed`.
    pub fn leg_positions(&self) -> Vec<DVec2> {
        let Support::Legs { size, inset } = self.support else {
            return Vec::new();
        };
        let Ok(path) = self.outline_path() else {
            return Vec::new();
        };
        let flat = &path.points;
        let mut corners: Vec<DVec2> = self.outline.corners().iter().map(|c| c.point).collect();
        if lab_geom::signed_area(&corners) < 0.0 {
            corners.reverse();
        }
        let n = corners.len();

        // 1. Corner candidates at distance `inset` from both edges of every corner up to 150°.
        let max_angle = 150f64.to_radians() + 1e-9;
        let candidates: Vec<DVec2> = (0..n)
            .filter(|&k| lab_geom::interior_angle(&corners, k) <= max_angle)
            .map(|k| {
                let p = corners[k];
                let u = (corners[(k + n - 1) % n] - p).normalize_or_zero();
                let v = (corners[(k + 1) % n] - p).normalize_or_zero();
                let theta = u.dot(v).clamp(-1.0, 1.0).acos();
                p + (u + v).normalize_or_zero() * (inset / (theta / 2.0).sin())
            })
            .collect();

        // 2. Span fill between consecutive candidates along the outline.
        let m = candidates.len();
        let mut points = Vec::with_capacity(m * 2);
        for k in 0..m {
            let a = candidates[k];
            points.push(a);
            let b = candidates[(k + 1) % m];
            let d = a.distance(b);
            if m > 1 && d > LEG_MAX_SPAN {
                let extra = (d / LEG_MAX_SPAN).ceil() as usize - 1;
                for s in 1..=extra {
                    points.push(a + (b - a) * (s as f64 / (extra + 1) as f64));
                }
            }
        }

        // 3. Keep test and 4. spacing.
        let select = |points: &[DVec2]| {
            let mut kept: Vec<DVec2> = Vec::new();
            for &c in points {
                if kept.len() == MAX_LEGS {
                    break;
                }
                let inside =
                    lab_geom::point_in_polygon(flat, c) && lab_geom::distance_to_boundary(flat, c) >= size * 0.7072;
                if inside && kept.iter().all(|k| k.distance(c) >= size) {
                    kept.push(c);
                }
            }
            kept
        };
        let legs = select(&points);
        if !legs.is_empty() {
            return legs;
        }

        // 5. Fallback: the bounding box corners inset, then the centre.
        let (lo, hi) = self.outline.bounds();
        let legs = select(&[
            DVec2::new(lo.x + inset, lo.y + inset),
            DVec2::new(hi.x - inset, lo.y + inset),
            DVec2::new(hi.x - inset, hi.y - inset),
            DVec2::new(lo.x + inset, hi.y - inset),
        ]);
        if !legs.is_empty() {
            return legs;
        }
        let centre = (lo + hi) / 2.0;
        if lab_geom::point_in_polygon(flat, centre) { vec![centre] } else { Vec::new() }
    }

    /// Room-space bounding box of the platform, down to the floor for legs, otherwise to the
    /// underside.
    pub fn room_aabb(&self) -> (DVec3, DVec3) {
        let points: Vec<DVec2> = self.outline.corners().iter().map(|c| self.local_to_room(c.point)).collect();
        let (lo, hi) = lab_geom::bounds(&points);
        let bottom = match self.support {
            Support::Legs { .. } => 0.0,
            Support::Fixed => self.top_height - self.thickness,
        };
        (lo.extend(bottom), hi.extend(self.top_height))
    }
}

/// A placed object. In M0/M1 every node carries a primitive shape; catalog parts with meshes and
/// connectors replace this in M2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    /// Catalog part id, e.g. `generic:post-12.7`.
    pub part: String,
    pub platform: PlatformId,
    /// Centre of the shape in the platform frame (origin on the top surface).
    pub position: DVec3,
    pub rotation: DQuat,
    pub shape: Shape,
    pub finish: Finish,
    pub pickable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Box {
        size: DVec3,
    },
    /// Axis along local +Z, centred on the node position.
    Cylinder {
        radius: Mm,
        length: Mm,
    },
}

impl Shape {
    /// Half-extent along local +Z, i.e. how far the shape reaches above its centre.
    pub fn half_height(&self) -> Mm {
        match self {
            Shape::Box { size } => size.z / 2.0,
            Shape::Cylinder { length, .. } => length / 2.0,
        }
    }
}

/// Surface finish. The viewport decides how each finish is rendered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Finish {
    AnodizedBlack,
    Stainless,
    Mirror,
    Glass,
    Painted {
        rgb: [u8; 3],
    },
    /// A visualised laser beam (not a physical part).
    Beam {
        wavelength_nm: f64,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::outline::{OutlineCorner, RectCorner};

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn table(outline: Outline, inset: f64) -> Platform {
        Platform {
            id: PlatformId(1),
            name: "Table A".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline,
            thickness: 210.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(HoleGrid::metric()),
            support: Support::Legs { size: 120.0, inset },
        }
    }

    fn key(p: DVec2) -> (i64, i64) {
        ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64)
    }

    #[test]
    fn l1_rectangle_legs_match_m0() {
        let p = table(Outline::Rectangle { size: v(1200.0, 900.0) }, 120.0);
        let legs = p.leg_positions();
        let (hx, hy) = (600.0 - 120.0, 450.0 - 120.0);
        let m0: BTreeSet<_> = [v(-hx, -hy), v(hx, -hy), v(hx, hy), v(-hx, hy)].into_iter().map(key).collect();
        assert_eq!(legs.len(), 4);
        for leg in &legs {
            let near = [v(-hx, -hy), v(hx, -hy), v(hx, hy), v(-hx, hy)].iter().any(|m| m.distance(*leg) < 1e-9);
            assert!(near, "{leg}");
        }
        assert_eq!(legs.iter().copied().map(key).collect::<BTreeSet<_>>(), m0);
    }

    #[test]
    fn l1_span_fill() {
        let legs = table(Outline::Rectangle { size: v(3000.0, 1500.0) }, 150.0).leg_positions();
        assert_eq!(legs.len(), 6);
        assert!(legs.iter().any(|p| p.distance(v(0.0, -600.0)) < 1e-9));
        assert!(legs.iter().any(|p| p.distance(v(0.0, 600.0)) < 1e-9));
        assert_eq!(table(Outline::Rectangle { size: v(2400.0, 1200.0) }, 150.0).leg_positions().len(), 4);
        // A clockwise polygon gives the same legs.
        let mut corners = Outline::Rectangle { size: v(3000.0, 1500.0) }.corners();
        corners.reverse();
        let cw = table(Outline::Polygon { corners }, 150.0).leg_positions();
        assert_eq!(cw.iter().copied().map(key).collect::<BTreeSet<_>>(), legs.iter().copied().map(key).collect());
    }

    #[test]
    fn l1_l_shape_has_five_legs() {
        let outline =
            Outline::LShape { size: v(1200.0, 900.0), notch: v(400.0, 300.0), notch_corner: RectCorner::XPosYPos };
        let legs = table(outline, 150.0).leg_positions();
        assert_eq!(legs.len(), 5);
        assert!(!legs.iter().any(|p| p.x > 200.0 - 1e-9 && p.y > 150.0 - 1e-9));
    }

    #[test]
    fn l1_forty_gon() {
        // Alternating tips and dents: 20 convex corners below 150°.
        let corners: Vec<OutlineCorner> = (0..40)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / 40.0;
                let r = if k % 2 == 0 { 2000.0 } else { 1500.0 };
                OutlineCorner::sharp(v(a.cos(), a.sin()) * r)
            })
            .collect();
        let p = table(Outline::Polygon { corners }, 150.0);
        let legs = p.leg_positions();
        assert!(!legs.is_empty() && legs.len() <= MAX_LEGS, "{}", legs.len());
        let flat = p.outline_path().unwrap().points;
        for leg in &legs {
            assert!(lab_geom::point_in_polygon(&flat, *leg));
            assert!(lab_geom::distance_to_boundary(&flat, *leg) >= 120.0 * 0.7072);
        }
        // A regular 40-gon has no corner below 150°: the fallback leaves only the centre.
        let corners: Vec<OutlineCorner> = (0..40)
            .map(|k| OutlineCorner::sharp(DVec2::from_angle(k as f64 * std::f64::consts::TAU / 40.0) * 1000.0))
            .collect();
        let legs = table(Outline::Polygon { corners }, 150.0).leg_positions();
        assert_eq!(legs.len(), 1);
        assert!(legs[0].length() < 1e-9);
    }

    #[test]
    fn fixed_support_and_frames() {
        let mut p = table(Outline::Rectangle { size: v(600.0, 400.0) }, 120.0);
        p.position = v(1000.0, -500.0);
        p.rotation_z = std::f64::consts::FRAC_PI_2;
        let room = p.local_to_room(v(300.0, 0.0));
        assert!(room.distance(v(1000.0, -200.0)) < 1e-9);
        assert!(p.room_to_local(room).distance(v(300.0, 0.0)) < 1e-9);
        let (lo, hi) = p.room_aabb();
        assert!(lo.distance(DVec3::new(800.0, -800.0, 0.0)) < 1e-9, "{lo}");
        assert!(hi.distance(DVec3::new(1200.0, -200.0, 800.0)) < 1e-9, "{hi}");
        p.support = Support::Fixed;
        assert!(p.leg_positions().is_empty());
        assert_eq!(p.room_aabb().0.z, 590.0);
    }

    #[test]
    fn ids_and_lookup() {
        let mut doc = Document::new("Test");
        assert_eq!(doc.max_id(), 0);
        assert_eq!(doc.next_free_id(), 1);
        assert_eq!(doc.allocate_id(), 1);
        assert_eq!(doc.next_free_id(), 2);
        doc.bump_next_id(10);
        assert_eq!(doc.next_free_id(), 11);
        doc.bump_next_id(3);
        assert_eq!(doc.next_free_id(), 11);
        let mut other = doc.clone();
        other.set_next_id(50);
        assert!(doc.content_eq(&other));
        assert_ne!(doc, other);
        other.name = "Other".into();
        assert!(!doc.content_eq(&other));
        assert!(!doc.contains_id(1));
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
    }
}
