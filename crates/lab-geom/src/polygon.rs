//! Simple-polygon validation and basic polygon measures.

use glam::DVec2;

/// Limits applied by [`validate_simple_polygon`].
#[derive(Clone, Copy, Debug)]
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
    let _ = points;
    unimplemented!()
}

/// `(min, max)` of the points; `(INF, -INF)` for an empty slice.
pub fn bounds(points: &[DVec2]) -> (DVec2, DVec2) {
    let _ = points;
    unimplemented!()
}

/// `(b - a).perp_dot(c - a)`: positive when `a, b, c` turn left.
pub fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    let _ = (a, b, c);
    unimplemented!()
}

/// Whether the closed segments `ab` and `cd` share a point (collinear overlap included).
pub fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let _ = (a, b, c, d);
    unimplemented!()
}

pub fn validate_simple_polygon(
    points: &[DVec2],
    limits: &PolygonLimits,
) -> Result<(), PolygonError> {
    let _ = (points, limits);
    unimplemented!()
}

/// Interior angle at `corner` in radians, in `(0, 2π)`, measured inside the polygon.
pub fn interior_angle(points: &[DVec2], corner: usize) -> f64 {
    let _ = (points, corner);
    unimplemented!()
}

/// Unit outward normal of edge `edge` (`p[edge] -> p[(edge + 1) % n]`).
pub fn outward_normal(points: &[DVec2], edge: usize) -> DVec2 {
    let _ = (points, edge);
    unimplemented!()
}
