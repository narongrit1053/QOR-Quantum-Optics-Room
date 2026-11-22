//! Beam trace and Gaussian beam size (docs/M6-PLAN.md section A, DESIGN.md §10 "Beam paths").
//!
//! Sequential ray trace of the beam axis through the optical ports of the placed parts, with a
//! Gaussian q-parameter carried per segment (ABCD, 1/e² radius `w`). Units: mm, radians, room
//! frame (+Z up). Pure and deterministic: the viewport and the app call the same functions.

pub mod pol;

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI};

use glam::{DAffine3, DQuat, DVec3};
use lab_core::part::{ParamValue, PartDef};
use lab_core::{Document, NodeId, Shape};
pub use pol::{field_from, horizontal, isolator, mirror, pbs, plate_axis, polarization_of, waveplate, Field, Polarization};

/// Port radius when a part gives no clear aperture.
pub const DEFAULT_RADIUS: f64 = 12.7;
/// Lens focal length when a lens gives no `focal_length_mm`.
pub const DEFAULT_FOCAL_LENGTH: f64 = 100.0;
/// Most segments traced per source; then the trace stops.
pub const MAX_SEGMENTS: usize = 64;
/// Longest segment: a beam that hits nothing ends here.
pub const MAX_SEGMENT_LENGTH: f64 = 5000.0;
/// Rays weaker than this (fraction of source power) are dropped.
pub const MIN_POWER: f64 = 1e-4;
const EPS: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    Source,
    Mirror,
    Lens,
    /// 50/50 non-polarizing beam splitter.
    Bs,
    /// Polarizing beam splitter: p transmits, s reflects.
    Pbs,
    Hwp,
    Qwp,
    /// Passes like `Pass` until it gets its own physics.
    Isolator,
    /// Iris, legacy waveplate, anything unknown: the beam passes unchanged.
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
            "splitter" | "bs" => PortKind::Bs,
            "pbs" => PortKind::Pbs,
            "hwp" => PortKind::Hwp,
            "qwp" => PortKind::Qwp,
            "isolator" => PortKind::Isolator,
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
    /// Plate or isolator axis angle from the lab horizontal, degrees.
    pub angle_deg: f64,
    /// Horizontal fallback where the port normal is vertical.
    pub frame_x: DVec3,
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
    pub pol: Polarization,
    /// Horizontal for a vertical beam, where `Z x dir` is undefined.
    pub frame_x: DVec3,
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
    pub field: Field,
    /// `field.power()`.
    pub power: f64,
    /// Gaussian q at `start` as (Re, Im), and lam*M2 (mm).
    pub q_start: (f64, f64),
    pub lm2: f64,
    /// Horizontal fallback for this ray, carried from the source.
    pub frame_x: DVec3,
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
    field: Field,
    frame_x: DVec3,
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
        let field = field_from(s.pol, dir, horizontal(dir, s.frame_x));
        let beam = Beam { re: 0.0, im: zr, lm2 };
        let mut stack = vec![Ray { origin: s.origin, dir, beam, field, frame_x: s.frame_x, from, depth: 0 }];
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
                field: ray.field,
                power: ray.field.power(),
                q_start: (ray.beam.re, ray.beam.im),
                lm2: ray.beam.lm2,
                frame_x: ray.frame_x,
            });
            let Some((i, _)) = hit else { continue };
            let p = &ports[i];
            let next = |dir: DVec3, beam: Beam, depth: u32| Ray {
                origin: end,
                dir,
                beam,
                field: ray.field,
                frame_x: ray.frame_x,
                from: Some(i),
                depth,
            };
            let reflected = ray.dir - 2.0 * ray.dir.dot(p.normal) * p.normal;
            let mut push = |r: Ray| {
                if r.field.power() >= MIN_POWER {
                    stack.push(r);
                }
            };
            match p.kind {
                PortKind::Source | PortKind::Stop => {}
                PortKind::Pass => push(next(ray.dir, out, ray.depth)),
                PortKind::Isolator => {
                    let field = isolator(ray.field, ray.dir, p.normal, p.frame_x, p.angle_deg);
                    push(Ray { field, ..next(ray.dir, out, ray.depth) });
                }
                PortKind::Mirror => push(Ray { field: mirror(ray.field, p.normal), ..next(reflected, out, ray.depth) }),
                PortKind::Lens => {
                    let f = p.focal_length.unwrap_or(DEFAULT_FOCAL_LENGTH);
                    push(next(ray.dir, out.lens(f), ray.depth));
                }
                PortKind::Hwp | PortKind::Qwp => {
                    let gamma = if p.kind == PortKind::Hwp { PI } else { FRAC_PI_2 };
                    let fast = plate_axis(p.normal, p.frame_x, p.angle_deg, ray.dir);
                    push(Ray { field: waveplate(ray.field, ray.dir, fast, gamma), ..next(ray.dir, out, ray.depth) });
                }
                // Pushed last = traced first: the transmitted beam before the reflected one.
                PortKind::Bs => {
                    let f = ray.field.scale(FRAC_1_SQRT_2);
                    push(Ray { field: mirror(f, p.normal), ..next(reflected, out, ray.depth + 1) });
                    push(Ray { field: f, ..next(ray.dir, out, ray.depth + 1) });
                }
                PortKind::Pbs => {
                    let (t, r) = pbs(ray.field, ray.dir, p.normal);
                    push(Ray { field: mirror(r, p.normal), ..next(reflected, out, ray.depth + 1) });
                    push(Ray { field: t, ..next(ray.dir, out, ray.depth + 1) });
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
                    pol: Polarization {
                        azimuth_deg: param(node, "beam_pol_angle_deg").unwrap_or(0.0),
                        ellipticity_deg: param(node, "beam_pol_ellipticity_deg").unwrap_or(0.0),
                    },
                    frame_x: pose.transform_vector3(op.frame.x),
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
                angle_deg: match kind {
                    PortKind::Hwp | PortKind::Qwp => param(node, "wp_angle_deg"),
                    PortKind::Isolator => param(node, "iso_angle_deg"),
                    _ => None,
                }
                .unwrap_or(0.0),
                frame_x: pose.transform_vector3(op.frame.x),
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
        Source { node: NodeId(1), origin: DVec3::ZERO, dir: DVec3::Y, wavelength_nm: 780.0, waist_mm: 0.5, m2: 1.0, pol: Polarization::default(), frame_x: DVec3::X }
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
            angle_deg: 0.0,
            frame_x: DVec3::X,
        }
    }

    fn plate(node: u64, kind: PortKind, y: f64, angle_deg: f64) -> Port {
        Port { angle_deg, ..port(node, kind, DVec3::new(0.0, y, 0.0), DVec3::Y) }
    }

    fn pbs_port(node: u64, y: f64) -> Port {
        port(node, PortKind::Pbs, DVec3::new(0.0, y, 0.0), DVec3::new(0.7071, 0.7071, 0.0))
    }

    fn src_pol(azimuth_deg: f64) -> Source {
        Source { pol: Polarization { azimuth_deg, ellipticity_deg: 0.0 }, ..source() }
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
        let bs = port(2, PortKind::Bs, DVec3::new(0.0, 100.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[bs]);
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments.iter().filter(|s| s.depth == 1).count(), 2);
        // Two facing mirrors trap the beam: the trace stops at MAX_SEGMENTS.
        let a = port(2, PortKind::Mirror, DVec3::new(0.0, 100.0, 0.0), DVec3::Y);
        let b = port(3, PortKind::Mirror, DVec3::new(0.0, -100.0, 0.0), DVec3::Y);
        assert_eq!(trace(&[source()], &[a, b]).segments.len(), MAX_SEGMENTS);
    }

    fn powers_after(t: &Trace, from: usize) -> Vec<f64> {
        t.segments[from..].iter().map(|s| s.power).collect()
    }

    #[test]
    fn pbs_sorts_h_and_v() {
        let t = trace(&[source()], &[pbs_port(2, 100.0)]);
        assert_eq!(t.segments.len(), 2);
        assert!((t.segments[1].power - 1.0).abs() < 1e-9);
        assert!((t.segments[1].end - t.segments[1].start).normalize().distance(DVec3::Y) < 1e-9);
        let t = trace(&[src_pol(90.0)], &[pbs_port(2, 100.0)]);
        assert_eq!(t.segments.len(), 2);
        assert!((t.segments[1].power - 1.0).abs() < 1e-9);
        let d = (t.segments[1].end - t.segments[1].start).normalize();
        assert!(d.distance(-DVec3::X) < 1e-3, "{d}");
    }

    #[test]
    fn hwp_22_5_gives_50_50() {
        let t = trace(&[source()], &[plate(2, PortKind::Hwp, 50.0, 22.5), pbs_port(3, 100.0)]);
        assert_eq!(t.segments.len(), 4);
        assert_eq!(powers_after(&t, 2).iter().filter(|p| (*p - 0.5).abs() < 1e-9).count(), 2);
        assert!((state(&t.segments[1]).azimuth_deg - 45.0).abs() < 1e-9);
        let t = trace(&[source()], &[plate(2, PortKind::Hwp, 50.0, -22.5)]);
        assert!((state(&t.segments[1]).azimuth_deg - 135.0).abs() < 1e-9);
    }

    #[test]
    fn qwp_45_anchor_is_right_circular() {
        let t = trace(&[source()], &[plate(2, PortKind::Qwp, 50.0, 45.0), pbs_port(3, 100.0)]);
        let p = state(&t.segments[1]);
        assert!((p.ellipticity_deg - 45.0).abs() < 1e-9, "{p:?}");
        assert_eq!(p.label(), "Circular R");
        assert_eq!(t.segments.len(), 4);
        assert!(powers_after(&t, 2).iter().all(|p| (p - 0.5).abs() < 1e-9));
    }

    #[test]
    fn two_qwp_make_a_hwp() {
        let t = trace(&[source()], &[plate(2, PortKind::Qwp, 50.0, 45.0), plate(3, PortKind::Qwp, 100.0, 45.0)]);
        let p = state(&t.segments[2]);
        assert!((p.azimuth_deg - 90.0).abs() < 1e-6 && p.ellipticity_deg.abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn bs_splits_50_50_and_conserves_power() {
        let bs = port(2, PortKind::Bs, DVec3::new(0.0, 100.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[src_pol(30.0)], &[bs]);
        assert_eq!(t.segments.len(), 3);
        assert!(powers_after(&t, 1).iter().all(|p| (p - 0.5).abs() < 1e-12));
        assert!((t.segments[1].power + t.segments[2].power - t.segments[0].power).abs() < 1e-12);
    }

    #[test]
    fn mirror_flips_circular_handedness() {
        let m = port(2, PortKind::Mirror, DVec3::new(0.0, 200.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let s = Source { pol: Polarization { azimuth_deg: 0.0, ellipticity_deg: 45.0 }, ..source() };
        let t = trace(&[s], &[m]);
        assert!((state(&t.segments[0]).ellipticity_deg - 45.0).abs() < 1e-9);
        assert!((state(&t.segments[1]).ellipticity_deg + 45.0).abs() < 1e-9);
    }

    #[test]
    fn double_pass_isolates() {
        let t = trace(
            &[source()],
            &[plate(2, PortKind::Qwp, 50.0, 45.0), plate(3, PortKind::Mirror, 100.0, 0.0), pbs_port(4, -50.0)],
        );
        assert_eq!(t.segments.len(), 5);
        let last = t.segments.last().unwrap();
        let d = (last.end - last.start).normalize();
        assert!(d.distance(DVec3::X) < 1e-3, "{d}");
        assert!((last.power - 1.0).abs() < 1e-9);
        assert!((state(last).azimuth_deg - 90.0).abs() < 1e-6);
    }

    #[test]
    fn legacy_kinds_trace_as_before() {
        let bs = port(2, PortKind::from_kind("splitter"), DVec3::new(0.0, 100.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[bs]);
        assert_eq!((t.segments.len(), t.segments.iter().filter(|s| s.depth == 1).count()), (3, 2));
        let wp = port(2, PortKind::from_kind("waveplate"), DVec3::new(0.0, 100.0, 0.0), DVec3::Y);
        let t = trace(&[source()], &[wp]);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].field, t.segments[1].field);
    }

    #[test]
    fn waveplate_back_side_and_oblique_conserve_power() {
        let s = src_pol(30.0);
        for normal in [-DVec3::Y, DVec3::new(0.5, 0.866, 0.0)] {
            let q = Port { angle_deg: 20.0, ..port(2, PortKind::Qwp, DVec3::new(0.0, 100.0, 0.0), normal) };
            let t = trace(&[s.clone()], &[q]);
            assert_eq!(t.segments.len(), 2);
            let f = t.segments[1].field;
            assert!(f.re.is_finite() && f.im.is_finite());
            assert!((t.segments[1].power - 1.0).abs() < 1e-12, "{}", t.segments[1].power);
        }
    }

    fn part_node(id: u64, parent: Parent, position: DVec3, rotation: DQuat) -> Node {
        Node { select_lock: false,
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
        doc.platforms.push(Platform { select_lock: false,
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
            features: Vec::new(),
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

    fn iso(node: u64, y: f64, normal: DVec3) -> Port {
        port(node, PortKind::Isolator, DVec3::new(0.0, y, 0.0), normal)
    }

    #[test]
    fn isolator_forward_passes_and_rotates() {
        let t = trace(&[source()], &[iso(2, 50.0, DVec3::Y)]);
        assert_eq!(t.segments.len(), 2);
        assert!((t.segments[1].power - 1.0).abs() < 1e-9);
        let p = state(&t.segments[1]);
        assert!((p.azimuth_deg - 45.0).abs() < 1e-6 && p.ellipticity_deg.abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn isolator_forward_malus() {
        let t = trace(&[src_pol(45.0)], &[iso(2, 50.0, DVec3::Y)]);
        assert_eq!(t.segments.len(), 2);
        assert!((t.segments[1].power - 0.5).abs() < 1e-9, "{}", t.segments[1].power);
    }

    #[test]
    fn isolator_blocks_the_return() {
        let t = trace(&[source()], &[iso(2, 50.0, DVec3::Y), port(3, PortKind::Mirror, DVec3::new(0.0, 100.0, 0.0), DVec3::Y)]);
        // source -> isolator -> mirror -> isolator (blocked): nothing leaves the input side.
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[2].hit, Some((NodeId(2), "p".into())));
    }

    #[test]
    fn isolator_blocks_any_backward_input() {
        let h = horizontal(-DVec3::Y, DVec3::X);
        for (a, e) in [(0.0, 0.0), (90.0, 0.0), (45.0, 0.0), (0.0, 45.0)] {
            let f = field_from(Polarization { azimuth_deg: a, ellipticity_deg: e }, -DVec3::Y, h);
            let out = isolator(f, -DVec3::Y, DVec3::Y, DVec3::X, 0.0);
            assert!(out.power() < MIN_POWER, "{a} {e}: {}", out.power());
        }
        let s = Source { origin: DVec3::new(0.0, 100.0, 0.0), dir: -DVec3::Y, ..source() };
        assert_eq!(trace(&[s], &[iso(2, 50.0, DVec3::Y)]).segments.len(), 1);
    }

    #[test]
    fn reversed_isolator_blocks_forward() {
        assert_eq!(trace(&[source()], &[iso(2, 50.0, -DVec3::Y)]).segments.len(), 1);
    }

    #[test]
    fn scene_reads_waveplate_and_isolator_angles() {
        let def = |id: &str, kind: &str| PartDef {
            id: id.into(),
            version: 1,
            vendor: String::new(),
            part_number: String::new(),
            name: id.into(),
            category: "optics".into(),
            tags: vec![],
            geometry: Geometry::Parametric { generator: "x".into(), params: Default::default() },
            connectors: vec![],
            optical_ports: vec![OpticalPort {
                name: "p".into(),
                kind: kind.into(),
                clear_aperture: None,
                frame: Frame { origin: DVec3::ZERO, z: DVec3::Y, x: DVec3::X },
            }],
            meta: Default::default(),
            features: Vec::new(),
        };
        let mut doc = Document::new("t");
        for (id, part, key, v) in [(1, "t:hwp", "wp_angle_deg", 22.5), (2, "t:iso", "iso_angle_deg", 30.0)] {
            let mut n = part_node(id, Parent::Room, DVec3::ZERO, DQuat::IDENTITY);
            n.part = part.into();
            n.params.insert(key.into(), ParamValue::Number(v));
            doc.nodes.push(n);
        }
        let lookup = |id: &str| match id {
            "t:hwp" => Some(def(id, "hwp")),
            "t:iso" => Some(def(id, "isolator")),
            _ => None,
        };
        let (_, ports) = scene(&doc, &lookup);
        assert_eq!(ports.iter().map(|p| (p.kind, p.angle_deg)).collect::<Vec<_>>(), [(PortKind::Hwp, 22.5), (PortKind::Isolator, 30.0)]);
    }

    fn state(s: &Segment) -> Polarization {
        let d = (s.end - s.start).normalize();
        polarization_of(&s.field, d, horizontal(d, s.frame_x))
    }

    #[test]
    fn source_state_round_trips() {
        for (a, e) in [(0.0, 0.0), (90.0, 0.0), (45.0, 0.0), (30.0, 20.0), (0.0, 45.0), (0.0, -45.0)] {
            let h = horizontal(DVec3::Y, DVec3::X);
            let p = polarization_of(&field_from(Polarization { azimuth_deg: a, ellipticity_deg: e }, DVec3::Y, h), DVec3::Y, h);
            assert!((p.ellipticity_deg - e).abs() < 1e-9);
            if e.abs() < 44.9 {
                assert!((p.azimuth_deg - a).abs() < 1e-9, "{a} {e} -> {p:?}");
            }
        }
    }

    #[test]
    fn labels() {
        assert_eq!(Polarization { azimuth_deg: 45.0, ellipticity_deg: 0.0 }.label(), "Linear 45°");
        assert_eq!(Polarization { azimuth_deg: 0.0, ellipticity_deg: 45.0 }.label(), "Circular R");
        assert_eq!(Polarization { azimuth_deg: 0.0, ellipticity_deg: -44.8 }.label(), "Circular L");
        assert_eq!(Polarization { azimuth_deg: 30.0, ellipticity_deg: 12.0 }.label(), "Elliptical 30°, 12°");
    }

    #[test]
    fn fold_mirror_keeps_horizontal() {
        let m = port(2, PortKind::Mirror, DVec3::new(0.0, 200.0, 0.0), DVec3::new(1.0, -1.0, 0.0));
        let t = trace(&[source()], &[m]);
        let p = state(&t.segments[1]);
        assert!(p.azimuth_deg < 1e-6 || p.azimuth_deg > 180.0 - 1e-6, "{p:?}");
        assert!(p.ellipticity_deg.abs() < 1e-6);
        assert!((t.segments[1].power - 1.0).abs() < 1e-12);
    }

    fn periscope() -> Trace {
        let lower = port(2, PortKind::Mirror, DVec3::new(0.0, 100.0, 0.0), DVec3::new(0.0, -0.7071, 0.7071));
        let upper = port(3, PortKind::Mirror, DVec3::new(0.0, 100.0, 90.0), DVec3::new(-1.0, 0.0, 1.0));
        trace(&[source()], &[lower, upper])
    }

    #[test]
    fn periscope_rotates_horizontal_to_vertical() {
        let t = periscope();
        assert_eq!(t.segments.len(), 3);
        let p = state(&t.segments[2]);
        assert!((p.azimuth_deg - 90.0).abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn vertical_beam_has_finite_field() {
        let s = &periscope().segments[1];
        assert!((s.end - s.start).normalize().distance(DVec3::Z) < 1e-9);
        let p = state(s);
        assert!(s.field.re.is_finite() && s.field.im.is_finite());
        assert!(p.azimuth_deg.is_finite() && p.ellipticity_deg.is_finite());
    }
}
