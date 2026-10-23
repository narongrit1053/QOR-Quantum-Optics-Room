//! Quadric-error edge-collapse decimator for LOD0..2 (Garland–Heckbert; meshopt is not in the
//! offline registry). Vertices only ever move onto other existing vertices (half-edge
//! collapse), so a flat face stays exactly flat; boundary and crease edges carry penalty
//! planes so silhouettes and hole rims survive.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use glam::{DVec2, DVec3};
use lab_geom::TriMesh;

/// Vertices closer than this (mm) are welded before decimating.
const WELD: f64 = 1e-3;
/// Edges between faces turning by more than 30° are creases.
const CREASE_COS: f64 = 0.866_025_404;
/// Weight of a boundary/crease penalty plane, per mm² of edge length squared.
const PENALTY: f64 = 1e3;
/// A collapse may turn a remaining face by at most ~78° (anything more counts as a flip).
const FLIP_COS: f64 = 0.2;

/// Reduces `mesh` to at most `target` triangles where the collapses allow it. Normals are
/// recomputed per face group (smooth within 30°, split across sharper creases).
pub fn decimate(mesh: &TriMesh, target: usize) -> TriMesh {
    if mesh.triangle_count() <= target || mesh.positions.is_empty() {
        return mesh.clone();
    }
    let (pos, mut tris) = weld(mesh);
    collapse(&pos, &mut tris, target);
    shade(&pos, &tris)
}

pub fn bounds(points: &[DVec3]) -> (DVec3, DVec3) {
    points.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(a, b), &p| {
        (a.min(p), b.max(p))
    })
}

fn weld(mesh: &TriMesh) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut pos = Vec::new();
    let remap: Vec<u32> = mesh
        .positions
        .iter()
        .map(|&p| {
            let q = (p / WELD).round();
            *ids.entry((q.x as i64, q.y as i64, q.z as i64)).or_insert_with(|| {
                pos.push(p);
                (pos.len() - 1) as u32
            })
        })
        .collect();
    let tris = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| remap[i as usize]))
        .filter(|[a, b, c]| a != b && b != c && a != c)
        .collect();
    (pos, tris)
}

/// Symmetric 4×4 quadric: a², ab, ac, ad, b², bc, bd, c², cd, d².
type Quadric = [f64; 10];

fn plane_quadric(n: DVec3, p: DVec3, w: f64) -> Quadric {
    let (a, b, c, d) = (n.x, n.y, n.z, -n.dot(p));
    [a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d].map(|x| x * w)
}

fn add(q: &mut Quadric, r: &Quadric) {
    q.iter_mut().zip(r).for_each(|(a, b)| *a += b);
}

fn error(q: &Quadric, p: DVec3) -> f64 {
    let (x, y, z) = (p.x, p.y, p.z);
    q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y
        + 2.0 * q[5] * y * z
        + 2.0 * q[6] * y
        + q[7] * z * z
        + 2.0 * q[8] * z
        + q[9]
}

/// Moving vertex `from` onto vertex `to` costs `cost`; stale when either stamp changed.
struct Candidate {
    cost: f64,
    from: u32,
    to: u32,
    stamps: (u32, u32),
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    // Reversed: BinaryHeap pops the cheapest first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.total_cmp(&self.cost)
    }
}

fn collapse(pos: &[DVec3], tris: &mut Vec<[u32; 3]>, target: usize) {
    let n = pos.len();
    let cross = |t: &[u32; 3]| {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        (b - a).cross(c - a)
    };
    let mut faces: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut q = vec![[0.0; 10]; n];
    let mut edges: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (f, t) in tris.iter().enumerate() {
        let c = cross(t);
        let pq = plane_quadric(c.normalize_or_zero(), pos[t[0] as usize], 0.5 * c.length());
        for k in 0..3 {
            faces[t[k] as usize].push(f as u32);
            add(&mut q[t[k] as usize], &pq);
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(f as u32);
        }
    }
    // Penalty planes through boundary and crease edges, perpendicular to their faces.
    for (&(a, b), fs) in &edges {
        let normals: Vec<DVec3> = fs.iter().map(|&f| cross(&tris[f as usize]).normalize_or_zero()).collect();
        if let [n0, n1] = normals[..]
            && n0.dot(n1) >= CREASE_COS
        {
            continue;
        }
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let e = pb - pa;
        for nf in normals {
            let pq = plane_quadric(e.cross(nf).normalize_or_zero(), pa, PENALTY * e.length_squared());
            add(&mut q[a as usize], &pq);
            add(&mut q[b as usize], &pq);
        }
    }

    let mut stamp = vec![0u32; n];
    let mut dead = vec![false; tris.len()];
    let mut gone = vec![false; n];
    let mut heap = BinaryHeap::new();
    let push = |heap: &mut BinaryHeap<Candidate>, q: &[Quadric], stamp: &[u32], a: u32, b: u32| {
        let mut sum = q[a as usize];
        add(&mut sum, &q[b as usize]);
        let (ab, ba) = (error(&sum, pos[b as usize]), error(&sum, pos[a as usize]));
        let (from, to, cost) = if ab <= ba { (a, b, ab) } else { (b, a, ba) };
        heap.push(Candidate { cost, from, to, stamps: (stamp[from as usize], stamp[to as usize]) });
    };
    for &(a, b) in edges.keys() {
        push(&mut heap, &q, &stamp, a, b);
    }

    let mut alive = tris.len();
    while alive > target {
        let Some(c) = heap.pop() else { break };
        let (from, to) = (c.from as usize, c.to as usize);
        if gone[from] || gone[to] || c.stamps != (stamp[from], stamp[to]) {
            continue;
        }
        faces[from].retain(|&f| !dead[f as usize]);
        faces[to].retain(|&f| !dead[f as usize]);
        // Link condition: the edge's faces are the only ones the two vertices share.
        let shared: Vec<u32> = faces[from].iter().copied().filter(|&f| tris[f as usize].contains(&c.to)).collect();
        if shared.is_empty() || shared.len() > 2 {
            continue;
        }
        let (ra, rb) = (ring(&faces[from], tris), ring(&faces[to], tris));
        let common = ra.iter().filter(|v| **v != c.from && **v != c.to && rb.binary_search(v).is_ok()).count();
        if common != shared.len() {
            continue;
        }
        // No remaining face may flip or degenerate.
        let flips = faces[from].iter().filter(|f| !shared.contains(f)).any(|&f| {
            let t = tris[f as usize];
            let moved = t.map(|v| if v == c.from { c.to } else { v });
            let (n0, n1) = (cross(&t).normalize_or_zero(), cross(&moved));
            n1.length_squared() < 1e-18 || n0.dot(n1.normalize()) < FLIP_COS
        });
        if flips {
            continue;
        }

        for f in std::mem::take(&mut faces[from]) {
            if shared.contains(&f) {
                dead[f as usize] = true;
                alive -= 1;
            } else {
                tris[f as usize].iter_mut().filter(|v| **v == c.from).for_each(|v| *v = c.to);
                faces[to].push(f);
            }
        }
        faces[to].retain(|&f| !dead[f as usize]);
        gone[from] = true;
        stamp[to] += 1;
        let qf = q[from];
        add(&mut q[to], &qf);
        for v in ring(&faces[to], tris) {
            if v != c.to {
                push(&mut heap, &q, &stamp, c.to, v);
            }
        }
    }
    let mut f = 0;
    tris.retain(|_| {
        f += 1;
        !dead[f - 1]
    });
}

/// The vertices of `faces` (sorted, with the centre vertex itself).
fn ring(faces: &[u32], tris: &[[u32; 3]]) -> Vec<u32> {
    let mut r: Vec<u32> = faces.iter().flat_map(|&f| tris[f as usize]).collect();
    r.sort_unstable();
    r.dedup();
    r
}

/// Indexed mesh with one normal per (vertex, smooth face group): each corner averages the
/// area-weighted normals of the vertex's faces within 30° of its own face.
fn shade(pos: &[DVec3], tris: &[[u32; 3]]) -> TriMesh {
    let cross: Vec<DVec3> = tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            (b - a).cross(c - a)
        })
        .collect();
    let mut faces: Vec<Vec<usize>> = vec![Vec::new(); pos.len()];
    for (f, t) in tris.iter().enumerate() {
        t.iter().for_each(|&v| faces[v as usize].push(f));
    }
    let mut out = TriMesh::default();
    let mut ids: HashMap<(u32, [i64; 3]), u32> = HashMap::new();
    for (f, t) in tris.iter().enumerate() {
        let nf = cross[f].normalize_or_zero();
        for &v in t {
            let n = faces[v as usize]
                .iter()
                .filter(|&&g| cross[g].normalize_or_zero().dot(nf) >= CREASE_COS)
                .map(|&g| cross[g])
                .sum::<DVec3>()
                .try_normalize()
                .unwrap_or(nf);
            let key = (v, (n * 1e4).round().to_array().map(|x| x as i64));
            let id = *ids.entry(key).or_insert_with(|| {
                out.positions.push(pos[v as usize]);
                out.normals.push(n);
                (out.positions.len() - 1) as u32
            });
            out.indices.push(id);
        }
    }
    out.uvs = vec![DVec2::ZERO; out.positions.len()];
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(stacks: u32, slices: u32, r: f64) -> TriMesh {
        let mut m = TriMesh::default();
        for i in 0..=stacks {
            let t = std::f64::consts::PI * i as f64 / stacks as f64;
            for j in 0..=slices {
                let p = std::f64::consts::TAU * j as f64 / slices as f64;
                let n = DVec3::new(t.sin() * p.cos(), t.sin() * p.sin(), t.cos());
                m.positions.push(n * r);
                m.normals.push(n);
                m.uvs.push(DVec2::ZERO);
            }
        }
        let row = slices + 1;
        for i in 0..stacks {
            for j in 0..slices {
                let (a, b) = (i * row + j, (i + 1) * row + j);
                m.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        m
    }

    #[test]
    fn sphere_reduces_and_keeps_bounds() {
        let m = sphere(100, 100, 10.0);
        assert!(m.triangle_count() >= 19_000);
        let (min0, max0) = bounds(&m.positions);
        for frac in [0.25, 0.05] {
            let target = (m.triangle_count() as f64 * frac) as usize;
            let d = decimate(&m, target);
            assert!(d.triangle_count() <= target, "{} > {target}", d.triangle_count());
            assert!(d.triangle_count() > target / 4, "too coarse: {}", d.triangle_count());
            let (min1, max1) = bounds(&d.positions);
            let (e0, e1) = (max0 - min0, max1 - min1);
            assert!(((e1 - e0).abs() / e0).max_element() < 0.02, "{e0} vs {e1}");
        }
    }

    /// 100×100×10 mm plate with a Ø6 mm through hole, finely meshed, one vertex set per face
    /// group like step2glb writes it.
    fn plate() -> TriMesh {
        const N: usize = 64; // points round the hole and the outer square
        const RINGS: usize = 20;
        let dir = |k: usize| {
            let a = std::f64::consts::TAU * k as f64 / N as f64;
            DVec3::new(a.cos(), a.sin(), 0.0)
        };
        let square = |d: DVec3| d * (50.0 / d.x.abs().max(d.y.abs()));
        let mut m = TriMesh::default();
        let quad = |m: &mut TriMesh, p: [DVec3; 4]| {
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
            let base = m.positions.len() as u32;
            for v in p {
                m.positions.push(v);
                m.normals.push(n);
                m.uvs.push(DVec2::ZERO);
            }
            m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        for k in 0..N {
            let (d0, d1) = (dir(k), dir((k + 1) % N));
            for r in 0..RINGS {
                let at = |d: DVec3, r: usize| d * 3.0 + (square(d) - d * 3.0) * (r as f64 / RINGS as f64);
                let ring = [at(d0, r), at(d0, r + 1), at(d1, r + 1), at(d1, r)];
                quad(&mut m, ring.map(|p| p + DVec3::Z * 10.0));
                quad(&mut m, [ring[3], ring[2], ring[1], ring[0]]);
            }
            let up = DVec3::Z * 10.0;
            quad(&mut m, [d0 * 3.0, d0 * 3.0 + up, d1 * 3.0 + up, d1 * 3.0]);
            let (s0, s1) = (square(d0), square(d1));
            quad(&mut m, [s0, s1, s1 + up, s0 + up]);
        }
        m
    }

    #[test]
    fn plate_top_stays_one_plane() {
        let m = plate();
        // Orientation sanity: the top face points up.
        assert!(m.normals[0].z > 0.99, "{}", m.normals[0]);
        let target = m.triangle_count() / 4;
        let d = decimate(&m, target);
        assert!(d.triangle_count() <= target, "{} > {target}", d.triangle_count());
        for p in d.positions.iter().filter(|p| p.z > 5.0) {
            assert!((p.z - 10.0).abs() < 0.01, "top vertex off the plane: {p}");
        }
        match crate::features::pick_feature(&d, DVec3::new(7.0, 0.0, 10.0), DVec3::Z) {
            Some(lab_core::part::Feature::Plane { normal, .. }) => assert!(normal.z > 0.999, "{normal}"),
            other => panic!("expected the top plane, got {other:?}"),
        }
    }
}
