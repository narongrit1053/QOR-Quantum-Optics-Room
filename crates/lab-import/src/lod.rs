//! Vertex-clustering decimator for LOD1/LOD2 (meshopt is not in the offline registry).

use std::collections::HashMap;

use glam::{DVec2, DVec3};
use lab_geom::TriMesh;

/// Reduces `mesh` to at most `target` triangles (when reachable in 8 tries) by snapping its
/// vertices to a uniform grid. Each cell keeps the vertex farthest from the mesh centre, so
/// convex silhouettes and the bounding box survive; normals are averaged per cell.
pub fn decimate(mesh: &TriMesh, target: usize) -> TriMesh {
    if mesh.triangle_count() <= target || mesh.positions.is_empty() {
        return mesh.clone();
    }
    let (min, max) = bounds(&mesh.positions);
    let diag = (max - min).length().max(1e-9);
    // Binary search the cell size in log space; keep the finest result within the target.
    let (mut lo, mut hi) = ((diag / 4000.0).ln(), diag.ln());
    let mut best: Option<TriMesh> = None;
    for _ in 0..8 {
        let mid = 0.5 * (lo + hi);
        let out = cluster(mesh, min, max, mid.exp());
        if out.triangle_count() <= target {
            hi = mid;
            best = Some(out);
        } else {
            lo = mid;
        }
    }
    best.unwrap_or_else(|| cluster(mesh, min, max, hi.exp()))
}

pub fn bounds(points: &[DVec3]) -> (DVec3, DVec3) {
    points.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(a, b), &p| {
        (a.min(p), b.max(p))
    })
}

fn cluster(mesh: &TriMesh, min: DVec3, max: DVec3, cell: f64) -> TriMesh {
    let centre = 0.5 * (min + max);
    let mut cells: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut rep: Vec<(DVec3, f64)> = Vec::new(); // chosen position, its distance from centre
    let remap: Vec<u32> = mesh
        .positions
        .iter()
        .map(|&p| {
            let q = ((p - min) / cell).floor();
            let id = *cells.entry((q.x as i64, q.y as i64, q.z as i64)).or_insert_with(|| {
                rep.push((p, -1.0));
                (rep.len() - 1) as u32
            });
            let d = p.distance_squared(centre);
            if d > rep[id as usize].1 {
                rep[id as usize] = (p, d);
            }
            id
        })
        .collect();

    let mut out = TriMesh {
        positions: rep.iter().map(|r| r.0).collect(),
        normals: vec![DVec3::ZERO; rep.len()],
        ..Default::default()
    };
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| remap[i as usize]);
        if a == b || b == c || a == c {
            continue;
        }
        out.indices.extend([a, b, c]);
        // Area-weighted face normal, accumulated per cell.
        let [pa, pb, pc] = [a, b, c].map(|i| out.positions[i as usize]);
        let n = (pb - pa).cross(pc - pa);
        for i in [a, b, c] {
            out.normals[i as usize] += n;
        }
    }
    for n in &mut out.normals {
        *n = n.try_normalize().unwrap_or(DVec3::Z);
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
}
