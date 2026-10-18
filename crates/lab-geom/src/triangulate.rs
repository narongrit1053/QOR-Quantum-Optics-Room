//! Ear-clipping triangulation of simple polygons.

use glam::DVec2;

use crate::polygon::{bounds, orient, signed_area};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TriangulateError {
    #[error("degenerate polygon")]
    Degenerate,
    #[error("no ear found ({remaining} corners left)")]
    NoEarFound { remaining: usize },
}

/// Triangles CCW seen from +Z, as indices into `points`. Either input orientation is accepted.
///
/// Vertices with `|orient(prev, p, next)| <= 1e-12·scale²` (`scale` = bounding-box diagonal)
/// are dropped first and appear in no triangle; the result has `kept - 2` triangles.
pub fn triangulate(points: &[DVec2]) -> Result<Vec<[u32; 3]>, TriangulateError> {
    let kept = preprocess(points)?;
    ear_clip(points, kept)
}

/// Like [`triangulate`], but vertices dropped as collinear are put back by splitting the
/// triangle on the boundary edge they lie on, so every boundary vertex is used and meshes
/// built from the result have no T-junctions along the outline.
pub(crate) fn triangulate_all_vertices(
    points: &[DVec2],
) -> Result<Vec<[u32; 3]>, TriangulateError> {
    let kept = preprocess(points)?;
    let mut tris = ear_clip(points, kept.clone())?;
    if kept.len() == points.len() {
        return Ok(tris);
    }
    let n = points.len();
    let m = kept.len();
    for j in 0..m {
        let a = kept[j];
        let c = kept[(j + 1) % m];
        let run: Vec<usize> = (1..n)
            .map(|k| (a + k) % n)
            .take_while(|&k| k != c)
            .collect();
        if run.is_empty() {
            continue;
        }
        let (a32, c32) = (a as u32, c as u32);
        // Find the triangle holding the boundary edge a-c and its winding along that edge.
        let found = tris.iter().enumerate().find_map(|(t, tri)| {
            (0..3).find_map(|e| {
                let (p, q, x) = (tri[e], tri[(e + 1) % 3], tri[(e + 2) % 3]);
                if p == a32 && q == c32 {
                    Some((t, true, x))
                } else if p == c32 && q == a32 {
                    Some((t, false, x))
                } else {
                    None
                }
            })
        });
        let Some((t, forward, x)) = found else {
            continue;
        };
        let mut chain: Vec<u32> = Vec::with_capacity(run.len() + 2);
        chain.push(a32);
        chain.extend(run.iter().map(|&k| k as u32));
        chain.push(c32);
        if !forward {
            chain.reverse();
        }
        tris[t] = [chain[0], chain[1], x];
        for w in chain.windows(2).skip(1) {
            tris.push([w[0], w[1], x]);
        }
    }
    Ok(tris)
}

/// Indices of the vertices that survive collinear removal, in loop order.
fn preprocess(points: &[DVec2]) -> Result<Vec<usize>, TriangulateError> {
    if points.len() < 3 || points.iter().any(|p| !p.is_finite()) {
        return Err(TriangulateError::Degenerate);
    }
    let (lo, hi) = bounds(points);
    let scale = (hi - lo).length();
    if !(scale > 0.0 && scale.is_finite()) {
        return Err(TriangulateError::Degenerate);
    }
    let tol = 1e-12 * scale * scale;
    let mut idx: Vec<usize> = (0..points.len()).collect();
    loop {
        let m = idx.len();
        if m < 3 {
            return Err(TriangulateError::Degenerate);
        }
        let drop = (0..m).find(|&i| {
            let prev = points[idx[(i + m - 1) % m]];
            let next = points[idx[(i + 1) % m]];
            orient(prev, points[idx[i]], next).abs() <= tol
        });
        match drop {
            Some(i) => {
                idx.remove(i);
            }
            None => return Ok(idx),
        }
    }
}

fn in_triangle_inclusive(a: DVec2, b: DVec2, c: DVec2, p: DVec2) -> bool {
    orient(a, b, p) >= 0.0 && orient(b, c, p) >= 0.0 && orient(c, a, p) >= 0.0
}

/// Whether the vertex at list position `i` of the CCW list `idx` is an ear.
fn is_ear(points: &[DVec2], idx: &[usize], i: usize) -> bool {
    let m = idx.len();
    let ia = (i + m - 1) % m;
    let ic = (i + 1) % m;
    let (a, b, c) = (points[idx[ia]], points[idx[i]], points[idx[ic]]);
    if orient(a, b, c) <= 0.0 {
        return false;
    }
    for j in 0..m {
        if j == ia || j == i || j == ic {
            continue;
        }
        let p = points[idx[j]];
        let prev = points[idx[(j + m - 1) % m]];
        let next = points[idx[(j + 1) % m]];
        // Only non-convex vertices can block an ear of a simple polygon.
        if orient(prev, p, next) <= 0.0 && in_triangle_inclusive(a, b, c, p) {
            return false;
        }
    }
    true
}

fn ear_clip(points: &[DVec2], mut idx: Vec<usize>) -> Result<Vec<[u32; 3]>, TriangulateError> {
    let ccw_area = {
        let loop_points: Vec<DVec2> = idx.iter().map(|&i| points[i]).collect();
        signed_area(&loop_points)
    };
    if ccw_area == 0.0 || !ccw_area.is_finite() {
        return Err(TriangulateError::Degenerate);
    }
    if ccw_area < 0.0 {
        idx.reverse();
    }
    let tri = |idx: &[usize], i: usize| {
        let m = idx.len();
        [
            idx[(i + m - 1) % m] as u32,
            idx[i] as u32,
            idx[(i + 1) % m] as u32,
        ]
    };
    let mut out = Vec::with_capacity(idx.len().saturating_sub(2));
    let mut i = 0usize;
    let mut misses = 0usize;
    while idx.len() > 3 {
        let m = idx.len();
        if i >= m {
            i = 0;
        }
        if is_ear(points, &idx, i) {
            out.push(tri(&idx, i));
            idx.remove(i);
            misses = 0;
            // The previous vertex may have just become an ear.
            i = if i == 0 { idx.len() - 1 } else { i - 1 };
            continue;
        }
        i += 1;
        misses += 1;
        if misses >= m {
            // No ear in a full pass (numerically awkward input): clip the convex vertex with
            // the largest triangle.
            let best = (0..m)
                .filter_map(|k| {
                    let [a, b, c] = tri(&idx, k).map(|v| points[v as usize]);
                    let area = orient(a, b, c);
                    (area > 0.0).then_some((k, area))
                })
                .max_by(|x, y| x.1.total_cmp(&y.1));
            let Some((k, _)) = best else {
                return Err(TriangulateError::NoEarFound { remaining: m });
            };
            out.push(tri(&idx, k));
            idx.remove(k);
            misses = 0;
            i = k.saturating_sub(1);
        }
    }
    out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fillet::{FilletCorner, flatten_fillets};
    use crate::polygon::{PolygonLimits, validate_simple_polygon};
    use std::f64::consts::TAU;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    /// In-test xorshift64 generator.
    pub(crate) struct XorShift(u64);
    impl XorShift {
        pub(crate) fn new(seed: u64) -> Self {
            XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }
        pub(crate) fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        /// Uniform in [0, 1).
        pub(crate) fn unit(&mut self) -> f64 {
            (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
        }
        pub(crate) fn range(&mut self, lo: usize, hi_inclusive: usize) -> usize {
            lo + (self.next_u64() % (hi_inclusive - lo + 1) as u64) as usize
        }
    }

    /// Checks count, CCW winding and area sum; returns the triangles.
    pub(crate) fn check(points: &[DVec2], expected_vertices: usize) -> Vec<[u32; 3]> {
        let tris = triangulate(points).expect("triangulation failed");
        assert_eq!(tris.len(), expected_vertices - 2, "triangle count");
        let (lo, hi) = bounds(points);
        let scale2 = (hi - lo).length_squared();
        let mut sum = 0.0;
        for t in &tris {
            let [a, b, c] = t.map(|i| points[i as usize]);
            let twice = orient(a, b, c);
            assert!(
                twice >= -1e-12 * scale2,
                "triangle {t:?} is clockwise ({twice})"
            );
            sum += 0.5 * twice;
        }
        let area = signed_area(points).abs();
        assert!(
            (sum - area).abs() <= 1e-9 * area,
            "area sum {sum} differs from polygon area {area}"
        );
        tris
    }

    /// Checks a polygon in both orientations.
    fn check_both(points: &[DVec2], expected_vertices: usize) {
        check(points, expected_vertices);
        let mut cw = points.to_vec();
        cw.reverse();
        check(&cw, expected_vertices);
    }

    #[test]
    fn square() {
        let p = vec![v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0), v(0.0, 10.0)];
        check_both(&p, 4);
    }

    #[test]
    fn l_and_u() {
        let l = vec![
            v(0.0, 0.0),
            v(200.0, 0.0),
            v(200.0, 100.0),
            v(100.0, 100.0),
            v(100.0, 200.0),
            v(0.0, 200.0),
        ];
        check_both(&l, 6);
        let u = vec![
            v(0.0, 0.0),
            v(300.0, 0.0),
            v(300.0, 300.0),
            v(200.0, 300.0),
            v(200.0, 100.0),
            v(100.0, 100.0),
            v(100.0, 300.0),
            v(0.0, 300.0),
        ];
        check_both(&u, 8);
    }

    #[test]
    fn collinear_vertex_is_dropped() {
        let p = vec![
            v(0.0, 0.0),
            v(5.0, 0.0),
            v(10.0, 0.0),
            v(10.0, 10.0),
            v(0.0, 10.0),
        ];
        let tris = check(&p, 4);
        assert!(tris.iter().all(|t| !t.contains(&1)));
        // The mesh variant keeps it.
        let all = triangulate_all_vertices(&p).expect("triangulation failed");
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|t| t.contains(&1)));
    }

    #[test]
    fn comb_with_40_teeth() {
        let teeth = 40;
        let mut p = vec![v(0.0, 0.0), v(teeth as f64 * 20.0, 0.0)];
        for k in (0..teeth).rev() {
            let x = k as f64 * 20.0;
            p.push(v(x + 20.0, 100.0));
            p.push(v(x + 10.0, 100.0));
            p.push(v(x + 10.0, 20.0));
            p.push(v(x, 20.0));
        }
        // The last push lands on (0, 20); the loop closes back to (0, 0).
        let n = p.len();
        assert_eq!(validate_simple_polygon(&p, &loose()), Ok(()));
        check_both(&p, n);
    }

    #[test]
    fn spiral() {
        // A thick Archimedean spiral of three turns; turns never touch (w < 2πb).
        let (a, b, w) = (10.0, 10.0, 30.0);
        let steps = 108;
        let max_t = 3.0 * TAU;
        let mut p = Vec::new();
        for k in 0..=steps {
            let t = max_t * k as f64 / steps as f64;
            let r = a + b * t + w;
            p.push(v(r * t.cos(), r * t.sin()));
        }
        for k in (0..=steps).rev() {
            let t = max_t * k as f64 / steps as f64;
            let r = a + b * t;
            p.push(v(r * t.cos(), r * t.sin()));
        }
        assert_eq!(validate_simple_polygon(&p, &loose()), Ok(()));
        let n = p.len();
        check_both(&p, n);
    }

    #[test]
    fn flattened_rounded_rectangle() {
        let corners: Vec<FilletCorner> = [
            v(-600.0, -450.0),
            v(600.0, -450.0),
            v(600.0, 450.0),
            v(-600.0, 450.0),
        ]
        .into_iter()
        .map(|point| FilletCorner {
            point,
            radius: 50.0,
        })
        .collect();
        let path = flatten_fillets(&corners, 0.02, 2048, &loose()).expect("flatten failed");
        let n = path.points.len();
        check_both(&path.points, n);
    }

    pub(crate) fn loose() -> PolygonLimits {
        PolygonLimits {
            min_corners: 3,
            max_corners: 4096,
            min_edge: 1e-6,
            max_abs_coord: 1e9,
            min_area: 0.0,
        }
    }

    /// Star-shaped polygon around the origin: increasing angles, random radii.
    fn random_star(rng: &mut XorShift) -> Vec<DVec2> {
        let n = rng.range(4, 64);
        // Gaps in [0.5, 1] keep every angular gap below π, so the polygon is simple.
        let gaps: Vec<f64> = (0..n).map(|_| 0.5 + 0.5 * rng.unit()).collect();
        let total: f64 = gaps.iter().sum();
        let mut angle = rng.unit() * TAU;
        gaps.iter()
            .map(|g| {
                angle += g / total * TAU;
                let r = 20.0 + 80.0 * rng.unit();
                v(r * angle.cos(), r * angle.sin())
            })
            .collect()
    }

    /// Random simply-connected polyomino on a grid, traced into an orthogonal polygon
    /// (collinear vertices merged).
    pub(crate) fn random_orthogonal(rng: &mut XorShift) -> Vec<DVec2> {
        const W: usize = 12;
        let mut cell = [[false; W]; W];
        let (mut x, mut y) = (W / 2, W / 2);
        cell[y][x] = true;
        let target = rng.range(3, 45);
        let mut count = 1;
        let mut guard = 0;
        while count < target && guard < 10_000 {
            guard += 1;
            match rng.next_u64() % 4 {
                0 if x > 1 => x -= 1,
                1 if x < W - 2 => x += 1,
                2 if y > 1 => y -= 1,
                3 if y < W - 2 => y += 1,
                _ => continue,
            }
            if !cell[y][x] {
                cell[y][x] = true;
                count += 1;
            }
        }
        // Fill holes and remove diagonal pinches until stable, so the boundary is one simple loop.
        loop {
            let mut changed = false;
            let mut outside = [[false; W]; W];
            let mut stack = vec![(0usize, 0usize)];
            while let Some((cx, cy)) = stack.pop() {
                if cell[cy][cx] || outside[cy][cx] {
                    continue;
                }
                outside[cy][cx] = true;
                if cx > 0 {
                    stack.push((cx - 1, cy));
                }
                if cx + 1 < W {
                    stack.push((cx + 1, cy));
                }
                if cy > 0 {
                    stack.push((cx, cy - 1));
                }
                if cy + 1 < W {
                    stack.push((cx, cy + 1));
                }
            }
            for cy in 0..W {
                for cx in 0..W {
                    if !cell[cy][cx] && !outside[cy][cx] {
                        cell[cy][cx] = true;
                        changed = true;
                    }
                }
            }
            for cy in 0..W - 1 {
                for cx in 0..W - 1 {
                    let (a, b, c, d) = (
                        cell[cy][cx],
                        cell[cy][cx + 1],
                        cell[cy + 1][cx],
                        cell[cy + 1][cx + 1],
                    );
                    if a && d && !b && !c {
                        cell[cy][cx + 1] = true;
                        changed = true;
                    } else if b && c && !a && !d {
                        cell[cy][cx] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Directed boundary edges, CCW around each filled cell, keeping only those whose
        // neighbour across the edge is empty.
        let filled = |cx: i64, cy: i64| {
            cx >= 0
                && cy >= 0
                && (cx as usize) < W
                && (cy as usize) < W
                && cell[cy as usize][cx as usize]
        };
        let mut next = std::collections::BTreeMap::new();
        for cy in 0..W as i64 {
            for cx in 0..W as i64 {
                if !filled(cx, cy) {
                    continue;
                }
                if !filled(cx, cy - 1) {
                    next.insert((cx, cy), (cx + 1, cy));
                }
                if !filled(cx + 1, cy) {
                    next.insert((cx + 1, cy), (cx + 1, cy + 1));
                }
                if !filled(cx, cy + 1) {
                    next.insert((cx + 1, cy + 1), (cx, cy + 1));
                }
                if !filled(cx - 1, cy) {
                    next.insert((cx, cy + 1), (cx, cy));
                }
            }
        }
        let start = *next.keys().next().expect("non-empty polyomino");
        let mut loop_pts = vec![start];
        let mut cur = next[&start];
        while cur != start {
            loop_pts.push(cur);
            cur = next[&cur];
        }
        assert_eq!(loop_pts.len(), next.len(), "boundary is not a single loop");
        // Merge collinear vertices.
        let m = loop_pts.len();
        let corners: Vec<DVec2> = (0..m)
            .filter(|&i| {
                let p = loop_pts[(i + m - 1) % m];
                let q = loop_pts[i];
                let r = loop_pts[(i + 1) % m];
                (q.0 - p.0) * (r.1 - q.1) - (q.1 - p.1) * (r.0 - q.0) != 0
            })
            .map(|i| v(loop_pts[i].0 as f64 * 37.0, loop_pts[i].1 as f64 * 23.0))
            .collect();
        corners
    }

    #[test]
    fn fuzz_star_shaped_seeds_1_to_100() {
        for seed in 1..=100 {
            let mut rng = XorShift::new(seed);
            let p = random_star(&mut rng);
            assert_eq!(validate_simple_polygon(&p, &loose()), Ok(()), "seed {seed}");
            let n = p.len();
            check_both(&p, n);
        }
    }

    #[test]
    fn fuzz_orthogonal_seeds_1_to_100() {
        let mut max_corners = 0;
        for seed in 1..=100 {
            let mut rng = XorShift::new(seed);
            let p = random_orthogonal(&mut rng);
            assert_eq!(validate_simple_polygon(&p, &loose()), Ok(()), "seed {seed}");
            let n = p.len();
            max_corners = max_corners.max(n);
            check_both(&p, n);
        }
        assert!(
            max_corners >= 16,
            "fuzz polygons are too simple ({max_corners} corners)"
        );
    }

    #[test]
    fn degenerate_inputs() {
        assert_eq!(triangulate(&[]), Err(TriangulateError::Degenerate));
        assert_eq!(
            triangulate(&[v(0.0, 0.0), v(1.0, 0.0)]),
            Err(TriangulateError::Degenerate)
        );
        assert_eq!(
            triangulate(&[v(0.0, 0.0), v(1.0, 0.0), v(2.0, 0.0)]),
            Err(TriangulateError::Degenerate)
        );
        assert_eq!(
            triangulate(&[v(0.0, 0.0), v(f64::NAN, 0.0), v(2.0, 1.0)]),
            Err(TriangulateError::Degenerate)
        );
    }
}
