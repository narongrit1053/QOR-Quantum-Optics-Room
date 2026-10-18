//! Simple-polygon validation and basic polygon measures.

use glam::DVec2;

/// Limits applied by [`validate_simple_polygon`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolygonLimits {
    pub min_corners: usize,
    pub max_corners: usize,
    pub min_edge: f64,
    pub max_abs_coord: f64,
    pub min_area: f64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PolygonError {
    #[error("needs at least {min} corners, has {count}")]
    TooFewCorners { count: usize, min: usize },
    #[error("has {count} corners; the maximum is {max}")]
    TooManyCorners { count: usize, max: usize },
    #[error("corner {} has an invalid coordinate", .corner + 1)]
    NonFinite { corner: usize },
    #[error("corner {} is too far from the origin", .corner + 1)]
    OutOfRange { corner: usize },
    #[error("edge {} is only {length:.2} mm long (minimum {min} mm)", .edge + 1)]
    EdgeTooShort { edge: usize, length: f64, min: f64 },
    #[error("corner {} folds back on itself", .corner + 1)]
    FoldBack { corner: usize },
    #[error("edges {} and {} cross", .a + 1, .b + 1)]
    SelfIntersecting { a: usize, b: usize },
    #[error("encloses no area")]
    ZeroArea,
    #[error("area {area:.0} mm² is below the minimum {min:.0} mm²")]
    TooSmallArea { area: f64, min: f64 },
}

/// Shoelace area; positive for counter-clockwise seen from +Z.
pub fn signed_area(points: &[DVec2]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    // Relative to the first point for better conditioning far from the origin.
    let o = points[0];
    let mut twice = 0.0;
    for i in 1..n - 1 {
        twice += (points[i] - o).perp_dot(points[i + 1] - o);
    }
    0.5 * twice
}

/// `(min, max)` of the points; `(INF, -INF)` for an empty slice.
pub fn bounds(points: &[DVec2]) -> (DVec2, DVec2) {
    points.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(lo, hi), &p| (lo.min(p), hi.max(p)),
    )
}

/// `(b - a).perp_dot(c - a)`: positive when `a, b, c` turn left (counter-clockwise).
pub fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

/// `r` lies in the axis-aligned box spanned by `p` and `q` (used for collinear points).
fn within_box(p: DVec2, q: DVec2, r: DVec2) -> bool {
    r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
}

/// Whether the closed segments `ab` and `cd` share a point (collinear overlap included).
pub fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let d1 = orient(c, d, a);
    let d2 = orient(c, d, b);
    let d3 = orient(a, b, c);
    let d4 = orient(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && within_box(c, d, a))
        || (d2 == 0.0 && within_box(c, d, b))
        || (d3 == 0.0 && within_box(a, b, c))
        || (d4 == 0.0 && within_box(a, b, d))
}

/// First pair `(i, j)`, `i < j`, of non-adjacent edges of the closed loop that touch or cross.
/// Edge `i` runs from `points[i]` to `points[(i + 1) % n]`.
pub(crate) fn first_self_intersection(points: &[DVec2]) -> Option<(usize, usize)> {
    let n = points.len();
    if n < 4 {
        return None;
    }
    let edge_box = |i: usize| {
        let a = points[i];
        let b = points[(i + 1) % n];
        (a.min(b), a.max(b))
    };
    for i in 0..n {
        let (lo_i, hi_i) = edge_box(i);
        let a = points[i];
        let b = points[(i + 1) % n];
        for j in (i + 2)..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (lo_j, hi_j) = edge_box(j);
            if lo_j.x > hi_i.x || lo_i.x > hi_j.x || lo_j.y > hi_i.y || lo_i.y > hi_j.y {
                continue;
            }
            if segments_intersect(a, b, points[j], points[(j + 1) % n]) {
                return Some((i, j));
            }
        }
    }
    None
}

/// Checks, in order: corner count, finite coordinates, coordinate range, edge length,
/// fold-back corners, self-intersection, and area. Collinear same-direction corners are allowed.
pub fn validate_simple_polygon(
    points: &[DVec2],
    limits: &PolygonLimits,
) -> Result<(), PolygonError> {
    let n = points.len();
    if n < limits.min_corners {
        return Err(PolygonError::TooFewCorners {
            count: n,
            min: limits.min_corners,
        });
    }
    if n > limits.max_corners {
        return Err(PolygonError::TooManyCorners {
            count: n,
            max: limits.max_corners,
        });
    }
    if n < 3 {
        // A loop needs three corners whatever the caller's limits say.
        return Err(PolygonError::TooFewCorners { count: n, min: 3 });
    }
    if let Some(corner) = points.iter().position(|p| !p.is_finite()) {
        return Err(PolygonError::NonFinite { corner });
    }
    if let Some(corner) = points
        .iter()
        .position(|p| p.x.abs() > limits.max_abs_coord || p.y.abs() > limits.max_abs_coord)
    {
        return Err(PolygonError::OutOfRange { corner });
    }
    for edge in 0..n {
        let length = points[edge].distance(points[(edge + 1) % n]);
        if length < limits.min_edge || length == 0.0 {
            return Err(PolygonError::EdgeTooShort {
                edge,
                length,
                min: limits.min_edge,
            });
        }
    }
    for corner in 0..n {
        let a = points[corner] - points[(corner + n - 1) % n];
        let b = points[(corner + 1) % n] - points[corner];
        let cross = a.perp_dot(b);
        if cross.abs() <= 1e-12 * a.length() * b.length() && a.dot(b) < 0.0 {
            return Err(PolygonError::FoldBack { corner });
        }
    }
    if let Some((a, b)) = first_self_intersection(points) {
        return Err(PolygonError::SelfIntersecting { a, b });
    }
    let area = signed_area(points).abs();
    if area == 0.0 {
        return Err(PolygonError::ZeroArea);
    }
    if area < limits.min_area {
        return Err(PolygonError::TooSmallArea {
            area,
            min: limits.min_area,
        });
    }
    Ok(())
}

/// `+1.0` for counter-clockwise (or degenerate) loops, `-1.0` for clockwise ones.
pub(crate) fn orientation_sign(points: &[DVec2]) -> f64 {
    if signed_area(points) < 0.0 { -1.0 } else { 1.0 }
}

/// Counter-clockwise angle from `from` to `to`, in `[0, 2π)`.
fn ccw_angle(from: DVec2, to: DVec2) -> f64 {
    let a = from.perp_dot(to).atan2(from.dot(to));
    if a < 0.0 {
        a + std::f64::consts::TAU
    } else {
        a
    }
}

/// Interior angle at `corner` in radians, in `(0, 2π)`, measured inside the polygon.
/// Returns NaN for fewer than three points or an out-of-range corner.
pub fn interior_angle(points: &[DVec2], corner: usize) -> f64 {
    let n = points.len();
    if n < 3 || corner >= n {
        return f64::NAN;
    }
    let p = points[corner];
    let to_prev = points[(corner + n - 1) % n] - p;
    let to_next = points[(corner + 1) % n] - p;
    if orientation_sign(points) > 0.0 {
        // Interior on the left of travel: sweep from the next edge round to the previous one.
        ccw_angle(to_next, to_prev)
    } else {
        ccw_angle(to_prev, to_next)
    }
}

/// Unit outward normal of edge `edge` (`p[edge] -> p[(edge + 1) % n]`). Zero for a
/// zero-length edge, fewer than two points or an out-of-range edge.
pub fn outward_normal(points: &[DVec2], edge: usize) -> DVec2 {
    let n = points.len();
    if n < 2 || edge >= n {
        return DVec2::ZERO;
    }
    let d = (points[(edge + 1) % n] - points[edge]).normalize_or_zero();
    orientation_sign(points) * DVec2::new(d.y, -d.x)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn square() -> Vec<DVec2> {
        vec![v(0.0, 0.0), v(100.0, 0.0), v(100.0, 100.0), v(0.0, 100.0)]
    }

    #[test]
    fn signed_area_ccw_and_cw() {
        let ccw = square();
        let mut cw = ccw.clone();
        cw.reverse();
        assert_eq!(signed_area(&ccw), 10_000.0);
        assert_eq!(signed_area(&cw), -10_000.0);
        assert_eq!(signed_area(&[]), 0.0);
    }

    #[test]
    fn bounds_of_points_and_empty() {
        assert_eq!(bounds(&square()), (v(0.0, 0.0), v(100.0, 100.0)));
        let (lo, hi) = bounds(&[]);
        assert_eq!(lo, DVec2::splat(f64::INFINITY));
        assert_eq!(hi, DVec2::splat(f64::NEG_INFINITY));
    }

    #[test]
    fn segment_intersection_cases() {
        // Proper crossing.
        assert!(segments_intersect(
            v(0.0, 0.0),
            v(2.0, 2.0),
            v(0.0, 2.0),
            v(2.0, 0.0)
        ));
        // Touching at an endpoint.
        assert!(segments_intersect(
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(1.0, 0.0),
            v(1.0, 1.0)
        ));
        // Collinear overlap and collinear disjoint.
        assert!(segments_intersect(
            v(0.0, 0.0),
            v(2.0, 0.0),
            v(1.0, 0.0),
            v(3.0, 0.0)
        ));
        assert!(!segments_intersect(
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(2.0, 0.0),
            v(3.0, 0.0)
        ));
        // Parallel apart.
        assert!(!segments_intersect(
            v(0.0, 0.0),
            v(1.0, 0.0),
            v(0.0, 1.0),
            v(1.0, 1.0)
        ));
    }

    #[test]
    fn valid_square_and_both_orientations() {
        assert_eq!(validate_simple_polygon(&square(), &LIMITS), Ok(()));
        let mut cw = square();
        cw.reverse();
        assert_eq!(validate_simple_polygon(&cw, &LIMITS), Ok(()));
    }

    #[test]
    fn collinear_corner_is_allowed() {
        let p = vec![
            v(0.0, 0.0),
            v(50.0, 0.0),
            v(100.0, 0.0),
            v(100.0, 100.0),
            v(0.0, 100.0),
        ];
        assert_eq!(validate_simple_polygon(&p, &LIMITS), Ok(()));
        assert!((interior_angle(&p, 1) - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn too_few_and_too_many_corners() {
        assert_eq!(
            validate_simple_polygon(&[v(0.0, 0.0), v(100.0, 0.0)], &LIMITS),
            Err(PolygonError::TooFewCorners { count: 2, min: 3 })
        );
        let many: Vec<DVec2> = (0..129)
            .map(|i| {
                let a = i as f64 / 129.0 * std::f64::consts::TAU;
                v(1000.0 * a.cos(), 1000.0 * a.sin())
            })
            .collect();
        assert_eq!(
            validate_simple_polygon(&many, &LIMITS),
            Err(PolygonError::TooManyCorners {
                count: 129,
                max: 128
            })
        );
    }

    #[test]
    fn nan_coordinate() {
        let mut p = square();
        p[2].x = f64::NAN;
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::NonFinite { corner: 2 })
        );
    }

    #[test]
    fn coordinate_out_of_range() {
        let mut p = square();
        p[1].x = 1e6;
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::OutOfRange { corner: 1 })
        );
    }

    #[test]
    fn zero_edge() {
        let p = vec![v(0.0, 0.0), v(100.0, 0.0), v(100.0, 0.0), v(0.0, 100.0)];
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::EdgeTooShort {
                edge: 1,
                length: 0.0,
                min: 1.0
            })
        );
    }

    #[test]
    fn spike_folds_back() {
        // A needle sticking up from the top edge: corner 5 at (50, 150) reverses direction.
        let p = vec![
            v(0.0, 0.0),
            v(100.0, 0.0),
            v(100.0, 100.0),
            v(60.0, 100.0),
            v(50.0, 100.0),
            v(50.0, 150.0),
            v(50.0, 120.0),
            v(0.0, 100.0),
        ];
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::FoldBack { corner: 5 })
        );
    }

    #[test]
    fn three_collinear_points_fold_back() {
        // Three collinear points reverse at both ends (corners 0 and 2), which the fold-back
        // check reports before the area check would.
        let p = vec![v(0.0, 0.0), v(100.0, 0.0), v(200.0, 0.0)];
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::FoldBack { corner: 0 })
        );
    }

    #[test]
    fn bow_tie_self_intersects() {
        let p = vec![v(0.0, 0.0), v(100.0, 100.0), v(100.0, 0.0), v(0.0, 100.0)];
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::SelfIntersecting { a: 0, b: 2 })
        );
    }

    #[test]
    fn zero_area() {
        // A simple polygon always has a non-zero exact area, so this needs float underflow:
        // legs of 2^-537 give twice-area 2^-1074 (the smallest subnormal), and halving it
        // rounds to zero. Every edge length and corner cross product stays non-zero.
        let tiny = PolygonLimits {
            min_edge: 0.0,
            min_area: 0.0,
            ..LIMITS
        };
        let leg = f64::from_bits((1023 - 537) << 52);
        let p = vec![v(0.0, 0.0), v(leg, 0.0), v(0.0, leg)];
        assert_eq!(
            validate_simple_polygon(&p, &tiny),
            Err(PolygonError::ZeroArea)
        );
    }

    #[test]
    fn too_small_area() {
        let p = vec![v(0.0, 0.0), v(40.0, 0.0), v(40.0, 40.0), v(0.0, 40.0)];
        assert_eq!(
            validate_simple_polygon(&p, &LIMITS),
            Err(PolygonError::TooSmallArea {
                area: 1600.0,
                min: 2500.0
            })
        );
    }

    #[test]
    fn error_messages_are_one_based() {
        let e = PolygonError::SelfIntersecting { a: 0, b: 2 };
        assert_eq!(e.to_string(), "edges 1 and 3 cross");
        let e = PolygonError::EdgeTooShort {
            edge: 1,
            length: 0.5,
            min: 1.0,
        };
        assert_eq!(e.to_string(), "edge 2 is only 0.50 mm long (minimum 1 mm)");
    }

    #[test]
    fn interior_angles_and_normals_are_orientation_aware() {
        use std::f64::consts::FRAC_PI_2;
        // L shape, CCW: the reflex corner is index 3.
        let l = vec![
            v(0.0, 0.0),
            v(200.0, 0.0),
            v(200.0, 100.0),
            v(100.0, 100.0),
            v(100.0, 200.0),
            v(0.0, 200.0),
        ];
        let mut cw = l.clone();
        cw.reverse(); // reflex corner is now index 2
        assert!((interior_angle(&l, 0) - FRAC_PI_2).abs() < 1e-12);
        assert!((interior_angle(&l, 3) - 3.0 * FRAC_PI_2).abs() < 1e-12);
        assert!((interior_angle(&cw, 2) - 3.0 * FRAC_PI_2).abs() < 1e-12);
        assert!((interior_angle(&cw, 0) - FRAC_PI_2).abs() < 1e-12);
        // Edge 0 is the bottom edge in the CCW list: outward is -Y.
        assert_eq!(outward_normal(&l, 0), v(0.0, -1.0));
        // In the CW list, edge 4 runs (200,0) -> (0,0): still the bottom edge.
        assert_eq!(outward_normal(&cw, 4), v(0.0, -1.0));
    }
}
