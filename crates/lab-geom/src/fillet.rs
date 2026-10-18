//! Corner fillets flattened into a closed polyline.

use std::f64::consts::PI;

use glam::DVec2;

use crate::polygon::{
    PolygonError, PolygonLimits, first_self_intersection, validate_simple_polygon,
};

/// Upper bound on segments per arc (lab-core `ARC_MAX_SEGMENTS`).
const ARC_MAX_SEGMENTS: usize = 64;
/// Consecutive output points closer than this are merged (tangent points of touching arcs).
const MERGE_DISTANCE: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilletCorner {
    pub point: DVec2,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlatPath {
    /// Flattened closed loop, same orientation as the input.
    pub points: Vec<DVec2>,
    /// `true` = arc-interior vertex (side walls share its normal).
    pub smooth: Vec<bool>,
    /// Input corner that produced each point.
    pub source_corner: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FilletError {
    #[error("corner {} radius cannot be negative", .corner + 1)]
    NegativeRadius { corner: usize },
    #[error("corner {} radius is too large; the maximum here is {max_radius:.1} mm", .corner + 1)]
    TooLarge { corner: usize, max_radius: f64 },
    #[error("{0}")]
    Polygon(PolygonError),
}

/// Per-corner fillet geometry.
#[derive(Clone, Copy)]
struct Corner {
    p: DVec2,
    /// Unit vectors from the corner towards the previous and the next corner.
    u: DVec2,
    v: DVec2,
    /// Angle between `u` and `v`, in `[0, π]`.
    theta: f64,
    radius: f64,
    /// Tangent length; 0 for a sharp corner.
    t: f64,
    sharp: bool,
}

/// Rounds each corner with its radius and flattens the arcs so that no chord deviates from the
/// arc by more than `tolerance`, using at most `max_points` points (arcs keep at least two
/// segments each, so a budget below `3 · arcs + sharp corners` cannot be met).
///
/// A collinear corner, or one with radius 0, stays sharp. Fillets at reflex corners are concave.
pub fn flatten_fillets(
    corners: &[FilletCorner],
    tolerance: f64,
    max_points: usize,
    limits: &PolygonLimits,
) -> Result<FlatPath, FilletError> {
    let pts: Vec<DVec2> = corners.iter().map(|c| c.point).collect();
    validate_simple_polygon(&pts, limits).map_err(FilletError::Polygon)?;
    if let Some(corner) = corners.iter().position(|c| c.radius < 0.0) {
        return Err(FilletError::NegativeRadius { corner });
    }
    let n = pts.len();

    let geo: Vec<Corner> = (0..n)
        .map(|i| {
            let p = pts[i];
            let u = (pts[(i + n - 1) % n] - p).normalize_or_zero();
            let v = (pts[(i + 1) % n] - p).normalize_or_zero();
            let theta = u.dot(v).clamp(-1.0, 1.0).acos();
            let radius = corners[i].radius;
            let sharp = radius == 0.0 || theta >= PI - 1e-9;
            let t = if sharp {
                0.0
            } else {
                radius / (theta / 2.0).tan()
            };
            Corner {
                p,
                u,
                v,
                theta,
                radius,
                t,
                sharp,
            }
        })
        .collect();

    // Fit check per edge: both tangent lengths must fit on the edge.
    for e in 0..n {
        let j = (e + 1) % n;
        let length = pts[e].distance(pts[j]);
        let (te, tj) = (geo[e].t, geo[j].t);
        if te + tj <= length + 1e-9 {
            continue;
        }
        let pick_j = tj.is_nan() || (!te.is_nan() && tj > te);
        let (corner, neighbour_t) = if pick_j { (j, te) } else { (e, tj) };
        let max = (length - neighbour_t) * (geo[corner].theta / 2.0).tan();
        let max_radius = if max.is_finite() && max > 0.0 {
            max
        } else {
            0.0
        };
        return Err(FilletError::TooLarge { corner, max_radius });
    }

    // Segments per arc, then scale down proportionally if the point budget is exceeded.
    let mut segments: Vec<usize> = geo
        .iter()
        .map(|c| {
            if c.sharp {
                return 0;
            }
            let step = 2.0 * (1.0 - tolerance / c.radius).clamp(-1.0, 1.0).acos();
            let wanted = (PI - c.theta) / step;
            if wanted.is_finite() {
                (wanted.ceil() as usize).clamp(2, ARC_MAX_SEGMENTS)
            } else {
                ARC_MAX_SEGMENTS
            }
        })
        .collect();
    let arcs = segments.iter().filter(|&&s| s > 0).count();
    let sharp_count = n - arcs;
    let total: usize = sharp_count
        + segments
            .iter()
            .filter(|&&s| s > 0)
            .map(|s| s + 1)
            .sum::<usize>();
    if total > max_points && arcs > 0 {
        let available = max_points.saturating_sub(sharp_count + arcs) as f64;
        let sum: usize = segments.iter().sum();
        let factor = available / sum as f64;
        for s in segments.iter_mut().filter(|s| **s > 0) {
            *s = ((*s as f64 * factor).floor() as usize).max(2);
        }
    }

    // Emit the loop. `arc_end` marks arc start/end points, used when merging touching arcs.
    let mut points: Vec<DVec2> = Vec::with_capacity(total.min(max_points.max(n)));
    let mut smooth: Vec<bool> = Vec::with_capacity(points.capacity());
    let mut source: Vec<u32> = Vec::with_capacity(points.capacity());
    let mut arc_end: Vec<bool> = Vec::with_capacity(points.capacity());
    let mut push = |p: DVec2, is_smooth: bool, src: usize, is_end: bool| {
        if let Some(&last) = points.last()
            && last.distance(p) <= MERGE_DISTANCE
        {
            let k = points.len() - 1;
            // Two arcs meeting at a shared tangent point continue smoothly.
            smooth[k] = arc_end[k] && is_end;
            arc_end[k] = false;
            return;
        }
        points.push(p);
        smooth.push(is_smooth);
        source.push(src as u32);
        arc_end.push(is_end);
    };
    for (i, c) in geo.iter().enumerate() {
        if c.sharp {
            push(c.p, false, i, false);
            continue;
        }
        let t1 = c.p + c.u * c.t;
        let t2 = c.p + c.v * c.t;
        let centre = c.p + (c.u + c.v).normalize_or_zero() * (c.radius / (c.theta / 2.0).sin());
        let a1 = t1 - centre;
        let a2 = t2 - centre;
        let sweep = a1.perp_dot(a2).atan2(a1.dot(a2));
        let segs = segments[i];
        push(t1, false, i, true);
        for k in 1..segs {
            let (s, co) = (sweep * k as f64 / segs as f64).sin_cos();
            push(
                centre + DVec2::new(co * a1.x - s * a1.y, s * a1.x + co * a1.y),
                true,
                i,
                false,
            );
        }
        push(t2, false, i, true);
    }
    // Close the loop: merge the last point into the first if they coincide.
    if points.len() > 1 {
        let last = points.len() - 1;
        if points[last].distance(points[0]) <= MERGE_DISTANCE {
            smooth[0] = arc_end[last] && arc_end[0];
            points.pop();
            smooth.pop();
            source.pop();
        }
    }

    if let Some((a, b)) = first_self_intersection(&points) {
        let (a, b) = (source[a] as usize, source[b] as usize);
        return Err(FilletError::Polygon(PolygonError::SelfIntersecting {
            a: a.min(b),
            b: a.max(b),
        }));
    }
    Ok(FlatPath {
        points,
        smooth,
        source_corner: source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polygon::signed_area;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    const LIMITS: PolygonLimits = PolygonLimits {
        min_corners: 3,
        max_corners: 128,
        min_edge: 1.0,
        max_abs_coord: 100_000.0,
        min_area: 2_500.0,
    };

    fn with_radii(points: &[DVec2], radii: &[f64]) -> Vec<FilletCorner> {
        points
            .iter()
            .zip(radii)
            .map(|(&point, &radius)| FilletCorner { point, radius })
            .collect()
    }

    fn rect(w: f64, h: f64) -> Vec<DVec2> {
        let (hx, hy) = (w / 2.0, h / 2.0);
        vec![v(-hx, -hy), v(hx, -hy), v(hx, hy), v(-hx, hy)]
    }

    #[test]
    fn sharp_corners_are_unchanged() {
        let pts = rect(1200.0, 900.0);
        let path =
            flatten_fillets(&with_radii(&pts, &[0.0; 4]), 0.02, 2048, &LIMITS).expect("flatten");
        assert_eq!(path.points, pts);
        assert_eq!(path.smooth, vec![false; 4]);
        assert_eq!(path.source_corner, vec![0, 1, 2, 3]);
    }

    #[test]
    fn rounded_rectangle_has_four_arcs_within_tolerance() {
        let (r, tol) = (50.0, 0.02);
        let pts = rect(1200.0, 900.0);
        let path =
            flatten_fillets(&with_radii(&pts, &[r; 4]), tol, 2048, &LIMITS).expect("flatten");
        let n = path.points.len();
        assert_eq!(path.smooth.len(), n);
        assert_eq!(path.source_corner.len(), n);
        // Four arcs: every corner contributes points, and arcs have interior points.
        for corner in 0..4u32 {
            let interior = (0..n)
                .filter(|&k| path.source_corner[k] == corner && path.smooth[k])
                .count();
            assert!(interior >= 1, "corner {corner} has no arc interior points");
        }
        // Start and end of each arc are the only non-smooth points.
        assert_eq!(path.smooth.iter().filter(|s| !**s).count(), 8);
        let centre = |c: u32| {
            let p = pts[c as usize];
            p - DVec2::new(p.x.signum(), p.y.signum()) * r
        };
        for k in 0..n {
            let c = centre(path.source_corner[k]);
            let d = path.points[k].distance(c);
            if path.smooth[k] {
                assert!((d - r).abs() <= 1e-9, "point {k} at {d}");
            }
            // Chord midpoints within the tolerance of the arc.
            let next = (k + 1) % n;
            if path.source_corner[next] == path.source_corner[k] {
                let mid = 0.5 * (path.points[k] + path.points[next]);
                let dm = mid.distance(c);
                assert!(
                    dm >= r - tol - 1e-9 && dm <= r + 1e-9,
                    "chord {k} midpoint at {dm}"
                );
            }
        }
        assert!(signed_area(&path.points) > 0.0);
    }

    #[test]
    fn clockwise_input_stays_clockwise() {
        let mut pts = rect(1200.0, 900.0);
        pts.reverse();
        let path =
            flatten_fillets(&with_radii(&pts, &[50.0; 4]), 0.02, 2048, &LIMITS).expect("flatten");
        assert!(signed_area(&path.points) < 0.0);
    }

    #[test]
    fn too_large_reports_corner_and_max_radius() {
        let pts = vec![v(0.0, 0.0), v(1000.0, 0.0), v(1000.0, 500.0), v(0.0, 500.0)];
        let err = flatten_fillets(
            &with_radii(&pts, &[0.0, 300.0, 350.0, 0.0]),
            0.02,
            2048,
            &LIMITS,
        )
        .expect_err("must not fit");
        match err {
            FilletError::TooLarge { corner, max_radius } => {
                assert_eq!(corner, 2);
                assert!((max_radius - 200.0).abs() < 1e-9, "max_radius {max_radius}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            FilletError::TooLarge {
                corner: 2,
                max_radius: 200.0
            }
            .to_string(),
            "corner 3 radius is too large; the maximum here is 200.0 mm"
        );
        // Exactly the maximum fits.
        assert!(
            flatten_fillets(
                &with_radii(&pts, &[0.0, 300.0, 200.0, 0.0]),
                0.02,
                2048,
                &LIMITS
            )
            .is_ok()
        );
    }

    #[test]
    fn negative_and_nan_radius() {
        let pts = rect(1000.0, 500.0);
        assert_eq!(
            flatten_fillets(
                &with_radii(&pts, &[0.0, -1.0, 0.0, 0.0]),
                0.02,
                2048,
                &LIMITS
            ),
            Err(FilletError::NegativeRadius { corner: 1 })
        );
        assert!(matches!(
            flatten_fillets(
                &with_radii(&pts, &[0.0, f64::NAN, 0.0, 0.0]),
                0.02,
                2048,
                &LIMITS
            ),
            Err(FilletError::TooLarge { corner: 1, .. })
        ));
    }

    #[test]
    fn invalid_points_are_polygon_errors() {
        let pts = vec![v(0.0, 0.0), v(100.0, 100.0), v(100.0, 0.0), v(0.0, 100.0)];
        assert_eq!(
            flatten_fillets(&with_radii(&pts, &[0.0; 4]), 0.02, 2048, &LIMITS),
            Err(FilletError::Polygon(PolygonError::SelfIntersecting {
                a: 0,
                b: 2
            }))
        );
    }

    #[test]
    fn collinear_corner_with_radius_stays_sharp() {
        let pts = vec![
            v(0.0, 0.0),
            v(500.0, 0.0),
            v(1000.0, 0.0),
            v(1000.0, 500.0),
            v(0.0, 500.0),
        ];
        let path = flatten_fillets(
            &with_radii(&pts, &[0.0, 80.0, 0.0, 0.0, 0.0]),
            0.02,
            2048,
            &LIMITS,
        )
        .expect("flatten");
        assert_eq!(path.points, pts);
    }

    #[test]
    fn concave_fillet_at_reflex_corner_of_l() {
        let r = 100.0;
        let l = vec![
            v(0.0, 0.0),
            v(2000.0, 0.0),
            v(2000.0, 1000.0),
            v(1000.0, 1000.0), // reflex corner
            v(1000.0, 2000.0),
            v(0.0, 2000.0),
        ];
        let mut radii = [0.0; 6];
        radii[3] = r;
        let path = flatten_fillets(&with_radii(&l, &radii), 0.02, 2048, &LIMITS).expect("flatten");
        // The arc centre lies in the notch, at (1100, 1100), and the arc bulges towards the corner.
        let centre = v(1100.0, 1100.0);
        let arc: Vec<DVec2> = (0..path.points.len())
            .filter(|&k| path.source_corner[k] == 3)
            .map(|k| path.points[k])
            .collect();
        assert!(arc.len() > 3);
        for p in &arc {
            assert!((p.distance(centre) - r).abs() <= 1e-9);
        }
        assert_eq!(arc[0], v(1100.0, 1000.0));
        assert_eq!(*arc.last().expect("arc"), v(1000.0, 1100.0));
        // Material is added: the square corner minus a quarter disc, up to chord error.
        let added = signed_area(&path.points) - signed_area(&l);
        let exact = r * r * (1.0 - PI / 4.0);
        assert!(
            added > exact && added - exact < 5.0,
            "added {added}, exact {exact}"
        );
    }

    #[test]
    fn fillet_touching_another_edge_self_intersects() {
        // A thin wedge from the top edge reaches towards corner 2 (1000, 0); a big fillet there
        // cuts through the wedge.
        let pts = vec![
            v(0.0, 0.0),
            v(1000.0, 0.0),
            v(1000.0, 1000.0),
            v(620.0, 1000.0),
            v(950.0, 50.0),
            v(600.0, 1000.0),
            v(0.0, 1000.0),
        ];
        assert_eq!(validate_simple_polygon(&pts, &LIMITS), Ok(()));
        let mut radii = [0.0; 7];
        radii[1] = 400.0;
        match flatten_fillets(&with_radii(&pts, &radii), 0.02, 2048, &LIMITS) {
            Err(FilletError::Polygon(PolygonError::SelfIntersecting { a, b })) => {
                assert!(a < b);
                assert!(a == 1 && (b == 3 || b == 4), "edges {a}, {b}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn touching_arcs_merge_into_a_stadium() {
        // Radius = half the short side: the tangent points of neighbouring arcs coincide.
        let pts = rect(1000.0, 500.0);
        let path =
            flatten_fillets(&with_radii(&pts, &[250.0; 4]), 0.02, 2048, &LIMITS).expect("flatten");
        let n = path.points.len();
        for k in 0..n {
            assert!(
                path.points[k].distance(path.points[(k + 1) % n]) > 1e-3,
                "duplicate point {k}"
            );
        }
        // The two short-side midpoints are smooth junctions.
        let mids: Vec<usize> = (0..n).filter(|&k| path.points[k].y.abs() < 1e-9).collect();
        assert_eq!(mids.len(), 2);
        assert!(mids.iter().all(|&k| path.smooth[k]));
        assert_eq!(
            validate_simple_polygon(
                &path.points,
                &PolygonLimits {
                    min_edge: 0.0,
                    max_corners: 4096,
                    ..LIMITS
                }
            ),
            Ok(())
        );
    }

    #[test]
    fn point_budget_is_respected() {
        // 128 corners, every one rounded with r = 1000.
        let corners: Vec<FilletCorner> = (0..128)
            .map(|k| {
                let a = k as f64 / 128.0 * std::f64::consts::TAU;
                FilletCorner {
                    point: v(5000.0 * a.cos(), 5000.0 * a.sin()),
                    radius: 1000.0,
                }
            })
            .collect();
        for (tol, max_points) in [(1e-4, 2048), (0.02, 400), (0.02, 2048)] {
            let path = flatten_fillets(&corners, tol, max_points, &LIMITS).expect("flatten");
            assert!(
                path.points.len() <= max_points,
                "{} > {max_points}",
                path.points.len()
            );
            assert!(path.points.len() >= 128 * 3);
            for c in 0..128u32 {
                assert!(path.source_corner.iter().filter(|&&s| s == c).count() >= 3);
            }
        }
        // Unconstrained, the fine tolerance would need far more than the budget.
        let path = flatten_fillets(&corners, 1e-4, usize::MAX, &LIMITS).expect("flatten");
        assert!(path.points.len() > 2048);
    }
}
