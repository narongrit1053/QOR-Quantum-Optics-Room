//! Beam trace and Gaussian beam size (docs/M6-PLAN.md section A, DESIGN.md §10 "Beam paths").
//!
//! Sequential ray trace of the beam axis through the optical ports of the placed parts, with a
//! Gaussian q-parameter carried per segment (ABCD, 1/e² radius `w`). Units: mm, radians, room
//! frame (+Z up). Pure and deterministic: the viewport and the app call the same functions.

use glam::{DAffine3, DQuat, DVec3};
use lab_core::part::{ParamValue, PartDef};
use lab_core::{Document, NodeId, Shape};

/// Port radius when a part gives no clear aperture.
pub const DEFAULT_RADIUS: f64 = 12.7;
/// Lens focal length when a lens gives no `focal_length_mm`.
pub const DEFAULT_FOCAL_LENGTH: f64 = 100.0;
/// Most segments traced per source; then the trace stops.
pub const MAX_SEGMENTS: usize = 64;
/// Longest segment: a beam that hits nothing ends here.
pub const MAX_SEGMENT_LENGTH: f64 = 5000.0;
const EPS: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    Source,
    Mirror,
    Lens,
    Splitter,
    /// Waveplate, iris, anything unknown: the beam passes unchanged.
    Pass,
    /// Dump, detector, fiber: the beam ends here.
    Stop,
}

impl PortKind {
    pub fn from_kind(kind: &str) -> Self {
        match kind {
            "source" => PortKind::Source,
            "mirror" => PortKind::Mirror,
            "lens" => PortKind::Lens,
            "splitter" => PortKind::Splitter,
            "dump" | "detector" | "fiber" => PortKind::Stop,
            _ => PortKind::Pass,
        }
    }
}

/// An optical surface in world space: a disc of `radius` around `origin`, normal `normal`.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub node: NodeId,
    pub port: String,
    pub kind: PortKind,
    pub origin: DVec3,
    pub normal: DVec3,
    pub radius: f64,
    pub focal_length: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub node: NodeId,
    pub origin: DVec3,
    pub dir: DVec3,
    pub wavelength_nm: f64,
    /// 1/e² radius at the exit, where the beam has its waist.
    pub waist_mm: f64,
    pub m2: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub source: NodeId,
    pub start: DVec3,
    pub end: DVec3,
    /// 1/e² radius at `start` and `end`.
    pub w_start: f64,
    pub w_end: f64,
    /// Waist position and radius when the waist lies inside the segment.
    pub waist: Option<(DVec3, f64)>,
    /// The port the segment ends on; `None` when it runs out into free space.
    pub hit: Option<(NodeId, String)>,
    /// 0 for the main beam, +1 per splitter reflection or transmission.
    pub depth: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub segments: Vec<Segment>,
}

impl Trace {
    /// 1/e² radius where the beam first hits `node`.
    pub fn radius_at(&self, node: NodeId) -> Option<f64> {
        self.segments.iter().find(|s| s.hit.as_ref().is_some_and(|(n, _)| *n == node)).map(|s| s.w_end)
    }
}

/// Gaussian q-parameter `q = z + i·zR` (mm) with the beam's λ·M² (mm).
#[derive(Clone, Copy, Debug)]
struct Beam {
    re: f64,
    im: f64,
    lm2: f64,
}

impl Beam {
    fn w(&self) -> f64 {
        // w = sqrt(-λM² / (π Im(1/q))), Im(1/q) = -im / |q|².
        (self.lm2 * (self.re * self.re + self.im * self.im) / (std::f64::consts::PI * self.im)).sqrt()
    }

    fn propagate(self, d: f64) -> Self {
        Beam { re: self.re + d, ..self }
    }

    /// Thin lens: 1/q' = 1/q - 1/f.
    fn lens(self, f: f64) -> Self {
        let n = self.re * self.re + self.im * self.im;
        let (a, b) = (self.re / n - 1.0 / f, -self.im / n);
        let m = a * a + b * b;
        Beam { re: a / m, im: -b / m, ..self }
    }
}

struct Ray {
    origin: DVec3,
    dir: DVec3,
    beam: Beam,
    /// Index of the port the ray left from.
    from: Option<usize>,
    depth: u32,
}

/// Traces every source through `ports`.
pub fn trace(sources: &[Source], ports: &[Port]) -> Trace {
    let mut segments = Vec::new();
    for s in sources {
        let dir = s.dir.normalize_or_zero();
        if dir == DVec3::ZERO || s.waist_mm <= 0.0 || s.wavelength_nm <= 0.0 {
            continue;
        }
        let lm2 = s.wavelength_nm * 1e-6 * s.m2.max(1.0);
        let zr = std::f64::consts::PI * s.waist_mm * s.waist_mm / lm2;
        let from = ports.iter().position(|p| p.node == s.node && p.kind == PortKind::Source);
        let mut stack = vec![Ray { origin: s.origin, dir, beam: Beam { re: 0.0, im: zr, lm2 }, from, depth: 0 }];
        let mut count = 0;
        while let Some(ray) = stack.pop() {
            if count == MAX_SEGMENTS {
                break;
            }
            count += 1;
            let hit = nearest_hit(&ray, ports);
            let d = hit.map_or(MAX_SEGMENT_LENGTH, |(_, t)| t);
            let end = ray.origin + ray.dir * d;
            let out = ray.beam.propagate(d);
            let waist = (ray.beam.re < 0.0 && -ray.beam.re < d).then(|| {
                let z = -ray.beam.re;
                (ray.origin + ray.dir * z, Beam { re: 0.0, ..ray.beam }.w())
            });
            segments.push(Segment {
                source: s.node,
                start: ray.origin,
                end,
                w_start: ray.beam.w(),
                w_end: out.w(),
                waist,
                hit: hit.map(|(i, _)| (ports[i].node, ports[i].port.clone())),
                depth: ray.depth,
            });
            let Some((i, _)) = hit else { continue };
            let p = &ports[i];
            let next = |dir: DVec3, beam: Beam, depth: u32| Ray { origin: end, dir, beam, from: Some(i), depth };
            let reflected = ray.dir - 2.0 * ray.dir.dot(p.normal) * p.normal;
            match p.kind {
                PortKind::Source | PortKind::Stop => {}
                PortKind::Pass => stack.push(next(ray.dir, out, ray.depth)),
                PortKind::Mirror => stack.push(next(reflected, out, ray.depth)),
                PortKind::Lens => {
                    let f = p.focal_length.unwrap_or(DEFAULT_FOCAL_LENGTH);
                    stack.push(next(ray.dir, out.lens(f), ray.depth));
                }
                PortKind::Splitter => {
                    // Pushed last = traced first: the transmitted beam before the reflected one.
                    stack.push(next(reflected, out, ray.depth + 1));
                    stack.push(next(ray.dir, out, ray.depth + 1));
                }
            }
        }
    }
    Trace { segments }
}

/// Nearest port plane the ray crosses inside the port's radius: (port index, distance).
fn nearest_hit(ray: &Ray, ports: &[Port]) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in ports.iter().enumerate() {
        if Some(i) == ray.from {
            continue;
        }
        let denom = ray.dir.dot(p.normal);
        if denom.abs() < 1e-12 {
            continue;
        }
        let t = (p.origin - ray.origin).dot(p.normal) / denom;
        if t <= EPS || t > MAX_SEGMENT_LENGTH || best.is_some_and(|(_, b)| b <= t) {
            continue;
        }
        if (ray.origin + ray.dir * t).distance(p.origin) <= p.radius {
            best = Some((i, t));
        }
    }
    best
}

/// World pose of a node: compose node position/rotation up the parent chain, then the platform
/// frame (position, rotation_z, top_height) or the room. Same convention as lab-viewport/src/sync.rs.
pub fn world_pose(doc: &Document, id: NodeId) -> Option<DAffine3> {
    let (position, rotation) = doc.node_pose(id)?;
    let local = DAffine3::from_rotation_translation(rotation, position);
    match doc.root_platform(id) {
        Some(p) => {
            let (origin, rot_z) = doc.platform_pose(p)?;
            Some(DAffine3::from_rotation_translation(DQuat::from_rotation_z(rot_z), origin) * local)
        }
        None => Some(local),
    }
}

fn param(doc_node: &lab_core::Node, key: &str) -> Option<f64> {
    doc_node.params.get(key).and_then(ParamValue::as_f64)
}

/// Sources and ports of every node, in world space. `part` looks a catalog def up by id.
pub fn scene(doc: &Document, part: &dyn Fn(&str) -> Option<PartDef>) -> (Vec<Source>, Vec<Port>) {
    let (mut sources, mut ports) = (Vec::new(), Vec::new());
    for node in &doc.nodes {
        if !matches!(node.shape, Shape::Part) {
            continue;
        }
        let Some(def) = part(&node.part).filter(|d| !d.optical_ports.is_empty()) else { continue };
        let Some(pose) = world_pose(doc, node.id) else { continue };
        let focal_length = def.meta.get("focal_length_mm").and_then(|f| f.trim().parse::<f64>().ok());
        let beam_on = !matches!(node.params.get("beam_on"), Some(ParamValue::Flag(false)));
        for op in &def.optical_ports {
            let kind = PortKind::from_kind(&op.kind);
            let origin = pose.transform_point3(op.frame.origin);
            let normal = pose.transform_vector3(op.frame.z).normalize_or_zero();
            if kind == PortKind::Source && beam_on {
                sources.push(Source {
                    node: node.id,
                    origin,
                    dir: normal,
                    wavelength_nm: param(node, "beam_wavelength_nm").unwrap_or(780.0),
                    waist_mm: param(node, "beam_waist_mm").unwrap_or(0.5),
                    m2: param(node, "beam_m2").unwrap_or(1.0),
                });
            }
            ports.push(Port {
                node: node.id,
                port: op.name.clone(),
                kind,
                origin,
                normal,
                radius: op.clear_aperture.map_or(DEFAULT_RADIUS, |a| a / 2.0),
                focal_length: (kind == PortKind::Lens).then_some(focal_length.unwrap_or(DEFAULT_FOCAL_LENGTH)),
            });
        }
    }
    (sources, ports)
}

/// [`scene`] + [`trace`].
pub fn trace_document(doc: &Document, part: &dyn Fn(&str) -> Option<PartDef>) -> Trace {
    let (sources, ports) = scene(doc, part);
    trace(&sources, &ports)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    use glam::DVec2;
    use lab_core::part::{Frame, Geometry, OpticalPort};
    use lab_core::{Attach, Finish, Node, Outline, Parent, Platform, PlatformId, PlatformKind, Support, Surface};

    use super::*;

    fn source() -> Source {
        Source { node: NodeId(1), origin: DVec3::ZERO, dir: DVec3::Y, wavelength_nm: 780.0, waist_mm: 0.5, m2: 1.0 }
    }

    fn port(node: u64, kind: PortKind, origin: DVec3, normal: DVec3) -> Port {
        Port {
            node: NodeId(node),
            port: "p".into(),
            kind,
            origin,
            normal: normal.normalize(),
            radius: 12.7,
            focal_length: (kind == PortKind::Lens).then_some(100.0),
        }
    }

    #[test]
    fn mirror_at_45_degrees_turns_the_beam_by_90() {
        let m = port(2, PortKind::Mirror, DVec3::new(0.0, 200.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[m]);
        assert_eq!(t.segments.len(), 2);
        let (a, b) = (&t.segments[0], &t.segments[1]);
        assert_eq!(a.hit, Some((NodeId(2), "p".into())));
        assert!(a.end.distance(DVec3::new(0.0, 200.0, 0.0)) < 1e-9);
        let (da, db) = ((a.end - a.start).normalize(), (b.end - b.start).normalize());
        assert!(da.dot(db).abs() < 1e-9, "{da} {db}");
        assert!(db.distance(DVec3::X) < 1e-9, "{db}");
        assert_eq!(b.hit, None);
        assert!((b.end - b.start).length() - MAX_SEGMENT_LENGTH < 1e-9);
        assert_eq!(t.radius_at(NodeId(2)), Some(a.w_end));
    }

    #[test]
    fn lens_focuses_at_its_focal_length() {
        let lens = port(2, PortKind::Lens, DVec3::new(0.0, 10.0, 0.0), DVec3::Y);
        let t = trace(&[source()], &[lens]);
        assert_eq!(t.segments.len(), 2);
        let (waist, w0) = t.segments[1].waist.expect("waist after the lens");
        let z = waist.y - 10.0;
        assert!((z - 100.0).abs() < 2.0, "waist {z} mm after the lens");
        let expected = 780e-6 * 100.0 / (PI * 0.5);
        assert!((w0 / expected - 1.0).abs() < 0.05, "w0 {w0}, expected {expected}");
        assert!(t.segments[0].waist.is_none());
        assert!((t.segments[0].w_start - 0.5).abs() < 1e-12);
    }

    #[test]
    fn mirror_outside_its_aperture_is_missed() {
        let m = port(2, PortKind::Mirror, DVec3::new(20.0, 200.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[m]);
        assert_eq!(t.segments.len(), 1);
        assert_eq!(t.segments[0].hit, None);
        assert_eq!(t.radius_at(NodeId(2)), None);
    }

    #[test]
    fn splitter_emits_two_beams_and_bounces_are_capped() {
        let bs = port(2, PortKind::Splitter, DVec3::new(0.0, 100.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[bs]);
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments.iter().filter(|s| s.depth == 1).count(), 2);
        // Two facing mirrors trap the beam: the trace stops at MAX_SEGMENTS.
        let a = port(2, PortKind::Mirror, DVec3::new(0.0, 100.0, 0.0), DVec3::Y);
        let b = port(3, PortKind::Mirror, DVec3::new(0.0, -100.0, 0.0), DVec3::Y);
        assert_eq!(trace(&[source()], &[a, b]).segments.len(), MAX_SEGMENTS);
    }

    fn part_node(id: u64, parent: Parent, position: DVec3, rotation: DQuat) -> Node {
        Node {
            note: String::new(),
            id: NodeId(id),
            name: format!("n{id}"),
            part: "test:laser".into(),
            parent,
            attach: Attach::Free,
            position,
            rotation,
            shape: Shape::Part,
            finish: Finish::AnodizedBlack,
            params: Default::default(),
            pickable: true,
            locked: false,
        }
    }

    #[test]
    fn world_pose_of_a_mated_child_on_a_rotated_platform() {
        let mut doc = Document::new("t");
        doc.platforms.push(Platform {
            id: PlatformId(1),
            name: "Table".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 210.0,
            top_height: 800.0,
            position: DVec2::new(1000.0, 500.0),
            rotation_z: FRAC_PI_2,
            hole_grid: None,
            support: Support::Fixed,
        });
        doc.nodes.push(part_node(2, Parent::Platform(PlatformId(1)), DVec3::new(100.0, 0.0, 0.0), DQuat::IDENTITY));
        let mut child = part_node(3, Parent::Node(NodeId(2)), DVec3::new(0.0, 0.0, 50.0), DQuat::from_rotation_z(FRAC_PI_4));
        child.attach = Attach::Mate { self_conn: "a".into(), parent_conn: "b".into(), slide: 0.0, twist: FRAC_PI_4 };
        doc.nodes.push(child);
        // Platform +X points to room +Y: the base sits at (1000, 600, 800), the child 50 mm above.
        let pose = world_pose(&doc, NodeId(3)).unwrap();
        assert!(pose.translation.distance(DVec3::new(1000.0, 600.0, 850.0)) < 1e-9, "{}", pose.translation);
        // Child +X: rotated 45° in the platform frame, then 90° by the platform: 135° in the room.
        let x = pose.transform_vector3(DVec3::X);
        assert!(x.distance(DVec3::new((3.0 * FRAC_PI_4).cos(), (3.0 * FRAC_PI_4).sin(), 0.0)) < 1e-9, "{x}");

        // scene(): the laser's output port follows the same pose and honours beam_on.
        let def = PartDef {
            id: "test:laser".into(),
            version: 1,
            vendor: String::new(),
            part_number: String::new(),
            name: "Laser".into(),
            category: "lasers/heads".into(),
            tags: vec![],
            geometry: Geometry::Parametric { generator: "laser_head".into(), params: Default::default() },
            connectors: vec![],
            optical_ports: vec![OpticalPort {
                name: "output".into(),
                kind: "source".into(),
                clear_aperture: Some(3.0),
                frame: Frame { origin: DVec3::new(0.0, 100.0, 40.0), z: DVec3::Y, x: DVec3::X },
            }],
            meta: Default::default(),
        };
        doc.nodes.truncate(1);
        let lookup = |id: &str| (id == "test:laser").then(|| def.clone());
        let (sources, ports) = scene(&doc, &lookup);
        assert_eq!((sources.len(), ports.len()), (1, 1));
        assert!(sources[0].origin.distance(DVec3::new(900.0, 600.0, 840.0)) < 1e-9, "{}", sources[0].origin);
        assert!(sources[0].dir.distance(-DVec3::X) < 1e-9);
        assert_eq!(trace_document(&doc, &lookup).segments.len(), 1);
        doc.nodes[0].params.insert("beam_on".into(), ParamValue::Flag(false));
        assert!(trace_document(&doc, &lookup).segments.is_empty());
    }
}
