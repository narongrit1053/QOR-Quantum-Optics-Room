//! Fits a round hole's axis from mesh triangles around a clicked point (M4-PLAN POLA-60).

use glam::DVec3;
use lab_core::part::Frame;
use lab_geom::TriMesh;

/// A fitted hole: `axis.origin` is the centre of the opening, `axis.z` points out of the
/// material, toward the side the hole opens to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoleFit {
    pub axis: Frame,
    pub diameter: f64,
}

/// Triangles whose centroid lies within this distance (mm) of the click take part in the fit.
const REACH: f64 = 6.0;

/// Fits the round hole nearest `point`. `normal` is the surface normal at the click (the face
/// around the hole, or the hole's wall). `None` when no round hole is found.
pub fn fit_hole(mesh: &TriMesh, point: DVec3, normal: DVec3) -> Option<HoleFit> {
    let near: Vec<([DVec3; 3], DVec3, f64)> = (0..mesh.indices.len() / 3)
        .filter_map(|t| {
            let v = [0, 1, 2].map(|k| mesh.positions[mesh.indices[t * 3 + k] as usize]);
            let cross = (v[1] - v[0]).cross(v[2] - v[0]);
            let area = cross.length() / 2.0;
            ((v[0] + v[1] + v[2]) / 3.0 - point).length().le(&REACH).then_some(())?;
            (area > 1e-9).then(|| (v, cross / (2.0 * area), area))
        })
        .collect();
    if near.len() < 6 {
        return None;
    }
    let n0 = normal.normalize_or_zero();
    // Candidate axes: the clicked normal (click on the face around the hole), else the main
    // directions of the nearby normals (click on the hole's wall).
    let mut candidates = vec![n0];
    let m = scatter(near.iter().map(|(_, n, a)| (*n, *a)));
    candidates.extend(eigenvectors(m));
    candidates
        .into_iter()
        .filter(|a| a.length_squared() > 0.5)
        .filter_map(|a| fit_along(&near, a, point, n0))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(fit, _)| fit)
}

/// One attempt with axis guess `guess`: (fit, score).
fn fit_along(near: &[([DVec3; 3], DVec3, f64)], guess: DVec3, point: DVec3, n0: DVec3) -> Option<(HoleFit, f64)> {
    // Walls: normals nearly perpendicular to the axis. Refine the axis from them.
    let walls: Vec<_> = near.iter().filter(|(_, n, _)| n.dot(guess).abs() < 0.3).collect();
    if walls.len() < 6 {
        return None;
    }
    let m = scatter(walls.iter().map(|(_, n, a)| (*n, *a)));
    let mut axis = smallest_eigenvector(m, guess);
    if axis.dot(guess) < 0.0 {
        axis = -axis;
    }
    let (u, v) = basis(axis);
    let to2 = |p: DVec3| (p.dot(u), p.dot(v));
    // Wall vertices with their triangle normal (walls of a hole face its axis).
    let pts: Vec<WallPoint> = walls
        .iter()
        .flat_map(|(tri, n, _)| tri.iter().map(move |p| (to2(*p), (n.dot(u), n.dot(v)), *p)))
        .collect();
    let click = to2(point);

    // RANSAC over vertex triples (deterministic), then a least-squares refit on the inliers.
    let mut best: Option<(usize, (f64, f64, f64))> = None;
    let mut seed = 0x2545_f491_u64;
    let mut next = |len: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % len as u64) as usize
    };
    for _ in 0..300 {
        let (a, b, c) = (next(pts.len()), next(pts.len()), next(pts.len()));
        let Some(circle) = circle3(pts[a].0, pts[b].0, pts[c].0) else { continue };
        if !(0.5..=30.0).contains(&circle.2) || dist(click, (circle.0, circle.1)) > circle.2 + 3.0 {
            continue;
        }
        let count = inliers(&pts, circle).count();
        if best.is_none_or(|(n, _)| count > n) {
            best = Some((count, circle));
        }
    }
    let (count, circle) = best?;
    let inl: Vec<_> = inliers(&pts, circle).collect();
    let (cx, cy, r) = kasa(inl.iter().map(|p| p.0))?;
    if count < 6 || !(0.5..=30.0).contains(&r) {
        return None;
    }
    let rms = (inl.iter().map(|p| (dist(p.0, (cx, cy)) - r).powi(2)).sum::<f64>() / inl.len() as f64).sqrt();
    if rms > 0.05 * r + 0.05 {
        return None;
    }
    // Along-axis extent of the wall; the opening is at the end the axis points to.
    let heights: Vec<f64> = inl.iter().map(|p| p.2.dot(axis)).collect();
    let (lo, hi) = heights.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &h| (lo.min(h), hi.max(h)));
    // A click on the face says which side the hole opens to; a click on the wall keeps the guess.
    let flip = n0.dot(axis) < -0.3;
    let origin = u * cx + v * cy + axis * if flip { lo } else { hi };
    let z = if flip { -axis } else { axis };
    let score = count as f64 / (1.0 + dist(click, (cx, cy)) / r);
    Some((HoleFit { axis: Frame { origin, z, x: basis(z).0 }, diameter: 2.0 * r }, score))
}

/// A wall vertex in the hole's plane, with its wall normal in the same plane and its 3D position.
type WallPoint = ((f64, f64), (f64, f64), DVec3);

fn inliers(
    pts: &[WallPoint],
    (cx, cy, r): (f64, f64, f64),
) -> impl Iterator<Item = &WallPoint> {
    pts.iter().filter(move |(p, n, _)| {
        let d = dist(*p, (cx, cy));
        // Hole walls face the axis: the normal points from the wall toward the centre.
        (d - r).abs() < 0.04 * r + 0.03 && (n.0 * (cx - p.0) + n.1 * (cy - p.1)) > 0.0
    })
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The circle through three points: (cx, cy, r).
fn circle3(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> Option<(f64, f64, f64)> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (a.0 * a.0 + a.1 * a.1, b.0 * b.0 + b.1 * b.1, c.0 * c.0 + c.1 * c.1);
    let cx = (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d;
    let cy = (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d;
    Some((cx, cy, dist(a, (cx, cy))))
}

/// Least-squares circle (Kåsa): solves x² + y² + D x + E y + F = 0.
pub(crate) fn kasa(points: impl Iterator<Item = (f64, f64)>) -> Option<(f64, f64, f64)> {
    let (mut s, mut b) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
    for (x, y) in points {
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += row[i] * row[j];
            }
            b[i] += row[i] * rhs;
        }
    }
    let m = glam::DMat3::from_cols_array_2d(&s).transpose();
    if m.determinant().abs() < 1e-12 {
        return None;
    }
    let sol = m.inverse() * DVec3::from(b);
    let (cx, cy) = (-sol.x / 2.0, -sol.y / 2.0);
    let r2 = cx * cx + cy * cy - sol.z;
    (r2 > 0.0).then(|| (cx, cy, r2.sqrt()))
}

/// Σ area · n nᵀ.
pub(crate) fn scatter(normals: impl Iterator<Item = (DVec3, f64)>) -> glam::DMat3 {
    normals.fold(glam::DMat3::ZERO, |m, (n, a)| {
        m + glam::DMat3::from_cols(n * n.x, n * n.y, n * n.z) * a
    })
}

/// Eigenvector of the smallest eigenvalue: power iteration on tr(M)·I − M.
pub(crate) fn smallest_eigenvector(m: glam::DMat3, start: DVec3) -> DVec3 {
    let shifted = glam::DMat3::from_diagonal(DVec3::splat(m.x_axis.x + m.y_axis.y + m.z_axis.z)) - m;
    let mut v = start.normalize_or(DVec3::Z);
    for _ in 0..100 {
        let w = shifted * v;
        if w.length_squared() < 1e-30 {
            break;
        }
        v = w.normalize();
    }
    v
}

/// Three orthogonal directions spanning M's eigenvectors (approximate; used only as guesses).
fn eigenvectors(m: glam::DMat3) -> [DVec3; 3] {
    let a = smallest_eigenvector(m, DVec3::new(0.3, 0.5, 0.8));
    let (u, v) = basis(a);
    // Within the plane ⟂ a, the larger of the two remaining directions.
    let big = if (m * u).dot(u) >= (m * v).dot(v) { u } else { v };
    [a, big, a.cross(big)]
}

/// Two unit vectors perpendicular to `n` and to each other.
pub(crate) fn basis(n: DVec3) -> (DVec3, DVec3) {
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = n.cross(helper).normalize();
    (u, n.cross(u))
}

/// What a fitted hole most likely is: (connector kind, thread spec or "").
/// Kind: "thread", "bore" or "optic_bore".
pub fn suggest(diameter: f64) -> (&'static str, &'static str) {
    // Tap-drill and nominal diameters, as vendor models draw either.
    const THREADS: [(&str, f64); 6] =
        [("M4", 3.3), ("8-32", 3.5), ("M4", 4.0), ("8-32", 4.17), ("M6", 5.0), ("1/4-20", 5.1)];
    if diameter >= 12.0 {
        return ("optic_bore", "");
    }
    if diameter >= 6.0 {
        return ("bore", "");
    }
    let (spec, _) = THREADS.iter().min_by(|a, b| (a.1 - diameter).abs().total_cmp(&(b.1 - diameter).abs())).copied().unwrap_or(("M4", 0.0));
    ("thread", spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 30 × 30 × 10 mm plate with a Ø3.5 mm through hole along Z at (2, 1), 48 segments.
    fn plate() -> TriMesh {
        let mut m = TriMesh::default();
        let (seg, r, c, h) = (48, 1.75, glam::DVec2::new(2.0, 1.0), 10.0);
        let mut tri = |a: DVec3, b: DVec3, d: DVec3| {
            let base = m.positions.len() as u32;
            m.positions.extend([a, b, d]);
            m.indices.extend([base, base + 1, base + 2]);
        };
        for i in 0..seg {
            let ang = |k: usize| std::f64::consts::TAU * k as f64 / seg as f64;
            let p = |k: usize, z: f64| DVec3::new(c.x + r * ang(k).cos(), c.y + r * ang(k).sin(), z);
            // Wall, normals facing the axis (winding: inward).
            tri(p(i, 0.0), p(i, h), p(i + 1, 0.0));
            tri(p(i + 1, 0.0), p(i, h), p(i + 1, h));
            // Top and bottom faces: fans from the hole edge to the plate corners (rough, fine for a fit).
            let corner = |k: usize, z: f64| {
                let a = ang(k);
                DVec3::new(c.x + 15.0 * a.cos().signum(), c.y + 15.0 * a.sin().signum(), z)
            };
            tri(p(i, h), corner(i, h), p(i + 1, h));
            tri(p(i, 0.0), p(i + 1, 0.0), corner(i, 0.0));
        }
        m
    }

    #[test]
    fn fits_a_through_hole_from_the_top_face() {
        let mesh = plate();
        let fit = fit_hole(&mesh, DVec3::new(4.5, 1.0, 10.0), DVec3::Z).expect("a fit");
        assert!(fit.axis.z.angle_between(DVec3::Z) < 1f64.to_radians(), "axis {:?}", fit.axis.z);
        assert!((fit.diameter - 3.5).abs() < 0.1, "diameter {}", fit.diameter);
        assert!((fit.axis.origin - DVec3::new(2.0, 1.0, 10.0)).length() < 0.1, "origin {:?}", fit.axis.origin);
        assert_eq!(suggest(fit.diameter), ("thread", "8-32"));
    }

    #[test]
    fn fits_from_the_bottom_and_from_the_wall() {
        let mesh = plate();
        let below = fit_hole(&mesh, DVec3::new(4.5, 1.0, 0.0), -DVec3::Z).expect("a fit");
        assert!(below.axis.z.angle_between(-DVec3::Z) < 1f64.to_radians());
        assert!(below.axis.origin.z.abs() < 0.1);
        let wall = fit_hole(&mesh, DVec3::new(2.0 + 1.75, 1.0, 5.0), -DVec3::X).expect("a fit");
        assert!(wall.axis.z.cross(DVec3::Z).length() < 0.02);
        assert!((wall.diameter - 3.5).abs() < 0.1);
    }
}
