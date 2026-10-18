//! Triangle meshes: extruded platforms, flat polygons and circles.

use std::collections::BTreeMap;

use glam::{DVec2, DVec3};

use crate::fillet::FlatPath;
use crate::polygon::orientation_sign;
use crate::triangulate::{TriangulateError, triangulate_all_vertices};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn append(&mut self, other: &TriMesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Test helper: every undirected edge (positions quantised to 1e-6 mm) is used by exactly
    /// two triangles in opposite directions. An empty mesh, an out-of-range index or a
    /// triangle with a collapsed edge is not closed.
    pub fn is_closed(&self) -> bool {
        if self.indices.is_empty() || !self.indices.len().is_multiple_of(3) {
            return false;
        }
        let quantise = |p: DVec3| {
            let q = (p * 1e6).round();
            (q.x as i64, q.y as i64, q.z as i64)
        };
        let mut ids = BTreeMap::new();
        let mut vertex_id = Vec::with_capacity(self.positions.len());
        for &p in &self.positions {
            let next = ids.len();
            vertex_id.push(*ids.entry(quantise(p)).or_insert(next));
        }
        let mut directed: BTreeMap<(usize, usize), u32> = BTreeMap::new();
        for tri in self.indices.as_chunks::<3>().0 {
            let mut v = [0usize; 3];
            for (k, &i) in tri.iter().enumerate() {
                match vertex_id.get(i as usize) {
                    Some(&id) => v[k] = id,
                    None => return false,
                }
            }
            for e in 0..3 {
                let (a, b) = (v[e], v[(e + 1) % 3]);
                if a == b {
                    return false;
                }
                *directed.entry((a, b)).or_insert(0) += 1;
            }
        }
        directed
            .iter()
            .all(|(&(a, b), &count)| count == 1 && directed.get(&(b, a)) == Some(&1))
    }

    fn push_vertex(&mut self, position: DVec3, normal: DVec3, uv: DVec2) -> u32 {
        self.positions.push(position);
        self.normals.push(normal);
        self.uvs.push(uv);
        (self.positions.len() - 1) as u32
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatformMeshes {
    pub top: TriMesh,
    pub body: TriMesh,
}

/// top: face at z_top, normal +Z, uv = local (x, y) in mm. body: side walls + bottom face at
/// z_bottom (normal −Z). Side normals: per-edge outward; averaged at `smooth` vertices; sharp
/// vertices are duplicated per edge. Side uv = (distance along the path, z) in mm.
pub fn extrude(
    path: &FlatPath,
    z_bottom: f64,
    z_top: f64,
) -> Result<PlatformMeshes, TriangulateError> {
    let points = &path.points;
    let top = flat_polygon(points, z_top, true, 1.0)?;
    let mut body = flat_polygon(points, z_bottom, false, 1.0)?;

    let n = points.len();
    let s = orientation_sign(points);
    let edge_normal = |i: usize| {
        let d = (points[(i + 1) % n] - points[i]).normalize_or_zero();
        DVec3::new(s * d.y, -s * d.x, 0.0)
    };
    let smooth = |i: usize| path.smooth.get(i).copied().unwrap_or(false);
    let vertex_normal = |i: usize| {
        let prev = edge_normal((i + n - 1) % n);
        let own = edge_normal(i);
        (prev + own).normalize_or(own)
    };
    // Distance along the path to the start of each edge.
    let mut along = Vec::with_capacity(n + 1);
    let mut acc = 0.0;
    for i in 0..n {
        along.push(acc);
        acc += points[i].distance(points[(i + 1) % n]);
    }
    along.push(acc);

    // Shared (bottom, top) vertex pairs at smooth vertices; vertex 0 is never shared across the
    // uv seam.
    let mut shared: Vec<Option<(u32, u32)>> = vec![None; n];
    let pair = |body: &mut TriMesh, i: usize, u: f64, normal: DVec3| {
        let p = points[i % n];
        let b = body.push_vertex(p.extend(z_bottom), normal, DVec2::new(u, z_bottom));
        let t = body.push_vertex(p.extend(z_top), normal, DVec2::new(u, z_top));
        (b, t)
    };
    for i in 0..n {
        let j = (i + 1) % n;
        let own = edge_normal(i);
        let (b0, t0) = if smooth(i) {
            match shared[i] {
                Some(v) => v,
                None => {
                    let v = pair(&mut body, i, along[i], vertex_normal(i));
                    shared[i] = Some(v);
                    v
                }
            }
        } else {
            pair(&mut body, i, along[i], own)
        };
        let (b1, t1) = if smooth(j) && j != 0 {
            match shared[j] {
                Some(v) => v,
                None => {
                    let v = pair(&mut body, j, along[i + 1], vertex_normal(j));
                    shared[j] = Some(v);
                    v
                }
            }
        } else if smooth(j) {
            pair(&mut body, j, along[i + 1], vertex_normal(j))
        } else {
            pair(&mut body, j, along[i + 1], own)
        };
        // Seen from outside a CCW outline, b0 -> b1 -> t1 is counter-clockwise.
        if s > 0.0 {
            body.indices.extend_from_slice(&[b0, b1, t1, b0, t1, t0]);
        } else {
            body.indices.extend_from_slice(&[b1, b0, t0, b1, t0, t1]);
        }
    }
    Ok(PlatformMeshes { top, body })
}

/// Flat polygon at z; up = true gives normal +Z with CCW winding from above. uv = (x, y) * uv_scale.
/// Every input point is a vertex (collinear points included), so the outline has no T-junctions.
pub fn flat_polygon(
    points: &[DVec2],
    z: f64,
    up: bool,
    uv_scale: f64,
) -> Result<TriMesh, TriangulateError> {
    let tris = triangulate_all_vertices(points)?;
    let normal = if up { DVec3::Z } else { DVec3::NEG_Z };
    let mut mesh = TriMesh {
        positions: points.iter().map(|p| p.extend(z)).collect(),
        normals: vec![normal; points.len()],
        uvs: points.iter().map(|&p| p * uv_scale).collect(),
        indices: Vec::with_capacity(tris.len() * 3),
    };
    for [a, b, c] in tris {
        if up {
            mesh.indices.extend_from_slice(&[a, b, c]);
        } else {
            mesh.indices.extend_from_slice(&[a, c, b]);
        }
    }
    Ok(mesh)
}

/// CCW circle approximation starting at angle 0, with enough segments that the chord sagitta
/// stays within `tolerance` (at least 3, at most `max_segments` when that is 3 or more).
/// Empty for a radius that is not positive and finite.
pub fn circle_points(
    center: DVec2,
    radius: f64,
    tolerance: f64,
    max_segments: usize,
) -> Vec<DVec2> {
    if !(radius > 0.0 && radius.is_finite()) {
        return Vec::new();
    }
    let step = 2.0 * (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos();
    let max = max_segments.max(3);
    let wanted = std::f64::consts::TAU / step;
    let n = if wanted.is_finite() {
        (wanted.ceil() as usize).clamp(3, max)
    } else {
        max
    };
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            center + radius * DVec2::new(a.cos(), a.sin())
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fillet::{FilletCorner, flatten_fillets};
    use crate::triangulate::tests::loose;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    /// Every triangle's geometric normal points the same way as its vertex normals.
    pub(crate) fn normals_agree(mesh: &TriMesh) -> bool {
        mesh.indices.as_chunks::<3>().0.iter().all(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            let g = (b - a).cross(c - a);
            [t[0], t[1], t[2]]
                .iter()
                .all(|&i| g.dot(mesh.normals[i as usize]) > 0.0)
        })
    }

    fn path_of(points: &[DVec2]) -> FlatPath {
        FlatPath {
            points: points.to_vec(),
            smooth: vec![false; points.len()],
            source_corner: (0..points.len() as u32).collect(),
        }
    }

    fn joined(m: &PlatformMeshes) -> TriMesh {
        let mut all = m.top.clone();
        all.append(&m.body);
        all
    }

    #[test]
    fn extruded_box_is_closed() {
        let sq = [v(0.0, 0.0), v(100.0, 0.0), v(100.0, 50.0), v(0.0, 50.0)];
        let m = extrude(&path_of(&sq), -10.0, 0.0).expect("extrude");
        assert!(!m.top.is_closed());
        let all = joined(&m);
        assert!(all.is_closed());
        assert!(normals_agree(&all));
        assert_eq!(m.top.triangle_count(), 2);
        assert_eq!(m.body.triangle_count(), 2 + 8);
        assert!(m.top.normals.iter().all(|&n| n == DVec3::Z));
        assert_eq!(m.top.uvs[2], v(100.0, 50.0));
    }

    #[test]
    fn extruded_rounded_rectangle_is_closed_both_orientations() {
        let pts = [
            v(-600.0, -450.0),
            v(600.0, -450.0),
            v(600.0, 450.0),
            v(-600.0, 450.0),
        ];
        for reverse in [false, true] {
            let mut corners: Vec<FilletCorner> = pts
                .iter()
                .map(|&point| FilletCorner {
                    point,
                    radius: 50.0,
                })
                .collect();
            if reverse {
                corners.reverse();
            }
            let path = flatten_fillets(&corners, 0.02, 2048, &loose()).expect("flatten");
            let m = extrude(&path, -60.0, 0.0).expect("extrude");
            let all = joined(&m);
            assert!(all.is_closed(), "reverse = {reverse}");
            assert!(normals_agree(&all), "reverse = {reverse}");
            // Side normals at arc-interior vertices are averaged, hence horizontal and unit.
            for n in &m.body.normals {
                assert!((n.length() - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn extruded_l_with_collinear_corner_is_closed() {
        let l = [
            v(0.0, 0.0),
            v(100.0, 0.0),
            v(200.0, 0.0),
            v(200.0, 100.0),
            v(100.0, 100.0),
            v(100.0, 200.0),
            v(0.0, 200.0),
        ];
        let m = extrude(&path_of(&l), 0.0, 25.0).expect("extrude");
        let all = joined(&m);
        assert!(all.is_closed());
        assert!(normals_agree(&all));
    }

    #[test]
    fn flat_polygon_up_and_down() {
        let sq = [
            v(0.0, 0.0),
            v(1000.0, 0.0),
            v(1000.0, 1000.0),
            v(0.0, 1000.0),
        ];
        let up = flat_polygon(&sq, 5.0, true, 0.001).expect("flat");
        assert!(normals_agree(&up));
        assert_eq!(up.uvs[2], v(1.0, 1.0));
        assert!(up.positions.iter().all(|p| p.z == 5.0));
        let down = flat_polygon(&sq, 5.0, false, 0.001).expect("flat");
        assert!(normals_agree(&down));
        assert!(down.normals.iter().all(|&n| n == DVec3::NEG_Z));
    }

    #[test]
    fn circle_is_ccw_and_within_tolerance() {
        let c = circle_points(v(10.0, -5.0), 3.0, 0.02, 64);
        assert!(c.len() >= 3 && c.len() <= 64);
        assert!(crate::polygon::signed_area(&c) > 0.0);
        let n = c.len() as f64;
        assert!(3.0 * (1.0 - (std::f64::consts::PI / n).cos()) <= 0.02 + 1e-12);
        assert_eq!(circle_points(v(0.0, 0.0), 1000.0, 0.02, 64).len(), 64);
        assert!(circle_points(v(0.0, 0.0), -1.0, 0.02, 64).is_empty());
        assert!(circle_points(v(0.0, 0.0), f64::NAN, 0.02, 64).is_empty());
    }

    #[test]
    fn append_offsets_indices() {
        let sq = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0)];
        let a = flat_polygon(&sq, 0.0, true, 1.0).expect("flat");
        let mut b = a.clone();
        b.append(&a);
        assert_eq!(b.positions.len(), 6);
        assert_eq!(&b.indices[3..], &[3, 4, 5]);
        assert_eq!(b.triangle_count(), 2);
    }
}
