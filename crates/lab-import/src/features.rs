//! Planes and axes inferred from triangles (M8-PLAN A). step2glb merges faces, so there is no
//! B-rep: a plane is the coplanar region grown from a triangle, an axis is a cylinder fitted to
//! a smoothly curved region (holes and shafts).

use std::collections::HashMap;

use glam::DVec3;
use lab_core::part::Feature;
use lab_geom::TriMesh;

use crate::fit::{basis, kasa, scatter, smallest_eigenvector};

/// Coplanar: normals within 2°, vertices within 0.05 mm of the seed's plane.
const PLANE_COS: f64 = 0.999_390_827; // cos 2°
const PLANE_DIST: f64 = 0.05;
/// One curved surface: neighbours turn by less than 40°, so 90° box edges split regions.
// shortcut: a fillet tangent to a flat wall joins it and the fit fails; split by curvature if that matters.
const CURVE_COS: f64 = 0.766_044_443; // cos 40°
const MIN_PLANE_AREA: f64 = 25.0;
const MAX_PLANES: usize = 12;

/// The feature under a click: the cylinder through the hit if the surface is curved there,
/// else the coplanar region's plane. `point`/`normal` in part space.
pub fn pick_feature(mesh: &TriMesh, point: DVec3, normal: DVec3) -> Option<Feature> {
    let m = Welded::new(mesh);
    let seed = m.under(point, normal)?;
    let curved = m.grow(seed, &mut vec![false; m.tris.len()], |t, u| m.tris[t].n.dot(m.tris[u].n) > CURVE_COS);
    m.cylinder(&curved).or_else(|| Some(m.plane(&m.plane_region(seed, &mut vec![false; m.tris.len()])).0))
}

/// Whole-mesh detection: cylinders with radius 0.5–30 mm (axes deduplicated within 0.2 mm / 1°),
/// then planar regions ≥ 25 mm² (largest 12).
pub fn detect(mesh: &TriMesh) -> Vec<Feature> {
    let m = Welded::new(mesh);
    let n = m.tris.len();
    let mut out: Vec<Feature> = Vec::new();
    let mut done = vec![false; n];
    for t in 0..n {
        if done[t] {
            continue;
        }
        let curved = m.grow(t, &mut done, |a, b| m.tris[a].n.dot(m.tris[b].n) > CURVE_COS);
        if let Some(f) = m.cylinder(&curved).filter(|f| !out.iter().any(|g| same_axis(f, g))) {
            out.push(f);
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| m.tris[b].area.total_cmp(&m.tris[a].area));
    let mut done = vec![false; n];
    let mut planes = Vec::new();
    for t in order {
        if !done[t] {
            let (f, area) = m.plane(&m.plane_region(t, &mut done));
            if area >= MIN_PLANE_AREA {
                planes.push((f, area));
            }
        }
    }
    planes.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.extend(planes.into_iter().take(MAX_PLANES).map(|(f, _)| f));
    out
}

fn same_axis(a: &Feature, b: &Feature) -> bool {
    let (Feature::Axis { origin: o1, dir: d1, radius: r1, .. }, Feature::Axis { origin: o2, dir: d2, radius: r2, .. }) =
        (a, b)
    else {
        return false;
    };
    let off = *o2 - *o1;
    d1.dot(*d2).abs() > 0.999_847_695 // cos 1°
        && (off - *d1 * off.dot(*d1)).length() < 0.2
        && (r1 - r2).abs() < 0.2
}

struct Tri {
    v: [usize; 3],
    n: DVec3,
    area: f64,
}

/// The mesh with vertices welded (0.001 mm) and edge neighbours: the merged mesh is unwelded.
struct Welded {
    pos: Vec<DVec3>,
    tris: Vec<Tri>,
    near: Vec<Vec<usize>>,
}

impl Welded {
    fn new(mesh: &TriMesh) -> Self {
        let mut ids: HashMap<(i64, i64, i64), usize> = HashMap::new();
        let mut pos = Vec::new();
        let mut tris = Vec::new();
        for t in mesh.indices.chunks_exact(3) {
            let Some(p) = t.iter().map(|&i| mesh.positions.get(i as usize).copied()).collect::<Option<Vec<_>>>() else {
                continue;
            };
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            let area = cross.length() / 2.0;
            if area < 1e-9 {
                continue;
            }
            let v = [0, 1, 2].map(|k| {
                let q = (p[k] * 1000.0).round();
                *ids.entry((q.x as i64, q.y as i64, q.z as i64)).or_insert_with(|| {
                    pos.push(p[k]);
                    pos.len() - 1
                })
            });
            if v[0] != v[1] && v[1] != v[2] && v[0] != v[2] {
                tris.push(Tri { v, n: cross / (2.0 * area), area });
            }
        }
        let mut edges: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            for e in 0..3 {
                let (a, b) = (t.v[e], t.v[(e + 1) % 3]);
                edges.entry((a.min(b), a.max(b))).or_default().push(i);
            }
        }
        let mut near = vec![Vec::new(); tris.len()];
        for list in edges.values() {
            for &a in list {
                near[a].extend(list.iter().copied().filter(|&b| b != a));
            }
        }
        Welded { pos, tris, near }
    }

    fn corners(&self, t: usize) -> [DVec3; 3] {
        self.tris[t].v.map(|i| self.pos[i])
    }

    fn centroid(&self, t: usize) -> DVec3 {
        let [a, b, c] = self.corners(t);
        (a + b + c) / 3.0
    }

    /// The triangle the click hit: on its plane (0.1 mm) and inside it, facing like `normal`;
    /// else the nearest centroid.
    fn under(&self, point: DVec3, normal: DVec3) -> Option<usize> {
        let facing = |t: usize| normal.length_squared() < 0.5 || self.tris[t].n.dot(normal) > 0.5;
        let inside = |t: usize| {
            let [a, b, c] = self.corners(t);
            let n = self.tris[t].n;
            (point - a).dot(n).abs() < 0.1
                && [(a, b), (b, c), (c, a)].iter().all(|(p, q)| (*q - *p).cross(point - *p).dot(n) > -1e-6)
        };
        (0..self.tris.len()).find(|&t| facing(t) && inside(t)).or_else(|| {
            (0..self.tris.len())
                .filter(|&t| facing(t))
                .min_by(|&a, &b| (self.centroid(a) - point).length().total_cmp(&(self.centroid(b) - point).length()))
        })
    }

    /// Triangles reachable from `seed` over shared edges where `join(from, to)` holds.
    fn grow(&self, seed: usize, done: &mut [bool], join: impl Fn(usize, usize) -> bool) -> Vec<usize> {
        let mut region = vec![seed];
        done[seed] = true;
        let mut i = 0;
        while i < region.len() {
            let t = region[i];
            i += 1;
            for &u in &self.near[t] {
                if !done[u] && join(t, u) {
                    done[u] = true;
                    region.push(u);
                }
            }
        }
        region
    }

    fn plane_region(&self, seed: usize, done: &mut [bool]) -> Vec<usize> {
        let (n0, d0) = (self.tris[seed].n, self.pos[self.tris[seed].v[0]].dot(self.tris[seed].n));
        self.grow(seed, done, |_, u| {
            self.tris[u].n.dot(n0) > PLANE_COS && self.corners(u).iter().all(|p| (p.dot(n0) - d0).abs() < PLANE_DIST)
        })
    }

    /// The region's plane (area-weighted centroid and normal) and its area.
    fn plane(&self, region: &[usize]) -> (Feature, f64) {
        let area: f64 = region.iter().map(|&t| self.tris[t].area).sum();
        let origin = region.iter().map(|&t| self.centroid(t) * self.tris[t].area).sum::<DVec3>() / area;
        let normal = region.iter().map(|&t| self.tris[t].n * self.tris[t].area).sum::<DVec3>().normalize();
        (Feature::Plane { origin, normal }, area)
    }

    /// A cylinder fitted to a curved region, or `None` if it is flat or not round.
    fn cylinder(&self, region: &[usize]) -> Option<Feature> {
        if region.len() < 3 {
            return None;
        }
        let m = scatter(region.iter().map(|&t| (self.tris[t].n, self.tris[t].area)));
        let total = m.x_axis.x + m.y_axis.y + m.z_axis.z;
        let a = smallest_eigenvector(m, DVec3::new(0.3, 0.5, 0.8));
        // Normals ⟂ the axis, and spread over at least ~50° of arc (a flat region is not).
        let (u, v) = basis(a);
        let q = |x: DVec3, y: DVec3| (m * x).dot(y);
        let (p, r, s) = (q(u, u), q(u, v), q(v, v));
        let spread = (p + s) / 2.0 - (((p - s) / 2.0).powi(2) + r * r).sqrt();
        if q(a, a) > 0.02 * total || spread < 0.05 * total {
            return None;
        }
        let pts: Vec<DVec3> = region.iter().flat_map(|&t| self.corners(t)).collect();
        let to2 = |p: DVec3| (p.dot(u), p.dot(v));
        let (cx, cy, radius) = kasa(pts.iter().map(|p| to2(*p)))?;
        let rms = (pts.iter().map(|p| (dist(to2(*p), (cx, cy)) - radius).powi(2)).sum::<f64>() / pts.len() as f64).sqrt();
        if !(0.5..=30.0).contains(&radius) || rms > 0.05 * radius + 0.05 {
            return None;
        }
        // Concave when the walls face the axis.
        let facing: f64 = region
            .iter()
            .map(|&t| {
                let (c, n) = (to2(self.centroid(t)), to2(self.tris[t].n));
                self.tris[t].area * ((cx - c.0) * n.0 + (cy - c.1) * n.1).signum()
            })
            .sum();
        let hole = facing > 0.0;
        let (lo, hi) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.dot(a)), hi.max(p.dot(a))));
        let centre = u * cx + v * cy;
        // An end is capped when a face across the axis covers its middle (blind bottom, shaft tip).
        let capped = |h: f64| {
            (0..self.tris.len()).any(|t| {
                let c = self.centroid(t);
                let off = c - centre;
                self.tris[t].n.dot(a).abs() > 0.9
                    && (c.dot(a) - h).abs() < 0.3
                    && (off - a * off.dot(a)).length() < 0.8 * radius
            })
        };
        // Hole: the open end; shaft: the capped tip. Undecided: the end facing down (seats).
        let at_hi = match (capped(lo), capped(hi)) {
            (true, false) => hole,
            (false, true) => !hole,
            _ => a.z.abs() < 0.1 || a.z < 0.0,
        };
        let (origin, dir) = if at_hi { (centre + a * hi, a) } else { (centre + a * lo, -a) };
        Some(Feature::Axis { origin, dir, radius, hole })
    }
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 30 × 30 × 10 mm block centred on the origin with a Ø6 mm through hole along Z at
    /// (2, 1), outward winding, 48 segments (corners fall on segment angles).
    fn block() -> TriMesh {
        let mut m = TriMesh::default();
        let (seg, r, h) = (48usize, 3.0, 10.0);
        let c = DVec3::new(2.0, 1.0, 0.0);
        let mut quad = |q: [DVec3; 4]| {
            let base = m.positions.len() as u32;
            m.positions.extend(q);
            m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        let ang = |k: usize| std::f64::consts::TAU * k as f64 / seg as f64;
        let rim = |k: usize, z: f64| c + DVec3::new(r * ang(k).cos(), r * ang(k).sin(), z);
        // Radial projection of the rim direction onto the square, about the hole centre.
        let edge = |k: usize, z: f64| {
            let d = DVec3::new(ang(k).cos(), ang(k).sin(), 0.0);
            let t = ((15.0 - c.x * d.x.signum()) / d.x.abs()).min((15.0 - c.y * d.y.signum()) / d.y.abs());
            c + d * t + DVec3::Z * z
        };
        for i in 0..seg {
            let j = i + 1;
            quad([rim(i, h), edge(i, h), edge(j, h), rim(j, h)]);
            quad([rim(j, 0.0), edge(j, 0.0), edge(i, 0.0), rim(i, 0.0)]);
            quad([edge(i, 0.0), edge(j, 0.0), edge(j, h), edge(i, h)]);
            quad([rim(i, 0.0), rim(i, h), rim(j, h), rim(j, 0.0)]);
        }
        m
    }

    #[test]
    fn detects_the_hole_and_the_top_and_picks_a_wall() {
        let mesh = block();
        let found = detect(&mesh);
        let hole = found
            .iter()
            .find_map(|f| match f {
                Feature::Axis { origin, dir, radius, hole: true } => Some((*origin, *dir, *radius)),
                _ => None,
            })
            .expect("a hole axis");
        assert!(hole.1.cross(DVec3::Z).length() < 0.02, "dir {:?}", hole.1);
        assert!((hole.0.truncate() - glam::DVec2::new(2.0, 1.0)).length() < 0.1, "origin {:?}", hole.0);
        assert!((hole.2 - 3.0).abs() < 0.1, "radius {}", hole.2);
        // Through hole: the downward opening at z = 0 (the seat rule needs that).
        assert!(hole.1.z < 0.0 && hole.0.z.abs() < 0.1, "{hole:?}");
        assert!(
            found.iter().any(|f| matches!(f, Feature::Plane { origin, normal }
                if normal.angle_between(DVec3::Z) < 0.01 && (origin.z - 10.0).abs() < 1e-6)),
            "{found:?}"
        );
        assert_eq!(found.iter().filter(|f| matches!(f, Feature::Axis { .. })).count(), 1);

        match pick_feature(&mesh, DVec3::new(15.0, 5.0, 5.0), DVec3::X) {
            Some(Feature::Plane { origin, normal }) => {
                assert!(normal.angle_between(DVec3::X) < 0.01, "{normal:?}");
                assert!((origin.x - 15.0).abs() < 1e-6);
            }
            other => panic!("{other:?}"),
        }
        // The middle of the hole wall's first facet.
        let a = std::f64::consts::TAU / 96.0;
        let wall = DVec3::new(2.0, 1.0, 5.0) + DVec3::new(a.cos(), a.sin(), 0.0) * 3.0 * a.cos();
        assert!(matches!(pick_feature(&mesh, wall, -DVec3::X), Some(Feature::Axis { hole: true, .. })));
    }
}
