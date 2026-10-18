//! Point-in-polygon and distance queries.

use glam::DVec2;

use crate::polygon::orient;

/// Non-zero winding rule; points exactly on the boundary may go either way.
pub fn point_in_polygon(points: &[DVec2], p: DVec2) -> bool {
    let n = points.len();
    let mut winding = 0i64;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        if a.y <= p.y {
            if b.y > p.y && orient(a, b, p) > 0.0 {
                winding += 1;
            }
        } else if b.y <= p.y && orient(a, b, p) < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

pub fn distance_to_segment(a: DVec2, b: DVec2, p: DVec2) -> f64 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 == 0.0 {
        return a.distance(p);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Distance to the nearest edge of the closed loop; infinity for an empty slice.
pub fn distance_to_boundary(points: &[DVec2], p: DVec2) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| distance_to_segment(points[i], points[(i + 1) % n], p))
        .fold(f64::INFINITY, f64::min)
}

/// 0 inside the polygon, otherwise [`distance_to_boundary`].
pub fn distance_to_region(points: &[DVec2], p: DVec2) -> f64 {
    if point_in_polygon(points, p) {
        0.0
    } else {
        distance_to_boundary(points, p)
    }
}

/// Clears `out` and fills it with the x of every edge crossing the line `y`, sorted ascending.
/// An edge from `y0` to `y1` counts when `y0 <= y < y1` or `y1 <= y < y0` (half-open), so a
/// vertex is counted once per side it continues to and horizontal edges never count.
pub fn horizontal_crossings(points: &[DVec2], y: f64, out: &mut Vec<f64>) {
    out.clear();
    let n = points.len();
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        if (a.y <= y && y < b.y) || (b.y <= y && y < a.y) {
            out.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
        }
    }
    out.sort_by(f64::total_cmp);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn l_shape() -> Vec<DVec2> {
        vec![
            v(0.0, 0.0),
            v(200.0, 0.0),
            v(200.0, 100.0),
            v(100.0, 100.0),
            v(100.0, 200.0),
            v(0.0, 200.0),
        ]
    }

    #[test]
    fn point_in_polygon_both_orientations() {
        let l = l_shape();
        let mut cw = l.clone();
        cw.reverse();
        for poly in [&l, &cw] {
            assert!(point_in_polygon(poly, v(50.0, 50.0)));
            assert!(point_in_polygon(poly, v(150.0, 50.0)));
            assert!(point_in_polygon(poly, v(50.0, 150.0)));
            assert!(!point_in_polygon(poly, v(150.0, 150.0))); // the notch
            assert!(!point_in_polygon(poly, v(-1.0, 50.0)));
            assert!(!point_in_polygon(poly, v(50.0, 250.0)));
        }
    }

    #[test]
    fn segment_distances() {
        let a = v(0.0, 0.0);
        let b = v(10.0, 0.0);
        assert_eq!(distance_to_segment(a, b, v(5.0, 3.0)), 3.0);
        assert_eq!(distance_to_segment(a, b, v(-3.0, 4.0)), 5.0);
        assert_eq!(distance_to_segment(a, b, v(13.0, -4.0)), 5.0);
        assert_eq!(distance_to_segment(a, a, v(3.0, 4.0)), 5.0);
    }

    #[test]
    fn boundary_and_region_distances() {
        let l = l_shape();
        assert_eq!(distance_to_boundary(&l, v(50.0, 50.0)), 50.0);
        assert_eq!(distance_to_boundary(&l, v(150.0, 80.0)), 20.0);
        // In the notch: 30 to the notch wall x = 100, 40 to the notch floor y = 100.
        assert_eq!(distance_to_boundary(&l, v(130.0, 140.0)), 30.0);
        assert_eq!(distance_to_boundary(&l, v(103.0, 104.0)), 3.0);
        assert_eq!(distance_to_region(&l, v(50.0, 50.0)), 0.0);
        assert_eq!(distance_to_region(&l, v(103.0, 104.0)), 3.0);
        assert_eq!(distance_to_region(&l, v(-30.0, -40.0)), 50.0);
        assert_eq!(distance_to_boundary(&[], v(0.0, 0.0)), f64::INFINITY);
    }

    #[test]
    fn crossings_through_a_vertex() {
        let diamond = vec![v(0.0, -10.0), v(10.0, 0.0), v(0.0, 10.0), v(-10.0, 0.0)];
        let mut out = vec![99.0];
        // Through the two side vertices: each counted once.
        horizontal_crossings(&diamond, 0.0, &mut out);
        assert_eq!(out, vec![-10.0, 10.0]);
        // Through the top vertex: both edges end there, so none count.
        horizontal_crossings(&diamond, 10.0, &mut out);
        assert!(out.is_empty());
        // Through the bottom vertex: both edges start there.
        horizontal_crossings(&diamond, -10.0, &mut out);
        assert_eq!(out, vec![0.0, 0.0]);
        horizontal_crossings(&diamond, 5.0, &mut out);
        assert_eq!(out, vec![-5.0, 5.0]);
    }

    #[test]
    fn crossings_along_a_horizontal_edge() {
        let l = l_shape();
        let mut out = Vec::new();
        // y = 100 runs along the notch floor (100,100)-(200,100): the horizontal edge never
        // counts; the edge going up from (100,100) does, the right edge ending at y=100 does not.
        horizontal_crossings(&l, 100.0, &mut out);
        assert_eq!(out, vec![0.0, 100.0]);
        // y = 0 runs along the bottom edge: the vertical edges starting there count.
        horizontal_crossings(&l, 0.0, &mut out);
        assert_eq!(out, vec![0.0, 200.0]);
        horizontal_crossings(&l, 50.0, &mut out);
        assert_eq!(out, vec![0.0, 200.0]);
        horizontal_crossings(&l, 150.0, &mut out);
        assert_eq!(out, vec![0.0, 100.0]);
    }
}
