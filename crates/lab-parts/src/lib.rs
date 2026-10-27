//! Parametric part generators (DESIGN.md §5, docs/M2-PLAN.md POLA-37). Every part is built from
//! boxes, prisms and lathed profiles in the part-local frame: millimetres, +Z up, origin at the bottom centre
//! (optics: their centre), optical axis along +Y. Meshes are cached by generator and parameters.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use glam::{DVec2, DVec3};
use lab_core::Finish;
use lab_core::part::{ParamValue, Params};
use lab_geom::TriMesh;

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
    gi("cel_head", &[("length", 108.0), ("width", 70.0), ("height", 86.7), ("axis_height", 58.0)]),
    gi("riser", &[("length", 100.0), ("width", 75.0), ("height", 25.0)]),
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
        "post" => {
            let (r, l) = (p("diameter") / 2.0, p("length"));
            let c = 0.5f64.min(l / 4.0).min(r / 4.0);
            b.lathe_z(STEEL, &[(0.0, 0.0), (r - c, 0.0), (r, c), (r, l - c), (r - c, l), (0.0, l)]);
        }
        "pedestal_post" => {
            let (r, l, fr) = (p("diameter") / 2.0, p("length"), p("flange_diameter") / 2.0);
            let f = (l * 0.1).min(5.0);
            let c = 0.5f64.min(l / 4.0).min(r / 4.0);
            let step = (fr - 1.0).max(r);
            b.lathe_z(STEEL, &[
                (0.0, 0.0), (fr, 0.0), (fr, f * 0.6), (step, f), (r, f), (r, l - c), (r - c, l), (0.0, l),
            ]);
        }
        "post_holder" => {
            let (h, br, bore) = (p("height"), p("base_diameter") / 2.0, p("bore") / 2.0);
            let ro = bore + 3.0;
            let f = (h * 0.2).min(8.0);
            let floor = (f + 2.0).min(h * 0.9);
            let lip = (br - 1.0).max(ro);
            b.lathe_z(BLACK, &[
                (0.0, 0.0), (br, 0.0), (br, f * 0.7), (lip, f), (ro, f),
                (ro, h), (bore, h), (bore, floor), (0.0, floor),
            ]);
            // spring-loaded thumbscrew on the side, near the top
            let z = (h - 6.0).max(f + 3.0).min(h - 2.0);
            let knob = (br - ro).max(2.0);
            let side = |x0: f64| move |q: DVec3| DVec3::new(x0 + q.z, q.x, z + q.y);
            b.lathe(STEEL, &[(0.0, 0.0), (1.5, 0.0), (1.5, 1.0), (0.0, 1.0)], 12, side(ro - 0.5));
            b.lathe(BLACK, &[(0.0, 0.0), (2.5, 0.0), (2.5, knob - 1.0), (0.0, knob - 1.0)], 16, side(ro + 0.5));
        }
        "base" | "clamping_fork" => {
            let (l, w, t) = (p("length"), p("width"), p("thickness"));
            let s = (w / 4.0).min(3.3); // half the slot width
            let end = (l * 0.12).min(10.0);
            let block = |b: &mut Builder, x0: f64, y0: f64, x1: f64, y1: f64, c: f64| {
                b.prism_z(BLACK, &chamfer_rect(DVec2::new(x0, y0), DVec2::new(x1, y1), c), 0.0, t);
            };
            block(&mut b, -l / 2.0, s, l / 2.0, w / 2.0, 1.0);
            block(&mut b, -l / 2.0, -w / 2.0, l / 2.0, -s, 1.0);
            block(&mut b, -l / 2.0, -s, -l / 2.0 + end, s, 0.0);
            if name == "base" {
                block(&mut b, l / 2.0 - end, -s, l / 2.0, s, 0.0);
            } else {
                // the fork's slot is open at +X; its clamp bolt sits in the slot, flush with the top
                let x = -l / 2.0 + end + 6.0f64.min(l / 4.0);
                let bolt = [(0.0, 0.0), (s + 2.0, 0.0), (s + 2.0, 1.0), (s * 0.8, 1.0), (s * 0.8, 3.0), (0.0, 3.0)];
                b.lathe(STEEL, &bolt, 6, move |q| DVec3::new(x + q.x, q.y, t - 3.0 + q.z));
            }
        }
        "kinematic_mount" => {
            let (w, t, h, a) = (p("width"), p("thickness"), p("height"), p("axis_height"));
            let y0 = -t / 2.0;
            let (lo, hi) = (DVec2::new(-w / 2.0, 0.0), DVec2::new(w / 2.0, h));
            let c = (w.min(h) * 0.08).min(3.0);
            // fixed back plate and the front plate that carries the optic
            b.prism_y(BLACK, &chamfer_rect(lo, hi, c), y0 + 0.3 * t, y0 + 0.6 * t);
            b.prism_y(BLACK, &chamfer_rect(lo + DVec2::new(0.0, 1.0f64.min(h / 4.0)), hi, c), y0 + 0.68 * t, t / 2.0);
            // retaining ring around the optic bore; never wider than the plate
            let ro = (p("optic_diameter") / 2.0 + 1.5).min(w / 2.0);
            b.lathe_y(BLACK, a, &annulus(ro - 1.5, ro, t / 2.0, t / 2.0 + 2.0, 0.0));
            // three adjusters on the back: knobs on screws (springs omitted)
            let k = (w.min(h) * 0.1).clamp(1.5, 4.0);
            for (x, z) in [(-w / 2.0 + k + 1.0, h - k - 1.0), (w / 2.0 - k - 1.0, h - k - 1.0), (w / 2.0 - k - 1.0, k + 1.0)] {
                let at = move |q: DVec3| DVec3::new(x - q.x, q.z, z + q.y);
                let (ya, yb, yc) = (y0 + 0.03 * t, y0 + 0.2 * t, y0 + 0.3 * t);
                b.lathe(STEEL, &[(0.0, yb), (k * 0.35, yb), (k * 0.35, yc), (0.0, yc)], 12, at);
                b.lathe(BLACK, &[(0.0, y0), (k * 0.85, y0), (k, ya), (k, yb), (0.0, yb)], 16, at);
            }
        }
        "fixed_mount" | "rotation_mount" | "iris" => {
            let (r, t, a) = (p("outer_diameter") / 2.0, p("thickness"), p("axis_height"));
            let inner = if name == "iris" { p("aperture") } else { p("optic_diameter") };
            let ri = (inner / 2.0).min(r - 1.0).max(0.1);
            stem(&mut b, a - r, r.min(t));
            match name {
                "fixed_mount" => {
                    b.lathe_y(BLACK, a, &annulus(ri, r, -t / 2.0, t / 2.0, 0.5));
                    // retaining ring inside the bore, at the front
                    b.lathe_y(STEEL, a, &annulus((ri - 1.0).max(ri * 0.8), ri, t / 2.0 - 1.5, t / 2.0 - 0.5, 0.0));
                }
                "rotation_mount" => {
                    b.lathe_y(BLACK, a, &annulus(ri, r, -t / 2.0, 0.0, 0.5));
                    let dial = (r - 1.0).max(ri + 0.5);
                    b.lathe_y(BLACK, a, &annulus(ri, dial, 0.3f64.min(t / 4.0), t / 2.0, 0.3));
                    // engraved graduations every 10°, longer every 30°
                    for i in 0..36 {
                        let (sn, cs) = (i as f64 * std::f64::consts::TAU / 36.0).sin_cos();
                        let len = if i % 3 == 0 { 2.0 } else { 1.0 };
                        let rr = (dial - len).max(ri);
                        let tick = [DVec2::new(-0.2, rr), DVec2::new(0.2, rr), DVec2::new(0.2, dial), DVec2::new(-0.2, dial)]
                            .map(|v| DVec2::new(v.x * cs - v.y * sn, a + v.x * sn + v.y * cs));
                        b.prism_y(STEEL, &tick, t / 2.0 - 0.05, t / 2.0 + 0.05);
                    }
                }
                _ => {
                    let rb = (ri + 3.0).min(r - 0.5);
                    b.lathe_y(BLACK, a, &annulus(rb, r, -t / 2.0, t / 2.0 - 2.0, 0.5));
                    // the leaves close the bore down to `aperture`
                    b.lathe_y(STEEL, a, &annulus(ri, rb, -0.3, 0.3, 0.0));
                    // actuation lever pin on the front face
                    let (sn, cs) = std::f64::consts::FRAC_PI_6.sin_cos();
                    let rl = r - 3.0f64.min(r / 3.0);
                    let (x, z) = (rl * cs, a + rl * sn);
                    let pin = [(0.0, t / 2.0 - 2.0), (1.5, t / 2.0 - 2.0), (1.5, t / 2.0), (0.0, t / 2.0)];
                    b.lathe(STEEL, &pin, 12, move |q| DVec3::new(x - q.x, q.z, z + q.y));
                }
            }
        }
        "periscope" => {
            let (w, h, a) = (p("width"), p("height"), p("axis_height"));
            let base = 10f64.min(h);
            b.prism_z(BLACK, &chamfer_rect(DVec2::splat(-w / 2.0), DVec2::splat(w / 2.0), w * 0.08), 0.0, base);
            b.lathe_z(STEEL, &[(0.0, 0.0), (w / 4.0, 0.0), (w / 4.0, h), (0.0, h)]);
            // two 45° mirror housings, one at the input height and one at the top
            let s = (w / 8.0).min(h / 4.0);
            let y = 3.0 * w / 8.0;
            for z in [a.clamp(base + s, h - s), h - s] {
                let diamond = [DVec2::new(y, z - s), DVec2::new(y + s, z), DVec2::new(y, z + s), DVec2::new(y - s, z)];
                b.prism_x(BLACK, &diamond, -w / 4.0, w / 4.0);
            }
        }
        "mirror" => {
            let (r, t) = (p("diameter") / 2.0, p("thickness"));
            let c = 0.3f64.min(t / 4.0).min(r / 4.0);
            let (y0, y1) = (-t / 2.0, t / 2.0);
            b.lathe_y(Finish::Mirror, 0.0, &[(0.0, y0), (r - c, y0), (r, y0 + c), (r, y1 - c), (r - c, y1), (0.0, y1)]);
        }
        "lens" => {
            // biconvex: the edge is 40 % of the centre thickness
            let (r, t) = (p("diameter") / 2.0, p("thickness"));
            let sag = |x: f64| 0.2 * t + 0.3 * t * (1.0 - x * x);
            let mut prof: Vec<(f64, f64)> = (0..=6).map(|i| (r * i as f64 / 6.0, -sag(i as f64 / 6.0))).collect();
            prof.extend((0..=6).rev().map(|i| (r * i as f64 / 6.0, sag(i as f64 / 6.0))));
            b.lathe_y(Finish::Glass, 0.0, &prof);
        }
        "waveplate" => {
            // the plate in a thin black retaining ring
            let (r, t) = (p("diameter") / 2.0, p("thickness"));
            let rg = (r - 2.0).max(r * 0.7);
            b.lathe_y(Finish::Glass, 0.0, &[(0.0, -t / 4.0), (rg, -t / 4.0), (rg, t / 4.0), (0.0, t / 4.0)]);
            b.lathe_y(BLACK, 0.0, &annulus(rg, r, -t / 2.0, t / 2.0, 0.0));
        }
        "cube" => {
            let s = p("size") / 2.0;
            let c = 0.4f64.min(s / 4.0);
            b.prism_z(Finish::Glass, &chamfer_rect(DVec2::splat(-s), DVec2::splat(s), c), -s, s);
            // the coated diagonal face, seen through the glass
            let (d, e) = (0.95 * s, 0.1);
            let diag = [DVec2::new(-d + e, -d - e), DVec2::new(d + e, d - e), DVec2::new(d - e, d + e), DVec2::new(-d - e, -d + e)];
            b.prism_z(Finish::Mirror, &diag, -d, d);
        }
        "beam_dump" => {
            // finned body, entrance cone open towards -Y (the dump port)
            let (r, l, a) = (p("diameter") / 2.0, p("length"), p("axis_height"));
            let (y0, y1) = (-l / 2.0, l / 2.0);
            let fin = (r * 0.15).min(2.0);
            let mut prof = vec![(0.0, y1), (r - 1.0, y1), (r, y1 - 1.0)];
            let fins = ((l * 0.75 - 2.0) / 4.0).floor().max(0.0) as usize;
            for i in 0..fins {
                let y = y1 - 2.0 - 4.0 * i as f64;
                prof.extend([(r, y - 2.0), (r - fin, y - 2.0), (r - fin, y - 4.0), (r, y - 4.0)]);
            }
            prof.extend([(r, y0 + 1.0), (r - 1.0, y0), (r * 0.45, y0), (r * 0.1, y0 + l * 0.5), (0.0, y0 + l * 0.5)]);
            b.lathe_y(BLACK, a, &prof);
            stem(&mut b, a - r, r);
        }
        "fiber_coupler" => {
            // body with its lens window at -Y (the fiber port) and an FC/PC connector at +Y
            let (r, l, a) = (p("diameter") / 2.0, p("length"), p("axis_height"));
            let (y0, y1) = (-l / 2.0, l / 2.0);
            let (cl, win) = (8f64.min(l / 3.0), r * 0.35);
            let body = [(0.0, y0 + 1.0), (win, y0 + 1.0), (win, y0), (r - 1.0, y0), (r, y0 + 1.0), (r, y1 - cl - 1.0), (r - 1.0, y1 - cl), (0.0, y1 - cl)];
            b.lathe_y(BLACK, a, &body);
            b.lathe_y(Finish::Glass, a, &[(0.0, y0 + 0.8), (win, y0 + 0.8), (win, y0 + 1.0), (0.0, y0 + 1.0)]);
            let nut = (r * 0.4).min(4.5);
            let (yn, ym) = (y1 - cl, y1 - cl * 0.4);
            b.lathe(STEEL, &[(0.0, yn), (nut, yn), (nut, ym), (0.0, ym)], 6, move |q| DVec3::new(-q.x, q.z, a + q.y));
            b.lathe_y(STEEL, a, &[(0.0, ym), (nut * 0.6, ym), (nut * 0.6, y1), (0.0, y1)]);
            stem(&mut b, a - r, r);
        }
        "laser_head" => {
            // body, heat-sink fins on top, exit aperture on +Y at the axis height
            let (l, w, h, a) = (p("length"), p("width"), p("height"), p("axis_height"));
            let c = (w.min(l).min(h) * 0.06).min(2.0);
            let fin = (h * 0.15).min(10.0);
            let nose = (l * 0.04).min(8.0);
            let front = l / 2.0 - nose;
            b.prism_z(GREY, &chamfer_rect(DVec2::new(-w / 2.0, -l / 2.0), DVec2::new(w / 2.0, front), c), 0.0, h - fin);
            let n = ((w / 6.0).floor() as usize).clamp(2, 16);
            let (pitch, inset) = (w / n as f64, 5f64.min(l / 10.0));
            for i in 0..n {
                let x = -w / 2.0 + (i as f64 + 0.5) * pitch;
                let ft = (pitch * 0.4).min(2.0) / 2.0;
                let fin_rect = chamfer_rect(DVec2::new(x - ft, -l / 2.0 + inset), DVec2::new(x + ft, front - inset), 0.0);
                b.prism_z(BLACK, &fin_rect, h - fin - 0.1, h);
            }
            let ar = (w.min(h) * 0.1).min(a - 0.5).min(h - fin - a - 2.5).max(0.5);
            let y1 = l / 2.0;
            b.lathe_y(BLACK, a, &[(0.0, front), (ar + 2.0, front), (ar + 2.0, y1), (ar, y1), (ar, y1 - 0.5), (0.0, y1 - 0.5)]);
        }
        "cel_head" => {
            // MOGLabs CEL-style ECDL: U-shaped body open on top, round output nose on +Y at the axis
            let (l, w, h, a) = (p("length"), p("width"), p("height"), p("axis_height"));
            let (y0, y1) = (-l / 2.0, l / 2.0);
            let base = 35f64.min(h * 0.4);
            let t = 10f64.min(l / 4.0);
            b.prism_z(GREY, &chamfer_rect(DVec2::new(-w / 2.0, y0), DVec2::new(w / 2.0, y1), 1.0), 0.0, base);
            // uprights with rounded top corners, as in the front view
            let r = 15f64.min(w / 4.0).min(h - base);
            let mut plate = vec![DVec2::new(-w / 2.0, 0.0), DVec2::new(w / 2.0, 0.0)];
            for (cx, from) in [(w / 2.0 - r, 0.0), (-w / 2.0 + r, 90.0)] {
                plate.extend((0..=6).map(|i| DVec2::new(cx, h - r) + DVec2::from_angle((from + 15.0 * i as f64).to_radians()) * r));
            }
            b.prism_y(GREY, &plate, y1 - t, y1);
            b.prism_y(GREY, &plate, y0, y0 + t);
            // dark aperture-window band across the front face, and the Ø10 nose protruding 4.5 mm
            let band = 4f64.min(a).min(h - a);
            b.cuboid(BLACK, DVec3::new(-w / 2.0 + 3.0, y1 - 0.1, a - band), DVec3::new(w / 2.0 - 3.0, y1 + 0.2, a + band));
            b.lathe_y(BLACK, a, &[(0.0, y1), (5.0, y1), (5.0, y1 + 4.5), (1.5, y1 + 4.5), (1.5, y1 + 4.3), (0.0, y1 + 4.3)]);
            // 4 x M6 clearance holes (Ø 6.6) on the 50.5 mm square, as dark discs on the floor top
            for (x, y) in [(-25.25, 40.3), (25.25, 40.3), (-25.25, -10.2), (25.25, -10.2)] {
                let (x, y) = (x * w / 70.0, y * l / 108.0);
                b.lathe(BLACK, &[(0.0, base), (3.3, base), (3.3, base + 0.1), (0.0, base + 0.1)], SIDES, move |q| q + DVec3::new(x, y, 0.0));
            }
            // electrical connector on the rear face
            let cz = 14f64.min(base - 4.0).max(4.0);
            b.cuboid(STEEL, DVec3::new(-7.5, y0 - 5.0, cz - 4.0), DVec3::new(7.5, y0, cz + 4.0));
        }
        "modulator_box" | "photodiode" | "camera" | "spcm" | "crystal_oven" => {
            // receivers face -Y: an input aperture or lens barrel there, a connector on top
            let (l, w, h, a) = (p("length"), p("width"), p("height"), p("axis_height"));
            let c = (w.min(l).min(h) * 0.06).min(2.0);
            let front = match name {
                "camera" => (l * 0.2).min(12.0),
                "photodiode" | "spcm" => (l * 0.08).min(6.0),
                _ => (l * 0.05).min(4.0),
            };
            let conn = (h * 0.1).min(5.0);
            let (y0, y1) = (-l / 2.0, l / 2.0);
            b.prism_z(BLACK, &chamfer_rect(DVec2::new(-w / 2.0, y0 + front), DVec2::new(w / 2.0, y1), c), 0.0, h - conn);
            let room = a.min(h - conn - a).min(w / 2.0);
            let ar = (w.min(h) * 0.15).min(6.0).min(room - 1.0).max(0.5);
            let barrel = if name == "camera" { (room - 0.5).max(ar + 0.5) } else { (ar + 1.5).min(room).max(ar + 0.5) };
            b.lathe_y(BLACK, a, &[(0.0, y0 + 1.0), (ar, y0 + 1.0), (ar, y0), (barrel, y0), (barrel, y0 + front), (0.0, y0 + front)]);
            if name == "camera" {
                b.lathe_y(Finish::Glass, a, &[(0.0, y0 + 0.8), (ar, y0 + 0.8), (ar, y0 + 1.0), (0.0, y0 + 1.0)]);
            }
            if matches!(name, "modulator_box" | "crystal_oven") {
                // through-beam parts: the exit aperture on +Y
                b.lathe_y(STEEL, a, &annulus(ar, barrel, y1, y1 + 0.5, 0.0));
            }
            // SMA / BNC connector on the top, towards the back
            let cr = (conn * 0.6).min(w / 4.0).max(0.5);
            let cy = y1 - cr - 3f64.min(l / 8.0);
            b.lathe(STEEL, &[(0.0, h - conn - 0.1), (cr, h - conn - 0.1), (cr, h), (0.0, h)], 6, move |q| DVec3::new(q.x, cy + q.y, q.z));
            if name == "spcm" {
                // cooling fins on the top, beside the connector
                let n = ((l / 8.0).floor() as usize).clamp(2, 12);
                let span = cy - cr - 2.0 - (y0 + front);
                for i in 0..n {
                    let y = y0 + front + (i as f64 + 0.5) * span / n as f64;
                    let fin_rect = chamfer_rect(DVec2::new(-w / 2.0 + 2.0, y - 0.75), DVec2::new(w / 2.0 - 2.0, y + 0.75), 0.0);
                    b.prism_z(BLACK, &fin_rect, h - conn - 0.1, h - 0.5);
                }
            }
        }
        "riser" => {
            let (l, w) = (p("length"), p("width"));
            let c = (w.min(l) * 0.05).min(3.0);
            b.prism_z(BLACK, &chamfer_rect(DVec2::new(-w / 2.0, -l / 2.0), DVec2::new(w / 2.0, l / 2.0), c), 0.0, p("height"));
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

    /// Solid of revolution about the part's Z axis; see `lathe`.
    fn lathe_z(&mut self, finish: Finish, profile: &[(f64, f64)]) {
        self.lathe(finish, profile, SIDES, |q| q);
    }

    /// Solid of revolution about the line x = 0, z = `z_axis`; profile heights are part Y.
    fn lathe_y(&mut self, finish: Finish, z_axis: f64, profile: &[(f64, f64)]) {
        self.lathe(finish, profile, SIDES, move |q| DVec3::new(-q.x, q.z, z_axis + q.y));
    }

    fn lathe(&mut self, finish: Finish, profile: &[(f64, f64)], sides: usize, to_part: impl Fn(DVec3) -> DVec3) {
        self.add(finish, lathe(profile, sides, to_part));
    }

    /// Convex CCW polygon in (x, y), extruded from z0 to z1.
    fn prism_z(&mut self, finish: Finish, poly: &[DVec2], z0: f64, z1: f64) {
        self.add(finish, prism(poly, z1 - z0, |q| DVec3::new(q.x, q.y, z0 + q.z)));
    }

    /// Convex CCW polygon in (x, z), extruded from y0 to y1.
    fn prism_y(&mut self, finish: Finish, poly: &[DVec2], y0: f64, y1: f64) {
        self.add(finish, prism(poly, y1 - y0, |q| DVec3::new(q.x, y1 - q.z, q.y)));
    }

    /// Convex CCW polygon in (y, z), extruded from x0 to x1.
    fn prism_x(&mut self, finish: Finish, poly: &[DVec2], x0: f64, x1: f64) {
        self.add(finish, prism(poly, x1 - x0, |q| DVec3::new(x0 + q.z, q.x, q.y)));
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

const SIDES: usize = 32;

/// Solid of revolution of `profile`, (radius, height) points in order around the outline (from the
/// axis at the bottom out, up and back to the axis, or a closed loop for a ring), about local +Z,
/// mapped by the rigid map `to_part`.
fn lathe(profile: &[(f64, f64)], sides: usize, to_part: impl Fn(DVec3) -> DVec3) -> TriMesh {
    let origin = to_part(DVec3::ZERO);
    let dir = |v: DVec3| (to_part(v) - origin).normalize();
    let tau = std::f64::consts::TAU;
    let ring: Vec<DVec2> = (0..sides).map(|i| DVec2::from_angle(i as f64 * tau / sides as f64)).collect();
    let mut m = TriMesh::default();
    for edge in profile.windows(2) {
        let [(r0, h0), (r1, h1)] = [edge[0], edge[1]];
        let len = (r1 - r0).hypot(h1 - h0);
        if len < 1e-9 {
            continue;
        }
        let (nr, nh) = ((h1 - h0) / len, (r0 - r1) / len);
        for i in 0..sides {
            let (a, b) = (ring[i], ring[(i + 1) % sides]);
            let base = m.positions.len() as u32;
            for (k, (c, r, h)) in [(a, r0, h0), (b, r0, h0), (b, r1, h1), (a, r1, h1)].into_iter().enumerate() {
                m.positions.push(to_part((c * r).extend(h)));
                m.normals.push(dir((c * nr).extend(nh)));
                m.uvs.push(DVec2::new((i + k % 2) as f64, (k / 2) as f64));
            }
            m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    m
}

/// Closed prism of the convex CCW polygon `poly` from 0 to `len` along local +Z, mapped by the
/// rigid map `to_part`.
fn prism(poly: &[DVec2], len: f64, to_part: impl Fn(DVec3) -> DVec3) -> TriMesh {
    let origin = to_part(DVec3::ZERO);
    let dir = |v: DVec3| (to_part(v) - origin).normalize();
    let mut m = TriMesh::default();
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let e = (b - a).normalize();
        let quad = [a.extend(0.0), b.extend(0.0), b.extend(len), a.extend(len)].map(&to_part);
        push_quad(&mut m, quad, dir(DVec3::new(e.y, -e.x, 0.0)));
    }
    for (h, up) in [(0.0, false), (len, true)] {
        let base = m.positions.len() as u32;
        let normal = dir(DVec3::new(0.0, 0.0, if up { 1.0 } else { -1.0 }));
        for p in poly {
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

/// Rectangle `lo`..`hi` with its corners cut by `c` (0: square corners), CCW.
fn chamfer_rect(lo: DVec2, hi: DVec2, c: f64) -> Vec<DVec2> {
    let c = c.min((hi.x - lo.x) / 2.0).min((hi.y - lo.y) / 2.0).max(0.0);
    if c < 1e-9 {
        return vec![lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)];
    }
    vec![
        DVec2::new(lo.x + c, lo.y),
        DVec2::new(hi.x - c, lo.y),
        DVec2::new(hi.x, lo.y + c),
        DVec2::new(hi.x, hi.y - c),
        DVec2::new(hi.x - c, hi.y),
        DVec2::new(lo.x + c, hi.y),
        DVec2::new(lo.x, hi.y - c),
        DVec2::new(lo.x, lo.y + c),
    ]
}

/// Lathe profile of a ring from radius `ri` to `ro`, heights `h0`..`h1`, outer edges chamfered by `c`.
fn annulus(ri: f64, ro: f64, h0: f64, h1: f64, c: f64) -> [(f64, f64); 7] {
    let c = c.min((ro - ri) / 2.0).min((h1 - h0) / 2.0).max(0.0);
    [(ri, h0), (ro - c, h0), (ro, h0 + c), (ro, h1 - c), (ro - c, h1), (ri, h1), (ri, h0)]
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
    fn cel_head_matches_the_drawing() {
        let m = gen_with("cel_head", &[("length", 108.0), ("width", 70.0), ("height", 86.7), ("axis_height", 58.0)]);
        assert!(m.pieces.iter().any(|(_, t)| !t.indices.is_empty()));
        // nose tip at y = 54 + 4.5; the rear connector protrudes 5 mm behind y = -54
        near(m.min, DVec3::new(-35.0, -59.0, 0.0));
        near(m.max, DVec3::new(35.0, 58.5, 86.7));
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

    #[test]
    fn premade_models_keep_their_size_and_budget() {
        // outer boxes of the M2 block models; the detailed models stay within 1 mm of them
        let old: &[(&str, [f64; 3], [f64; 3])] = &[
            ("post", [-6.35, -6.30, 0.00], [6.35, 6.30, 50.00]),
            ("pedestal_post", [-19.00, -19.00, 0.00], [19.00, 19.00, 50.00]),
            ("post_holder", [-14.95, -14.99, 0.00], [15.00, 14.99, 50.00]),
            ("base", [-37.50, -12.50, 0.00], [37.50, 12.50, 10.00]),
            ("clamping_fork", [-25.00, -12.50, 0.00], [25.00, 12.50, 10.00]),
            ("kinematic_mount", [-19.05, -6.35, 0.00], [19.05, 8.35, 38.10]),
            ("fixed_mount", [-19.00, -5.00, 0.00], [19.00, 5.00, 39.30]),
            ("rotation_mount", [-25.00, -7.50, 0.00], [25.00, 7.50, 55.00]),
            ("periscope", [-25.00, -25.00, 0.00], [25.00, 25.00, 150.00]),
            ("mirror", [-12.70, -3.00, -12.70], [12.70, 3.00, 12.70]),
            ("lens", [-12.70, -2.00, -12.70], [12.70, 2.00, 12.70]),
            ("cube", [-12.70, -12.70, -12.70], [12.70, 12.70, 12.70]),
            ("waveplate", [-12.70, -1.00, -12.70], [12.70, 1.00, 12.70]),
            ("iris", [-25.00, -4.00, 0.00], [25.00, 4.00, 55.00]),
            ("beam_dump", [-12.50, -20.00, 0.00], [12.50, 20.00, 37.50]),
            ("fiber_coupler", [-12.50, -15.00, 0.00], [12.50, 15.00, 37.50]),
            ("modulator_box", [-20.00, -30.00, 0.00], [20.00, 30.00, 30.00]),
            ("photodiode", [-10.00, -20.00, 0.00], [10.00, 20.00, 50.00]),
            ("camera", [-15.00, -25.00, 0.00], [15.00, 25.00, 30.00]),
            ("spcm", [-30.00, -60.00, 0.00], [30.00, 60.00, 50.00]),
            ("crystal_oven", [-20.00, -30.00, 0.00], [20.00, 30.00, 30.00]),
            ("laser_head", [-40.00, -100.00, 0.00], [40.00, 100.00, 60.00]),
            ("riser", [-37.50, -50.00, 0.00], [37.50, 50.00, 25.00]),
        ];
        for &(name, lo, hi) in old {
            let m = gen_default(name);
            let tris: usize = m.pieces.iter().map(|(_, t)| t.indices.len() / 3).sum();
            assert!(tris > 0 && tris < 5000, "{name}: {tris} triangles");
            let err = (m.min - DVec3::from(lo)).abs().max((m.max - DVec3::from(hi)).abs()).max_element();
            assert!(err <= 1.0, "{name}: box {}..{} moved {err} mm", m.min, m.max);
        }
    }
}
