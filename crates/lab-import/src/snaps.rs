//! Joint-origin snaps on a part mesh (docs/M9-PLAN.md C): circular edge centres, vertices,
//! mid-edges and face centres, all in the part frame (mm).

use std::collections::HashMap;

use glam::DVec3;
use lab_core::part::Frame;
use lab_geom::TriMesh;

use crate::fit::{basis, kasa, scatter, smallest_eigenvector};

/// Sharp edge: the two faces turn by more than 30°.
const SHARP_COS: f64 = 0.866_025_404; // cos 30°
/// Coplanar region: normals within 2°, vertices within 0.05 mm (as `features`).
const PLANE_COS: f64 = 0.999_390_827;
const PLANE_DIST: f64 = 0.05;
/// Straight edge run: consecutive segments within ~2.5°.
const LINE_COS: f64 = 0.999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    FaceCentre,
    EdgeCentre,
    Vertex,
    MidEdge,
}

/// `frame.z` points out of the material (face normal / circle axis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub kind: SnapKind,
    pub frame: Frame,
}

/// Best snap near a hit, priority: circular edge centre (rim of a fitted cylinder within `radius`),
/// vertex, mid-edge (sharp edges > 30°) within `radius`, else the centroid of the coplanar region.
pub fn snap_at(mesh: &TriMesh, point: DVec3, normal: DVec3, radius: f64) -> Option<Snap> {
    let m = Mesh::new(mesh);
    let hit = m.under(point, normal)?;
    let face_n = m.tris[hit].n;
    let frame = |kind, origin, z: DVec3| Snap { kind, frame: Frame { origin, z, x: basis(z).0 } };

    // Sharp edges within reach, nearest first.
    let mut near: Vec<((usize, usize), f64)> = m
        .sharp
        .iter()
        .map(|&(a, b)| ((a, b), seg_dist(point, m.pos[a], m.pos[b])))
        .filter(|(_, d)| *d <= radius)
        .collect();
    near.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut seen = vec![false; m.pos.len()];
    for &((a, _), _) in &near {
        if seen[a] {
            continue;
        }
        let ring = m.component(a, &mut seen);
        if let Some((centre, axis)) = m.circle(&ring) {
            return Some(frame(SnapKind::EdgeCentre, centre, axis));
        }
    }
    let corner = (0..m.pos.len())
        .filter(|&v| m.degree(v) >= 3 && (m.pos[v] - point).length() <= radius)
        .min_by(|&a, &b| (m.pos[a] - point).length().total_cmp(&(m.pos[b] - point).length()));
    if let Some(v) = corner {
        return Some(frame(SnapKind::Vertex, m.pos[v], face_n));
    }
    if let Some(&((a, b), _)) = near.first() {
        let (s, e) = (m.run_end(a, b), m.run_end(b, a));
        let mid = (m.pos[s] + m.pos[e]) / 2.0;
        let dir = (m.pos[e] - m.pos[s]).normalize_or_zero();
        let x = (dir - face_n * dir.dot(face_n)).normalize_or(basis(face_n).0);
        return Some(Snap { kind: SnapKind::MidEdge, frame: Frame { origin: mid, z: face_n, x } });
    }
    let (centre, n) = m.plane_centre(hit);
    Some(frame(SnapKind::FaceCentre, centre, n))
}

fn seg_dist(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let d = b - a;
    let t = if d.length_squared() > 0.0 { ((p - a).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    (a + d * t - p).length()
}

struct Tri {
    v: [usize; 3],
    n: DVec3,
    area: f64,
}

/// Welded mesh (0.001 mm) with its triangle neighbours and sharp/boundary edges.
struct Mesh {
    pos: Vec<DVec3>,
    tris: Vec<Tri>,
    edges: HashMap<(usize, usize), Vec<usize>>,
    sharp: Vec<(usize, usize)>,
    /// Sharp edges at each vertex: the other end.
    links: Vec<Vec<usize>>,
}

impl Mesh {
    fn new(mesh: &TriMesh) -> Self {
        let mut ids: HashMap<(i64, i64, i64), usize> = HashMap::new();
        let (mut pos, mut tris) = (Vec::new(), Vec::new());
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
        let mut sharp: Vec<(usize, usize)> = edges
            .iter()
            .filter(|(_, ts)| ts.len() != 2 || tris[ts[0]].n.dot(tris[ts[1]].n) < SHARP_COS)
            .map(|(&e, _)| e)
            .collect();
        sharp.sort_unstable();
        let mut links = vec![Vec::new(); pos.len()];
        for &(a, b) in &sharp {
            links[a].push(b);
            links[b].push(a);
        }
        Mesh { pos, tris, edges, sharp, links }
    }

    fn degree(&self, v: usize) -> usize {
        self.links[v].len()
    }

    fn corners(&self, t: usize) -> [DVec3; 3] {
        self.tris[t].v.map(|i| self.pos[i])
    }

    fn centroid(&self, t: usize) -> DVec3 {
        let [a, b, c] = self.corners(t);
        (a + b + c) / 3.0
    }

    /// The triangle the click hit (on its plane within 0.1 mm, inside, facing like `normal`),
    /// else the nearest facing centroid.
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

    /// Vertices joined to `v` over sharp edges.
    fn component(&self, v: usize, seen: &mut [bool]) -> Vec<usize> {
        let mut out = vec![v];
        seen[v] = true;
        let mut i = 0;
        while i < out.len() {
            for &w in &self.links[out[i]] {
                if !seen[w] {
                    seen[w] = true;
                    out.push(w);
                }
            }
            i += 1;
        }
        out
    }

    /// (centre, axis out of the material) when `ring` is a closed, planar, round loop.
    fn circle(&self, ring: &[usize]) -> Option<(DVec3, DVec3)> {
        if ring.len() < 6 || ring.iter().any(|&v| self.degree(v) != 2) {
            return None;
        }
        let c = ring.iter().map(|&v| self.pos[v]).sum::<DVec3>() / ring.len() as f64;
        let n = smallest_eigenvector(scatter(ring.iter().map(|&v| (self.pos[v] - c, 1.0))), DVec3::new(0.3, 0.5, 0.8));
        if ring.iter().any(|&v| (self.pos[v] - c).dot(n).abs() > PLANE_DIST) {
            return None;
        }
        let (u, w) = basis(n);
        let to2 = |p: DVec3| (p.dot(u), p.dot(w));
        let (cx, cy, r) = kasa(ring.iter().map(|&v| to2(self.pos[v])))?;
        let worst = ring
            .iter()
            .map(|&v| {
                let (x, y) = to2(self.pos[v]);
                (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs()
            })
            .fold(0.0, f64::max);
        if !(0.2..=60.0).contains(&r) || worst > 0.05 * r + 0.05 {
            return None;
        }
        // Out of the material: the flat faces along the rim (end cap / face round a hole).
        let side: f64 = ring
            .iter()
            .flat_map(|&a| self.links[a].iter().map(move |&b| (a.min(b), a.max(b))))
            .filter_map(|e| self.edges.get(&e))
            .flatten()
            .map(|&t| (self.tris[t].n.dot(n), self.tris[t].area))
            .filter(|(d, _)| d.abs() > 0.9)
            .map(|(d, a)| d * a)
            .sum();
        let n = if side < 0.0 { -n } else { n };
        Some((u * cx + w * cy + n * c.dot(n), n))
    }

    /// Far end of the straight sharp-edge run leaving `from` away from `prev`.
    fn run_end(&self, prev: usize, from: usize) -> usize {
        let (mut prev, mut at) = (prev, from);
        for _ in 0..self.pos.len() {
            let dir = (self.pos[at] - self.pos[prev]).normalize_or_zero();
            let next = match self.links[at].as_slice() {
                [a, b] => if *a == prev { *b } else { *a },
                _ => break,
            };
            if (self.pos[next] - self.pos[at]).normalize_or_zero().dot(dir) < LINE_COS {
                break;
            }
            (prev, at) = (at, next);
        }
        at
    }

    /// Area-weighted centroid and normal of the coplanar region around `seed`.
    fn plane_centre(&self, seed: usize) -> (DVec3, DVec3) {
        let (n0, d0) = (self.tris[seed].n, self.pos[self.tris[seed].v[0]].dot(self.tris[seed].n));
        let mut done = vec![false; self.tris.len()];
        let mut region = vec![seed];
        done[seed] = true;
        let mut i = 0;
        while i < region.len() {
            let t = region[i];
            i += 1;
            for e in 0..3 {
                let (a, b) = (self.tris[t].v[e], self.tris[t].v[(e + 1) % 3]);
                for &u in self.edges.get(&(a.min(b), a.max(b))).into_iter().flatten() {
                    if !done[u]
                        && self.tris[u].n.dot(n0) > PLANE_COS
                        && self.corners(u).iter().all(|p| (p.dot(n0) - d0).abs() < PLANE_DIST)
                    {
                        done[u] = true;
                        region.push(u);
                    }
                }
            }
        }
        let area: f64 = region.iter().map(|&t| self.tris[t].area).sum();
        let centre = region.iter().map(|&t| self.centroid(t) * self.tris[t].area).sum::<DVec3>() / area;
        let n = region.iter().map(|&t| self.tris[t].n * self.tris[t].area).sum::<DVec3>().normalize();
        (centre, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Ø12.7 × 50 mm post standing on the origin, 48 segments, outward winding.
    fn post() -> TriMesh {
        let (seg, r, h) = (48usize, 6.35, 50.0);
        let mut m = TriMesh::default();
        let ang = |k: usize| std::f64::consts::TAU * k as f64 / seg as f64;
        let rim = |k: usize, z: f64| DVec3::new(r * ang(k).cos(), r * ang(k).sin(), z);
        for i in 0..seg {
            let j = i + 1;
            let b = m.positions.len() as u32;
            m.positions.extend([rim(i, 0.0), rim(j, 0.0), rim(j, h), rim(i, h)]);
            m.indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
            let b = m.positions.len() as u32;
            m.positions.extend([DVec3::new(0.0, 0.0, h), rim(i, h), rim(j, h)]);
            m.positions.extend([DVec3::ZERO, rim(j, 0.0), rim(i, 0.0)]);
            m.indices.extend([b, b + 1, b + 2, b + 3, b + 4, b + 5]);
        }
        m
    }

    #[test]
    fn post_top_rim_snaps_to_the_axis_at_the_top() {
        let mesh = post();
        // On the top face, 1 mm inside the rim.
        let s = snap_at(&mesh, DVec3::new(5.35, 0.0, 50.0), DVec3::Z, 3.0).expect("snap");
        assert_eq!(s.kind, SnapKind::EdgeCentre);
        assert!((s.frame.origin - DVec3::new(0.0, 0.0, 50.0)).length() < 0.01, "{}", s.frame.origin);
        assert!(s.frame.z.dot(DVec3::Z) > 0.999, "{}", s.frame.z);
        // Far from any edge: the face centre.
        let c = snap_at(&mesh, DVec3::new(0.5, 0.0, 50.0), DVec3::Z, 3.0).expect("snap");
        assert_eq!(c.kind, SnapKind::FaceCentre);
    }
}
