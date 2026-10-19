//! Parametric part generators (DESIGN.md §5, docs/M2-PLAN.md POLA-37). Every part is built from
//! boxes and cylinders in the part-local frame: millimetres, +Z up, origin at the bottom centre
//! (optics: their centre), optical axis along +Y. Meshes are cached by generator and parameters.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use glam::{DVec2, DVec3};
use lab_core::Finish;
use lab_core::part::{ParamValue, Params};
use lab_geom::{TriMesh, circle_points};

pub struct PartMesh {
    pub pieces: Vec<(Finish, TriMesh)>,
    pub min: DVec3,
    pub max: DVec3,
}

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("Unknown part generator \"{0}\".")]
    Unknown(String),
    #[error("Parameter {name} {reason}.")]
    BadParam { name: String, reason: String },
}

pub struct GeneratorInfo {
    pub name: &'static str,
    /// (name, default)
    pub params: &'static [(&'static str, f64)],
}

const GENERATORS: &[GeneratorInfo] = &[
    gi("post", &[("diameter", 12.7), ("length", 50.0)]),
    gi("pedestal_post", &[("diameter", 25.0), ("length", 50.0), ("flange_diameter", 38.0)]),
    gi("post_holder", &[("bore", 12.7), ("height", 50.0), ("base_diameter", 30.0)]),
    gi("base", &[("length", 75.0), ("width", 25.0), ("thickness", 10.0)]),
    gi("clamping_fork", &[("length", 50.0), ("width", 25.0), ("thickness", 10.0)]),
    gi(
        "kinematic_mount",
        &[
            ("optic_diameter", 25.4),
            ("width", 38.1),
            ("height", 38.1),
            ("thickness", 12.7),
            ("axis_height", 20.3),
        ],
    ),
    gi(
        "fixed_mount",
        &[("optic_diameter", 25.4), ("outer_diameter", 38.0), ("thickness", 10.0), ("axis_height", 20.3)],
    ),
    gi(
        "rotation_mount",
        &[("optic_diameter", 25.4), ("outer_diameter", 50.0), ("thickness", 15.0), ("axis_height", 30.0)],
    ),
    gi("periscope", &[("width", 50.0), ("height", 150.0), ("axis_height", 50.0)]),
    gi("mirror", &[("diameter", 25.4), ("thickness", 6.0)]),
    gi("lens", &[("diameter", 25.4), ("thickness", 4.0)]),
    gi("cube", &[("size", 25.4)]),
    gi("waveplate", &[("diameter", 25.4), ("thickness", 2.0)]),
    gi("iris", &[("outer_diameter", 50.0), ("aperture", 12.0), ("thickness", 8.0), ("axis_height", 30.0)]),
    gi("beam_dump", &[("diameter", 25.0), ("length", 40.0), ("axis_height", 25.0)]),
    gi("fiber_coupler", &[("diameter", 25.0), ("length", 30.0), ("axis_height", 25.0)]),
    gi("modulator_box", &[("length", 60.0), ("width", 40.0), ("height", 30.0), ("axis_height", 20.0)]),
    gi("photodiode", &[("length", 40.0), ("width", 20.0), ("height", 50.0), ("axis_height", 25.0)]),
    gi("camera", &[("length", 50.0), ("width", 30.0), ("height", 30.0), ("axis_height", 15.0)]),
    gi("spcm", &[("length", 120.0), ("width", 60.0), ("height", 50.0), ("axis_height", 25.0)]),
    gi("crystal_oven", &[("length", 60.0), ("width", 40.0), ("height", 30.0), ("axis_height", 20.0)]),
    gi("laser_head", &[("length", 200.0), ("width", 80.0), ("height", 60.0), ("axis_height", 40.0)]),
    gi("rack_19in", &[("units", 42.0), ("depth", 800.0)]),
    gi("desk", &[("width", 1500.0), ("depth", 750.0), ("height", 750.0)]),
    gi("shelf", &[("width", 1200.0), ("depth", 400.0), ("thickness", 20.0), ("height", 500.0)]),
    gi("laser_curtain", &[("width", 2000.0), ("height", 2000.0)]),
];

const fn gi(name: &'static str, params: &'static [(&'static str, f64)]) -> GeneratorInfo {
    GeneratorInfo { name, params }
}

pub fn generators() -> &'static [GeneratorInfo] {
    GENERATORS
}

/// Generator name plus every param's (name, value) in sorted order; text and flags included.
type CacheKey = (String, Vec<(String, String)>);

/// Missing params take defaults; non-positive or non-finite sizes are BadParam. Cached by the
/// generator name and the params' sorted (name, value bits); same inputs return the same Arc.
pub fn generate(generator: &str, params: &Params) -> Result<Arc<PartMesh>, GenError> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<PartMesh>>>> = OnceLock::new();
    let key: CacheKey = (
        generator.to_string(),
        params
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    ParamValue::Number(n) => format!("n{:x}", n.to_bits()),
                    ParamValue::Text(t) => format!("t{t}"),
                    ParamValue::Flag(b) => format!("f{b}"),
                };
                (k.clone(), v)
            })
            .collect(),
    );
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(hit.clone());
    }
    let info = GENERATORS
        .iter()
        .find(|g| g.name == generator)
        .ok_or_else(|| GenError::Unknown(generator.to_string()))?;
    let mut p = HashMap::new();
    for &(name, default) in info.params {
        let v = match params.get(name) {
            None => default,
            Some(ParamValue::Number(v)) => *v,
            Some(_) => return Err(bad(name, "must be a number")),
        };
        if !v.is_finite() || v <= 0.0 {
            return Err(bad(name, "must be a positive finite number"));
        }
        p.insert(name, v);
    }
    let mesh = Arc::new(build(generator, &|n| p[n])?);
    Ok(cache.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_insert(mesh).clone())
}

fn bad(name: &str, reason: &str) -> GenError {
    GenError::BadParam { name: name.to_string(), reason: reason.to_string() }
}

const BLACK: Finish = Finish::AnodizedBlack;
const STEEL: Finish = Finish::Stainless;
const GREY: Finish = Finish::Painted { rgb: [150, 150, 155] };

fn build(name: &str, p: &dyn Fn(&str) -> f64) -> Result<PartMesh, GenError> {
    let mut b = Builder::default();
    match name {
        "post" => b.cyl_z(STEEL, 0.0, p("length"), p("diameter") / 2.0),
        "pedestal_post" => {
            let l = p("length");
            b.cyl_z(STEEL, 0.0, (l * 0.1).min(5.0), p("flange_diameter") / 2.0);
            b.cyl_z(STEEL, 0.0, l, p("diameter") / 2.0);
        }
        "post_holder" => {
            let h = p("height");
            b.cyl_z(BLACK, 0.0, (h * 0.2).min(8.0), p("base_diameter") / 2.0);
            b.cyl_z(BLACK, 0.0, h, p("bore") / 2.0 + 3.0);
        }
        "base" | "clamping_fork" => {
            let (l, w) = (p("length"), p("width"));
            b.cuboid(BLACK, DVec3::new(-l / 2.0, -w / 2.0, 0.0), DVec3::new(l / 2.0, w / 2.0, p("thickness")));
        }
        "kinematic_mount" => {
            let (w, t, h, a) = (p("width"), p("thickness"), p("height"), p("axis_height"));
            b.cuboid(BLACK, DVec3::new(-w / 2.0, -t / 2.0, 0.0), DVec3::new(w / 2.0, t / 2.0, h));
            // seat ring marks the optic; never wider than the plate
            let r = (p("optic_diameter") / 2.0 + 1.5).min(w / 2.0);
            b.cyl_y(BLACK, a, t / 2.0, t / 2.0 + 2.0, r);
        }
        "fixed_mount" | "rotation_mount" | "iris" => {
            let (od, t, a) = (p("outer_diameter"), p("thickness"), p("axis_height"));
            ring_on_stem(&mut b, a, od / 2.0, t);
        }
        "periscope" => {
            let (w, h) = (p("width"), p("height"));
            b.cuboid(BLACK, DVec3::new(-w / 2.0, -w / 2.0, 0.0), DVec3::new(w / 2.0, w / 2.0, 10f64.min(h)));
            b.cyl_z(STEEL, 0.0, h, w / 4.0);
        }
        "mirror" => optic(&mut b, Finish::Mirror, p("diameter"), p("thickness")),
        "lens" | "waveplate" => optic(&mut b, Finish::Glass, p("diameter"), p("thickness")),
        "cube" => {
            let s = p("size") / 2.0;
            b.cuboid(Finish::Glass, DVec3::splat(-s), DVec3::splat(s));
        }
        "beam_dump" | "fiber_coupler" => {
            let (r, l, a) = (p("diameter") / 2.0, p("length"), p("axis_height"));
            b.cyl_y(BLACK, a, -l / 2.0, l / 2.0, r);
            stem(&mut b, a - r, r);
        }
        "modulator_box" | "photodiode" | "camera" | "spcm" | "crystal_oven" | "laser_head" => {
            let (l, w, h) = (p("length"), p("width"), p("height"));
            let finish = if name == "laser_head" { GREY } else { BLACK };
            b.cuboid(finish, DVec3::new(-w / 2.0, -l / 2.0, 0.0), DVec3::new(w / 2.0, l / 2.0, h));
        }
        "rack_19in" => {
            let (h, d) = (p("units") * 44.45 + 100.0, p("depth"));
            b.cuboid(GREY, DVec3::new(-300.0, -d / 2.0, 0.0), DVec3::new(300.0, d / 2.0, h));
        }
        "desk" => {
            let (w, d, h) = (p("width"), p("depth"), p("height"));
            let top = 30f64.min(h);
            let wood = Finish::Painted { rgb: [190, 160, 120] };
            b.cuboid(wood, DVec3::new(-w / 2.0, -d / 2.0, h - top), DVec3::new(w / 2.0, d / 2.0, h));
            let leg = 50f64.min(w / 2.0).min(d / 2.0);
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let c = DVec2::new(sx * (w - leg) / 2.0, sy * (d - leg) / 2.0);
                let lo = (c - leg / 2.0).extend(0.0);
                b.cuboid(GREY, lo, (c + leg / 2.0).extend(h - top));
            }
        }
        "shelf" => {
            let (w, d, t, h) = (p("width"), p("depth"), p("thickness"), p("height"));
            let t = t.min(h).min(w / 2.0);
            b.cuboid(GREY, DVec3::new(-w / 2.0, -d / 2.0, h - t), DVec3::new(w / 2.0, d / 2.0, h));
            for sx in [-1.0, 1.0] {
                let x = sx * (w - t) / 2.0;
                b.cuboid(GREY, DVec3::new(x - t / 2.0, -d / 2.0, 0.0), DVec3::new(x + t / 2.0, d / 2.0, h));
            }
        }
        "laser_curtain" => {
            let w = p("width");
            let black = Finish::Painted { rgb: [25, 25, 25] };
            b.cuboid(black, DVec3::new(-w / 2.0, -1.0, 0.0), DVec3::new(w / 2.0, 1.0, p("height")));
        }
        _ => return Err(GenError::Unknown(name.to_string())),
    }
    Ok(b.finish())
}

/// Optic centred on the origin, faces normal to ±Y.
fn optic(b: &mut Builder, finish: Finish, diameter: f64, thickness: f64) {
    b.cyl_y(finish, 0.0, -thickness / 2.0, thickness / 2.0, diameter / 2.0);
}

/// Ring of radius `r` around the optical axis at height `a`, on a stem down to z = 0.
fn ring_on_stem(b: &mut Builder, a: f64, r: f64, t: f64) {
    b.cyl_y(BLACK, a, -t / 2.0, t / 2.0, r);
    stem(b, a - r, r.min(t));
}

/// Square stem from z = 0 to `top` (skipped when the body already reaches the table).
fn stem(b: &mut Builder, top: f64, half: f64) {
    if top > 1e-6 {
        let h = (half / 2.0).max(2.0);
        b.cuboid(STEEL, DVec3::new(-h, -h, 0.0), DVec3::new(h, h, top + 1.0));
    }
}

#[derive(Default)]
struct Builder {
    pieces: Vec<(Finish, TriMesh)>,
}

impl Builder {
    fn add(&mut self, finish: Finish, mesh: TriMesh) {
        match self.pieces.iter_mut().find(|(f, _)| *f == finish) {
            Some((_, m)) => m.append(&mesh),
            None => self.pieces.push((finish, mesh)),
        }
    }

    fn cuboid(&mut self, finish: Finish, lo: DVec3, hi: DVec3) {
        let mut m = TriMesh::default();
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                let n = DVec3::AXES[axis] * sign;
                let (u, v) = (DVec3::AXES[(axis + 1) % 3], DVec3::AXES[(axis + 2) % 3]);
                let corner = |su: f64, sv: f64| {
                    let pick = |d: DVec3, s: f64| if s > 0.0 { hi * d } else { lo * d };
                    pick(n.abs(), sign) + pick(u, su) + pick(v, sv)
                };
                // u × v = +axis, so swap the order on the negative face to keep CCW outward
                let quad = if sign > 0.0 {
                    [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)]
                } else {
                    [corner(-1.0, -1.0), corner(-1.0, 1.0), corner(1.0, 1.0), corner(1.0, -1.0)]
                };
                push_quad(&mut m, quad, n);
            }
        }
        self.add(finish, m);
    }

    /// Cylinder along +Z centred on the Z axis.
    fn cyl_z(&mut self, finish: Finish, z0: f64, z1: f64, r: f64) {
        self.add(finish, cylinder(r, z1 - z0, |p| DVec3::new(p.x, p.y, z0 + p.z)));
    }

    /// Cylinder along +Y through (0, y, z_axis).
    fn cyl_y(&mut self, finish: Finish, z_axis: f64, y0: f64, y1: f64, r: f64) {
        // local (x, y, h) → part (x, h, y) flips handedness; mirror x to keep it right-handed
        self.add(finish, cylinder(r, y1 - y0, |p| DVec3::new(-p.x, y0 + p.z, z_axis + p.y)));
    }

    fn finish(self) -> PartMesh {
        let (mut min, mut max) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for p in self.pieces.iter().flat_map(|(_, m)| &m.positions) {
            min = min.min(*p);
            max = max.max(*p);
        }
        PartMesh { pieces: self.pieces, min, max }
    }
}

/// Closed cylinder of length `len` along local +Z from 0, mapped by the rigid map `to_part`.
fn cylinder(r: f64, len: f64, to_part: impl Fn(DVec3) -> DVec3) -> TriMesh {
    let ring = circle_points(DVec2::ZERO, r, 0.05, 48);
    let origin = to_part(DVec3::ZERO);
    let dir = |v: DVec3| (to_part(v) - origin).normalize();
    let mut m = TriMesh::default();
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let quad = [a.extend(0.0), b.extend(0.0), b.extend(len), a.extend(len)].map(&to_part);
        let base = m.positions.len() as u32;
        for (k, (p, rim)) in quad.iter().zip([a, b, b, a]).enumerate() {
            m.positions.push(*p);
            m.normals.push(dir(rim.extend(0.0)));
            m.uvs.push(DVec2::new((i + k % 2) as f64, (k / 2) as f64));
        }
        m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    for (h, up) in [(0.0, false), (len, true)] {
        let base = m.positions.len() as u32;
        let normal = dir(DVec3::new(0.0, 0.0, if up { 1.0 } else { -1.0 }));
        for p in &ring {
            m.positions.push(to_part(p.extend(h)));
            m.normals.push(normal);
            m.uvs.push(*p);
        }
        for i in 1..n as u32 - 1 {
            let tri = if up { [0, i, i + 1] } else { [0, i + 1, i] };
            m.indices.extend(tri.map(|j| base + j));
        }
    }
    m
}

fn push_quad(m: &mut TriMesh, quad: [DVec3; 4], normal: DVec3) {
    let base = m.positions.len() as u32;
    for (k, p) in quad.into_iter().enumerate() {
        m.positions.push(p);
        m.normals.push(normal);
        m.uvs.push(DVec2::new((k == 1 || k == 2) as u8 as f64, (k >= 2) as u8 as f64));
    }
    m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_with(name: &str, params: &[(&str, f64)]) -> Arc<PartMesh> {
        let params = params.iter().map(|(k, v)| (k.to_string(), ParamValue::Number(*v))).collect();
        generate(name, &params).unwrap()
    }

    fn gen_default(name: &str) -> Arc<PartMesh> {
        gen_with(name, &[])
    }

    fn near(a: DVec3, b: DVec3) {
        assert!((a - b).abs().max_element() < 0.1, "{a} != {b}");
    }

    fn check_meshes(m: &PartMesh) {
        for (_, t) in &m.pieces {
            for tri in t.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|k| t.positions[tri[k] as usize]);
                let geo = (b - a).cross(c - a);
                if geo.length() > 1e-9 {
                    assert!(geo.dot(t.normals[tri[0] as usize]) > 0.0, "inward triangle");
                }
            }
        }
    }

    #[test]
    fn every_generator_builds_with_outward_normals() {
        for g in generators() {
            let m = gen_default(g.name);
            assert!(!m.pieces.is_empty(), "{}", g.name);
            assert!(m.min.z.abs() < 1e-9 || g.name == "mirror" || g.name == "lens"
                || g.name == "waveplate" || g.name == "cube", "{} min.z {}", g.name, m.min.z);
            check_meshes(&m);
        }
    }

    #[test]
    fn posts_holders_bases() {
        let m = gen_with("post", &[("length", 50.0)]);
        near(m.min, DVec3::new(-6.35, -6.35, 0.0));
        near(m.max, DVec3::new(6.35, 6.35, 50.0));
        near(gen_default("pedestal_post").max, DVec3::new(19.0, 19.0, 50.0));
        near(gen_default("post_holder").max, DVec3::new(15.0, 15.0, 50.0));
        near(gen_default("base").max, DVec3::new(37.5, 12.5, 10.0));
        near(gen_default("clamping_fork").min, DVec3::new(-25.0, -12.5, 0.0));
    }

    #[test]
    fn mounts() {
        let m = gen_default("kinematic_mount");
        assert!((m.max.x - 19.05).abs() < 0.1 && (m.max.z - 38.1).abs() < 0.1);
        let m = gen_default("fixed_mount");
        assert!((m.max.z - (20.3 + 19.0)).abs() < 0.1 && (m.max.x - 19.0).abs() < 0.1);
        let m = gen_default("rotation_mount");
        assert!((m.max.z - 55.0).abs() < 0.1 && (m.max.y - 7.5).abs() < 0.1);
        let m = gen_default("iris");
        assert!((m.max.z - 55.0).abs() < 0.1);
        near(gen_default("periscope").max, DVec3::new(25.0, 25.0, 150.0));
    }

    #[test]
    fn optics() {
        let m = gen_default("mirror");
        near(m.min, DVec3::new(-12.7, -3.0, -12.7));
        near(m.max, DVec3::new(12.7, 3.0, 12.7));
        near(gen_default("lens").max, DVec3::new(12.7, 2.0, 12.7));
        near(gen_default("waveplate").max, DVec3::new(12.7, 1.0, 12.7));
        near(gen_default("cube").max, DVec3::splat(12.7));
    }

    #[test]
    fn devices_and_room() {
        let m = gen_default("beam_dump");
        near(m.max, DVec3::new(12.5, 20.0, 37.5));
        assert!(m.min.z.abs() < 1e-9);
        near(gen_default("laser_head").max, DVec3::new(40.0, 100.0, 60.0));
        near(gen_default("spcm").min, DVec3::new(-30.0, -60.0, 0.0));
        let m = gen_default("rack_19in");
        assert!((m.max.y - 400.0).abs() < 0.1 && m.max.z > 42.0 * 44.45);
        near(gen_default("desk").max, DVec3::new(750.0, 375.0, 750.0));
        near(gen_default("shelf").max, DVec3::new(600.0, 200.0, 500.0));
        near(gen_default("laser_curtain").max, DVec3::new(1000.0, 1.0, 2000.0));
    }

    #[test]
    fn cache_and_errors() {
        let a = gen_with("post", &[("length", 75.0)]);
        let b = gen_with("post", &[("length", 75.0)]);
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &gen_with("post", &[("length", 76.0)])));
        assert!(matches!(generate("nope", &Params::new()), Err(GenError::Unknown(_))));
        let bad = Params::from([("length".to_string(), ParamValue::Number(-1.0))]);
        assert!(matches!(generate("post", &bad), Err(GenError::BadParam { .. })));
    }
}
