//! Point-in-polygon and distance queries.

use glam::DVec2;

/// Non-zero winding rule; points exactly on the boundary may go either way.
pub fn point_in_polygon(points: &[DVec2], p: DVec2) -> bool {
    let _ = (points, p);
    unimplemented!()
}

pub fn distance_to_segment(a: DVec2, b: DVec2, p: DVec2) -> f64 {
    let _ = (a, b, p);
    unimplemented!()
}

pub fn distance_to_boundary(points: &[DVec2], p: DVec2) -> f64 {
    let _ = (points, p);
    unimplemented!()
}

/// 0 inside the polygon, otherwise [`distance_to_boundary`].
pub fn distance_to_region(points: &[DVec2], p: DVec2) -> f64 {
    let _ = (points, p);
    unimplemented!()
}

/// Clears `out` and fills it with the x of every edge crossing the line `y`, sorted ascending.
pub fn horizontal_crossings(points: &[DVec2], y: f64, out: &mut Vec<f64>) {
    let _ = (points, y, out);
    unimplemented!()
}
