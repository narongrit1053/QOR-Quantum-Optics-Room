//! Mitred room walls with real openings.
//!
//! The outline is the inner face of the walls; walls extrude outward by `thickness`. Each wall is
//! its own body. Neighbouring bodies meet on the mitre plane through the inner and outer corner,
//! so they share the outer corner and never overlap.
//!
//! Every face is split only where a neighbouring face needs a vertex, so a body plus its (virtual)
//! bottom face is a closed mesh without T-junctions.

use glam::{DVec2, DVec3};

use crate::mesh::TriMesh;
use crate::polygon::{
    PolygonError, PolygonLimits, interior_angle, orientation_sign, validate_simple_polygon,
};

/// Interior-angle limits that keep the mitre intersection finite (lab-core `ROOM_ANGLE_DEG`).
const MIN_ANGLE_DEG: f64 = 20.0;
const MAX_ANGLE_DEG: f64 = 340.0;
/// Shortest allowed outer face, in mm.
const MIN_OUTER_FACE: f64 = 10.0;
const EPS: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpeningSpec {
    pub wall: usize,
    pub offset: f64,
    pub width: f64,
    pub sill: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WallGeometry {
    pub wall: usize,
    /// Outline corners `wall` and `wall + 1`.
    pub inner_start: DVec2,
    pub inner_end: DVec2,
    /// Mitred outer corners.
    pub outer_start: DVec2,
    pub outer_end: DVec2,
    pub outward_normal: DVec2,
    /// No bottom face.
    pub body: TriMesh,
    /// Plan quad inner_start, inner_end, outer_end, outer_start at footprint_z, normal +Z.
    pub footprint: TriMesh,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WallError {
    #[error("{0}")]
    BadOutline(PolygonError),
    #[error("corner {} is {degrees:.0}°; allowed 20°–340°", .corner + 1)]
    AngleOutOfRange { corner: usize, degrees: f64 },
    #[error("wall {} is too short for its thickness", .wall + 1)]
    OuterFaceTooShort { wall: usize },
    #[error("opening {} is on wall {}, which does not exist", .opening + 1, .wall + 1)]
    UnknownWall { opening: usize, wall: usize },
    #[error("opening {} does not fit on wall {}: it must lie between {min_offset:.0} and {max_end:.0} mm from the wall start", .opening + 1, .wall + 1)]
    OpeningDoesNotFit {
        opening: usize,
        wall: usize,
        min_offset: f64,
        max_end: f64,
    },
    #[error("openings {} and {} overlap", .a + 1, .b + 1)]
    OpeningsOverlap { a: usize, b: usize },
    #[error("opening {} is taller than the wall", .opening + 1)]
    OpeningTooTall { opening: usize },
}

/// Mitred outer corner of every outline corner: the intersection of the outward offset lines
/// (offset `thickness`) of the two edges meeting there.
fn outer_corners(inner: &[DVec2], thickness: f64) -> Vec<DVec2> {
    let n = inner.len();
    let s = orientation_sign(inner);
    let normal = |i: usize| {
        let d = (inner[(i + 1) % n] - inner[i]).normalize_or_zero();
        s * DVec2::new(d.y, -d.x)
    };
    (0..n)
        .map(|i| {
            let na = normal((i + n - 1) % n);
            let nb = normal(i);
            // m·na = m·nb = thickness for m = t (na + nb) / (1 + na·nb).
            inner[i] + (na + nb) * (thickness / (1.0 + na.dot(nb)))
        })
        .collect()
}

/// Edge coordinates of one wall: `σ` along the inner face from its start, `n` outward.
struct Frame {
    a: DVec2,
    d: DVec2,
    n: DVec2,
    len: f64,
    /// σ of the outer start and end corners.
    sa: f64,
    sb: f64,
}

impl Frame {
    fn new(inner: &[DVec2], outer: &[DVec2], wall: usize) -> Frame {
        let count = inner.len();
        let a = inner[wall];
        let b = inner[(wall + 1) % count];
        let d = (b - a).normalize_or_zero();
        let n = orientation_sign(inner) * DVec2::new(d.y, -d.x);
        Frame {
            a,
            d,
            n,
            len: a.distance(b),
            sa: (outer[wall] - a).dot(d),
            sb: (outer[(wall + 1) % count] - a).dot(d),
        }
    }

    fn plan(&self, sigma: f64, out: f64) -> DVec2 {
        self.a + self.d * sigma + self.n * out
    }

    fn at(&self, sigma: f64, out: f64, z: f64) -> DVec3 {
        self.plan(sigma, out).extend(z)
    }

    /// `[min_offset, max_end]` on the inner face, including the jambs.
    fn limits(&self, jamb: f64) -> (f64, f64) {
        (self.sa.max(0.0) + jamb, self.sb.min(self.len) - jamb)
    }
}

/// Usable inner-face interval `[min_offset, max_end]` on `wall`, already including `min_jamb`.
/// Returns `(0.0, 0.0)` for fewer than three corners or a wall index out of range.
pub fn opening_limits(inner: &[DVec2], thickness: f64, wall: usize, min_jamb: f64) -> (f64, f64) {
    if inner.len() < 3 || wall >= inner.len() {
        return (0.0, 0.0);
    }
    let outer = outer_corners(inner, thickness);
    Frame::new(inner, &outer, wall).limits(min_jamb)
}

/// Builds one mitred body per wall (wall `i` runs from `inner[i]` to `inner[i + 1]`), with each
/// opening cut through the wall and lined with reveals.
///
/// Checks, in order: the outline (simple, any orientation), corner angles (20°–340°), wall
/// thickness (positive and finite, else `OuterFaceTooShort` on wall 1), outer face length
/// (> 10 mm), then each opening in turn (wall exists, fits horizontally per [`opening_limits`],
/// `sill >= 0` and `sill + height <= height`), then spacing (no overlap, at least `min_jamb`
/// apart on one wall).
pub fn build_walls(
    inner: &[DVec2],
    thickness: f64,
    height: f64,
    openings: &[OpeningSpec],
    min_jamb: f64,
    footprint_z: f64,
) -> Result<Vec<WallGeometry>, WallError> {
    build_walls_impl(
        inner,
        thickness,
        height,
        openings,
        min_jamb,
        footprint_z,
        false,
    )
}

/// One opening on one wall, with its vertical extent snapped to the floor and wall top.
#[derive(Clone, Copy)]
struct Cut {
    lo: f64,
    hi: f64,
    z0: f64,
    z1: f64,
    /// Reaches the floor (no sill face, no strip below).
    bottom: bool,
    /// Reaches the wall top (no lintel, no strip above, the top face is cut).
    top: bool,
}

fn build_walls_impl(
    inner: &[DVec2],
    thickness: f64,
    height: f64,
    openings: &[OpeningSpec],
    min_jamb: f64,
    footprint_z: f64,
    with_bottom: bool,
) -> Result<Vec<WallGeometry>, WallError> {
    let limits = PolygonLimits {
        min_corners: 3,
        max_corners: usize::MAX,
        min_edge: 1e-6,
        max_abs_coord: f64::MAX,
        min_area: 0.0,
    };
    validate_simple_polygon(inner, &limits).map_err(WallError::BadOutline)?;
    let n = inner.len();
    for corner in 0..n {
        let degrees = interior_angle(inner, corner).to_degrees();
        if !(MIN_ANGLE_DEG..=MAX_ANGLE_DEG).contains(&degrees) {
            return Err(WallError::AngleOutOfRange { corner, degrees });
        }
    }
    if !(thickness.is_finite() && thickness > 0.0) {
        return Err(WallError::OuterFaceTooShort { wall: 0 });
    }
    let outer = outer_corners(inner, thickness);
    let frames: Vec<Frame> = (0..n).map(|i| Frame::new(inner, &outer, i)).collect();
    if let Some(wall) = frames
        .iter()
        .position(|f| !(f.sb - f.sa).is_finite() || f.sb - f.sa <= MIN_OUTER_FACE)
    {
        return Err(WallError::OuterFaceTooShort { wall });
    }

    for (opening, o) in openings.iter().enumerate() {
        let Some(frame) = frames.get(o.wall) else {
            return Err(WallError::UnknownWall {
                opening,
                wall: o.wall,
            });
        };
        let (min_offset, max_end) = frame.limits(min_jamb);
        let fits = o.offset.is_finite()
            && o.width.is_finite()
            && o.width > 0.0
            && o.offset >= min_offset - EPS
            && o.offset + o.width <= max_end + EPS;
        if !fits {
            return Err(WallError::OpeningDoesNotFit {
                opening,
                wall: o.wall,
                min_offset,
                max_end,
            });
        }
        let vertical_ok = o.sill.is_finite()
            && o.height.is_finite()
            && o.height > 0.0
            && o.sill >= 0.0
            && o.sill + o.height <= height + EPS;
        if !vertical_ok {
            return Err(WallError::OpeningTooTall { opening });
        }
    }

    // Openings per wall, sorted along the wall; check spacing.
    let mut per_wall: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, o) in openings.iter().enumerate() {
        per_wall[o.wall].push(k);
    }
    for list in &mut per_wall {
        list.sort_by(|&p, &q| {
            openings[p]
                .offset
                .total_cmp(&openings[q].offset)
                .then(p.cmp(&q))
        });
        for w in list.windows(2) {
            let (p, q) = (openings[w[0]], openings[w[1]]);
            if q.offset < p.offset + p.width + min_jamb - EPS {
                return Err(WallError::OpeningsOverlap {
                    a: w[0].min(w[1]),
                    b: w[0].max(w[1]),
                });
            }
        }
    }

    let walls = (0..n)
        .map(|i| {
            let f = &frames[i];
            let cuts: Vec<Cut> = per_wall[i]
                .iter()
                .map(|&k| {
                    let o = openings[k];
                    let bottom = o.sill <= EPS;
                    let top = o.sill + o.height >= height - EPS;
                    Cut {
                        lo: o.offset,
                        hi: o.offset + o.width,
                        z0: if bottom { 0.0 } else { o.sill },
                        z1: if top { height } else { o.sill + o.height },
                        bottom,
                        top,
                    }
                })
                .collect();
            let body = wall_body(f, thickness, height, &cuts, with_bottom);
            let (a, b) = (inner[i], inner[(i + 1) % n]);
            let (oa, ob) = (outer[i], outer[(i + 1) % n]);
            let mut footprint = TriMesh::default();
            let corners = [a, b, ob, oa].map(|p| p.extend(footprint_z));
            for tri in [[0, 1, 2], [0, 2, 3]] {
                let mut t = tri.map(|k| corners[k]);
                if (t[1] - t[0]).cross(t[2] - t[0]).z < 0.0 {
                    t.swap(1, 2);
                }
                for p in t {
                    footprint.positions.push(p);
                    footprint.normals.push(DVec3::Z);
                    footprint.uvs.push(p.truncate() / 1000.0);
                    footprint.indices.push(footprint.indices.len() as u32);
                }
            }
            WallGeometry {
                wall: i,
                inner_start: a,
                inner_end: b,
                outer_start: oa,
                outer_end: ob,
                outward_normal: f.n,
                body,
                footprint,
            }
        })
        .collect();
    Ok(walls)
}

/// A point in wall coordinates: (σ, outward offset, z).
type WallPoint = (f64, f64, f64);

struct BodyBuilder<'a> {
    f: &'a Frame,
    mesh: TriMesh,
}

impl BodyBuilder<'_> {
    /// Adds a flat-shaded triangle, wound so its geometric normal agrees with `normal`.
    fn tri(&mut self, pts: [WallPoint; 3], normal: DVec3) {
        let mut p = pts.map(|(s, o, z)| (self.f.at(s, o, z), DVec2::new(s, z) / 1000.0));
        if (p[1].0 - p[0].0).cross(p[2].0 - p[0].0).dot(normal) < 0.0 {
            p.swap(1, 2);
        }
        for (pos, uv) in p {
            self.mesh.indices.push(self.mesh.positions.len() as u32);
            self.mesh.positions.push(pos);
            self.mesh.normals.push(normal);
            self.mesh.uvs.push(uv);
        }
    }

    /// Adds a convex planar quad given in cyclic order (either direction).
    fn quad(&mut self, q: [WallPoint; 4], normal: DVec3) {
        self.tri([q[0], q[1], q[2]], normal);
        self.tri([q[0], q[2], q[3]], normal);
    }

    /// A vertical rectangle `[x0, x1] × [0, H]` at outward offset `o` whose left and right
    /// sides carry extra vertices at the given ascending heights.
    fn ladder(&mut self, o: f64, x0: f64, left: &[f64], x1: f64, right: &[f64], normal: DVec3) {
        if x1 - x0 <= 0.0 {
            return;
        }
        let (mut i, mut j) = (0, 0);
        while i + 1 < left.len() || j + 1 < right.len() {
            let advance_left =
                j + 1 >= right.len() || (i + 1 < left.len() && left[i + 1] <= right[j + 1]);
            if advance_left {
                self.tri(
                    [(x0, o, left[i]), (x0, o, left[i + 1]), (x1, o, right[j])],
                    normal,
                );
                i += 1;
            } else {
                self.tri(
                    [(x0, o, left[i]), (x1, o, right[j + 1]), (x1, o, right[j])],
                    normal,
                );
                j += 1;
            }
        }
    }
}

/// Heights along an opening's side where the neighbouring full-height strip needs vertices.
fn side_chain(cut: Option<&Cut>, height: f64) -> Vec<f64> {
    let mut z = vec![0.0];
    if let Some(c) = cut {
        if !c.bottom {
            z.push(c.z0);
        }
        if !c.top {
            z.push(c.z1);
        }
    }
    z.push(height);
    z
}

fn wall_body(f: &Frame, t: f64, h: f64, cuts: &[Cut], with_bottom: bool) -> TriMesh {
    let mut b = BodyBuilder {
        f,
        mesh: TriMesh::default(),
    };
    let d3 = f.d.extend(0.0);
    let n3 = f.n.extend(0.0);

    // Inner (offset 0, facing the room) and outer (offset t) faces, as strips.
    for (o, lo_face, hi_face, normal) in [(0.0, 0.0, f.len, -n3), (t, f.sa, f.sb, n3)] {
        let mut x0 = lo_face;
        let mut left: Option<&Cut> = None;
        for cut in cuts.iter().map(Some).chain(std::iter::once(None)) {
            let x1 = cut.map_or(hi_face, |c| c.lo);
            b.ladder(o, x0, &side_chain(left, h), x1, &side_chain(cut, h), normal);
            if let Some(c) = cut {
                if !c.bottom {
                    b.quad(
                        [
                            (c.lo, o, 0.0),
                            (c.hi, o, 0.0),
                            (c.hi, o, c.z0),
                            (c.lo, o, c.z0),
                        ],
                        normal,
                    );
                }
                if !c.top {
                    b.quad(
                        [(c.lo, o, c.z1), (c.hi, o, c.z1), (c.hi, o, h), (c.lo, o, h)],
                        normal,
                    );
                }
                x0 = c.hi;
                left = Some(c);
            }
        }
    }

    // Top face (and the virtual bottom used by tests): the plan trapezoid cut across at every
    // opening side, skipping openings that reach that face.
    let mut horizontals = vec![(h, DVec3::Z, true)];
    if with_bottom {
        horizontals.push((0.0, DVec3::NEG_Z, false));
    }
    for (z, normal, is_top) in horizontals {
        let mut pieces: Vec<(f64, f64, f64, f64)> = Vec::new(); // (in_lo, in_hi, out_lo, out_hi)
        let (mut in_lo, mut out_lo) = (0.0, f.sa);
        for c in cuts {
            pieces.push((in_lo, c.lo, out_lo, c.lo));
            let reaches = if is_top { c.top } else { c.bottom };
            if !reaches {
                pieces.push((c.lo, c.hi, c.lo, c.hi));
            }
            in_lo = c.hi;
            out_lo = c.hi;
        }
        pieces.push((in_lo, f.len, out_lo, f.sb));
        for (il, ih, ol, oh) in pieces {
            b.quad([(il, 0.0, z), (ih, 0.0, z), (oh, t, z), (ol, t, z)], normal);
        }
    }

    // Mitre end faces, facing away from the wall's own plan quad.
    let centre = (f.plan(0.0, 0.0) + f.plan(f.len, 0.0) + f.plan(f.sb, t) + f.plan(f.sa, t)) / 4.0;
    for (s_in, s_out) in [(0.0, f.sa), (f.len, f.sb)] {
        let p = f.plan(s_in, 0.0);
        let e = f.plan(s_out, t) - p;
        let mut perp = DVec2::new(e.y, -e.x).normalize_or_zero();
        if (centre - p).dot(perp) > 0.0 {
            perp = -perp;
        }
        b.quad(
            [
                (s_in, 0.0, 0.0),
                (s_out, t, 0.0),
                (s_out, t, h),
                (s_in, 0.0, h),
            ],
            perp.extend(0.0),
        );
    }

    // Reveals: jambs, lintel underside, sill top.
    for c in cuts {
        b.quad(
            [
                (c.lo, 0.0, c.z0),
                (c.lo, t, c.z0),
                (c.lo, t, c.z1),
                (c.lo, 0.0, c.z1),
            ],
            d3,
        );
        b.quad(
            [
                (c.hi, 0.0, c.z0),
                (c.hi, t, c.z0),
                (c.hi, t, c.z1),
                (c.hi, 0.0, c.z1),
            ],
            -d3,
        );
        if !c.top {
            b.quad(
                [
                    (c.lo, 0.0, c.z1),
                    (c.hi, 0.0, c.z1),
                    (c.hi, t, c.z1),
                    (c.lo, t, c.z1),
                ],
                DVec3::NEG_Z,
            );
        }
        if !c.bottom {
            b.quad(
                [
                    (c.lo, 0.0, c.z0),
                    (c.hi, 0.0, c.z0),
                    (c.hi, t, c.z0),
                    (c.lo, t, c.z0),
                ],
                DVec3::Z,
            );
        }
    }
    b.mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::normals_agree;
    use crate::polygon::orient;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    const T: f64 = 150.0;
    const H: f64 = 3000.0;
    const JAMB: f64 = 50.0;

    fn rect() -> Vec<DVec2> {
        vec![
            v(-3000.0, -2500.0),
            v(3000.0, -2500.0),
            v(3000.0, 2500.0),
            v(-3000.0, 2500.0),
        ]
    }

    /// L room, CCW, with the reflex corner at index 3.
    fn l_room() -> Vec<DVec2> {
        vec![
            v(0.0, 0.0),
            v(6000.0, 0.0),
            v(6000.0, 3000.0),
            v(3000.0, 3000.0),
            v(3000.0, 6000.0),
            v(0.0, 6000.0),
        ]
    }

    fn closed_walls(inner: &[DVec2], openings: &[OpeningSpec]) -> Vec<WallGeometry> {
        let with_bottom = build_walls_impl(inner, T, H, openings, JAMB, 1.0, true).expect("walls");
        let public = build_walls(inner, T, H, openings, JAMB, 1.0).expect("walls");
        assert_eq!(with_bottom.len(), inner.len());
        for (w, p) in with_bottom.iter().zip(&public) {
            assert!(w.body.is_closed(), "wall {} is not closed", w.wall);
            assert!(!p.body.is_closed(), "public body {} has a bottom", p.wall);
            assert!(normals_agree(&w.body), "wall {} winding", w.wall);
            // The public body is the closed body minus its bottom triangles.
            let bottom_tris = w
                .body
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|t| {
                    t.iter().all(|&i| {
                        w.body.positions[i as usize].z == 0.0
                            && w.body.normals[i as usize] == DVec3::NEG_Z
                    })
                })
                .count();
            assert!(bottom_tris >= 2);
            assert_eq!(
                p.body.triangle_count(),
                w.body.triangle_count() - bottom_tris
            );
        }
        public
    }

    fn opening(wall: usize, offset: f64, width: f64, sill: f64, height: f64) -> OpeningSpec {
        OpeningSpec {
            wall,
            offset,
            width,
            sill,
            height,
        }
    }

    #[test]
    fn rectangle_gives_four_closed_bodies_both_orientations() {
        let ccw = rect();
        let mut cw = ccw.clone();
        cw.reverse();
        for inner in [ccw, cw] {
            let walls = closed_walls(&inner, &[]);
            assert_eq!(walls.len(), 4);
            for w in &walls {
                // Outward: away from the room centre at the origin.
                assert!(w.outward_normal.dot(w.inner_start) > 0.0);
                assert!((w.outer_start.x.abs() - 3150.0).abs() < 1e-9);
                assert!((w.outer_start.y.abs() - 2650.0).abs() < 1e-9);
                // Footprint: two triangles facing up.
                assert_eq!(w.footprint.triangle_count(), 2);
                assert!(normals_agree(&w.footprint));
                assert!(w.footprint.positions.iter().all(|p| p.z == 1.0));
            }
        }
    }

    #[test]
    fn neighbouring_walls_share_outer_corners() {
        let walls = build_walls(&l_room(), T, H, &[], JAMB, 1.0).expect("walls");
        for i in 0..walls.len() {
            let j = (i + 1) % walls.len();
            assert_eq!(walls[i].outer_end, walls[j].outer_start);
            assert_eq!(walls[i].inner_end, walls[j].inner_start);
        }
    }

    #[test]
    fn l_room_reflex_corner_moves_outer_start_along_the_wall() {
        let inner = l_room();
        let walls = closed_walls(&inner, &[]);
        // Wall 3 starts at the reflex corner (3000, 3000).
        let w = &walls[3];
        let d = (w.inner_end - w.inner_start).normalize();
        let sigma = (w.outer_start - w.inner_start).dot(d);
        assert!(sigma > 0.0);
        assert!((sigma - T).abs() < 1e-9);
        // Wall 2 ends there: its outer end falls short of the inner end.
        let w2 = &walls[2];
        let d2 = (w2.inner_end - w2.inner_start).normalize();
        let len2 = w2.inner_start.distance(w2.inner_end);
        assert!(((w2.outer_end - w2.inner_start).dot(d2) - (len2 - T)).abs() < 1e-9);
    }

    #[test]
    fn opening_limits_at_convex_and_reflex_corners() {
        let inner = l_room();
        // Wall 0: 90° corners at both ends.
        let (lo, hi) = opening_limits(&inner, T, 0, JAMB);
        assert!((lo - JAMB).abs() < 1e-9);
        assert!((hi - (6000.0 - JAMB)).abs() < 1e-9);
        // Wall 3 starts at the 270° corner: t + jamb.
        let (lo, hi) = opening_limits(&inner, T, 3, JAMB);
        assert!((lo - (T + JAMB)).abs() < 1e-9, "lo {lo}");
        assert!((hi - (3000.0 - JAMB)).abs() < 1e-9);
        // Wall 2 ends there.
        let (_, hi) = opening_limits(&inner, T, 2, JAMB);
        assert!((hi - (3000.0 - T - JAMB)).abs() < 1e-9);
        // Same values for the clockwise outline (wall indices change).
        let mut cw = inner.clone();
        cw.reverse(); // wall k of cw runs inner[5-k] -> inner[4-k]; ccw wall 3 is cw wall 1, reversed
        let (lo, hi) = opening_limits(&cw, T, 1, JAMB);
        assert!((lo - JAMB).abs() < 1e-9);
        assert!((hi - (3000.0 - T - JAMB)).abs() < 1e-9);
        assert_eq!(opening_limits(&inner, T, 6, JAMB), (0.0, 0.0));
    }

    #[test]
    fn fitting_openings_keep_bodies_closed() {
        let openings = [
            opening(0, 1000.0, 900.0, 0.0, 2100.0),       // door
            opening(0, 2500.0, 1200.0, 900.0, 1200.0),    // window
            opening(0, 5000.0, 800.0, 800.0, 2200.0),     // reaches the top
            opening(2, 1000.0, 1000.0, 0.0, 3000.0),      // full height
            opening(3, T + JAMB, 1000.0, 1000.0, 1000.0), // tight against the reflex jamb
        ];
        let walls = closed_walls(&l_room(), &openings);
        // Openings are real holes: no inner-face triangle covers the window's middle.
        let w = &walls[0];
        let mid = DVec3::new(2500.0 + 600.0, 0.0, 1500.0);
        for tri in w.body.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| w.body.positions[i as usize]);
            if a.y == 0.0 && b.y == 0.0 && c.y == 0.0 {
                let p = DVec2::new(mid.x, mid.z);
                let (a, b, c) = (
                    DVec2::new(a.x, a.z),
                    DVec2::new(b.x, b.z),
                    DVec2::new(c.x, c.z),
                );
                let s = [orient(a, b, p), orient(b, c, p), orient(c, a, p)];
                assert!(!(s.iter().all(|&x| x > 0.0) || s.iter().all(|&x| x < 0.0)));
            }
        }
    }

    #[test]
    fn no_lintel_strip_when_opening_reaches_the_top() {
        let o = opening(0, 1000.0, 1000.0, 1000.0, H - 1000.0);
        let walls = closed_walls(&rect(), &[o]);
        let w = &walls[0];
        // No lintel underside anywhere, and no inner-face triangle above the opening.
        assert!(w.body.normals.iter().all(|&n| n != DVec3::NEG_Z));
        let start = w.inner_start;
        for tri in w.body.indices.as_chunks::<3>().0 {
            let ps = [tri[0], tri[1], tri[2]].map(|i| w.body.positions[i as usize]);
            let on_inner = ps.iter().all(|p| (p.y - start.y).abs() < 1e-9);
            if on_inner {
                let centroid = (ps[0] + ps[1] + ps[2]) / 3.0;
                let sigma = centroid.x - start.x;
                assert!(!(sigma > 1000.0 && sigma < 2000.0 && centroid.z > 1000.0));
            }
        }
        // With a lower top there is a lintel.
        let o = opening(0, 1000.0, 1000.0, 1000.0, H - 1001.0);
        let walls = closed_walls(&rect(), &[o]);
        assert!(walls[0].body.normals.contains(&DVec3::NEG_Z));
    }

    #[test]
    fn overlapping_or_too_close_openings() {
        let a = opening(1, 500.0, 1000.0, 0.0, 2000.0);
        let b = opening(1, 1200.0, 1000.0, 0.0, 2000.0);
        assert_eq!(
            build_walls(&rect(), T, H, &[b, a], JAMB, 1.0).map(|_| ()),
            Err(WallError::OpeningsOverlap { a: 0, b: 1 })
        );
        // 49 mm apart: closer than the jamb.
        let c = opening(1, 1549.0, 1000.0, 0.0, 2000.0);
        assert_eq!(
            build_walls(&rect(), T, H, &[a, c], JAMB, 1.0).map(|_| ()),
            Err(WallError::OpeningsOverlap { a: 0, b: 1 })
        );
        // Exactly a jamb apart is fine, and openings on different walls never clash.
        let d = opening(1, 1550.0, 1000.0, 0.0, 2000.0);
        assert!(build_walls(&rect(), T, H, &[a, d], JAMB, 1.0).is_ok());
        let e = opening(2, 500.0, 1000.0, 0.0, 2000.0);
        assert!(build_walls(&rect(), T, H, &[a, e], JAMB, 1.0).is_ok());
    }

    #[test]
    fn opening_errors() {
        let inner = rect();
        assert_eq!(
            build_walls(
                &inner,
                T,
                H,
                &[opening(4, 100.0, 500.0, 0.0, 2000.0)],
                JAMB,
                1.0
            )
            .map(|_| ()),
            Err(WallError::UnknownWall {
                opening: 0,
                wall: 4
            })
        );
        let err = build_walls(
            &inner,
            T,
            H,
            &[opening(0, 5600.0, 500.0, 0.0, 2000.0)],
            JAMB,
            1.0,
        )
        .map(|_| ())
        .expect_err("does not fit");
        assert_eq!(
            err,
            WallError::OpeningDoesNotFit {
                opening: 0,
                wall: 0,
                min_offset: 50.0,
                max_end: 5950.0
            }
        );
        assert_eq!(
            err.to_string(),
            "opening 1 does not fit on wall 1: it must lie between 50 and 5950 mm from the wall start"
        );
        assert_eq!(
            build_walls(
                &inner,
                T,
                H,
                &[opening(0, 49.0, 500.0, 0.0, 2000.0)],
                JAMB,
                1.0
            )
            .map(|_| ()),
            Err(WallError::OpeningDoesNotFit {
                opening: 0,
                wall: 0,
                min_offset: 50.0,
                max_end: 5950.0
            })
        );
        assert_eq!(
            build_walls(
                &inner,
                T,
                H,
                &[opening(0, 100.0, 500.0, 1000.0, 2001.0)],
                JAMB,
                1.0
            )
            .map(|_| ()),
            Err(WallError::OpeningTooTall { opening: 0 })
        );
        assert_eq!(
            build_walls(
                &inner,
                T,
                H,
                &[opening(0, 100.0, 500.0, -1.0, 2000.0)],
                JAMB,
                1.0
            )
            .map(|_| ()),
            Err(WallError::OpeningTooTall { opening: 0 })
        );
        assert_eq!(
            build_walls(
                &inner,
                T,
                H,
                &[opening(0, f64::NAN, 500.0, 0.0, 2000.0)],
                JAMB,
                1.0
            )
            .map(|_| ()),
            Err(WallError::OpeningDoesNotFit {
                opening: 0,
                wall: 0,
                min_offset: 50.0,
                max_end: 5950.0
            })
        );
    }

    #[test]
    fn outline_errors() {
        let bow = vec![
            v(0.0, 0.0),
            v(1000.0, 1000.0),
            v(1000.0, 0.0),
            v(0.0, 1000.0),
        ];
        assert_eq!(
            build_walls(&bow, T, H, &[], JAMB, 1.0).map(|_| ()),
            Err(WallError::BadOutline(PolygonError::SelfIntersecting {
                a: 0,
                b: 2
            }))
        );
        // A 15° corner.
        let sharp = vec![
            v(0.0, 0.0),
            v(10_000.0, 0.0),
            v(10_000.0, 10_000.0 * 15f64.to_radians().tan()),
        ];
        match build_walls(&sharp, T, H, &[], JAMB, 1.0) {
            Err(WallError::AngleOutOfRange { corner: 0, degrees }) => {
                assert!((degrees - 15.0).abs() < 1e-9)
            }
            other => panic!("unexpected {:?}", other.map(|_| ())),
        }
        // A 100 mm wall between two reflex corners cannot take a 150 mm wall.
        let slot = vec![
            v(0.0, 0.0),
            v(6000.0, 0.0),
            v(6000.0, 5000.0),
            v(3050.0, 5000.0),
            v(3050.0, 4000.0),
            v(2950.0, 4000.0),
            v(2950.0, 5000.0),
            v(0.0, 5000.0),
        ];
        assert_eq!(
            build_walls(&slot, T, H, &[], JAMB, 1.0).map(|_| ()),
            Err(WallError::OuterFaceTooShort { wall: 4 })
        );
        assert_eq!(
            build_walls(&rect(), 0.0, H, &[], JAMB, 1.0).map(|_| ()),
            Err(WallError::OuterFaceTooShort { wall: 0 })
        );
    }

    #[test]
    fn mitre_at_120_degrees_does_not_overlap() {
        // Regular hexagon: every corner is 120°.
        let inner: Vec<DVec2> = (0..6)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / 6.0;
                v(3000.0 * a.cos(), 3000.0 * a.sin())
            })
            .collect();
        for k in 0..6 {
            assert!((interior_angle(&inner, k).to_degrees() - 120.0).abs() < 1e-9);
        }
        let walls = closed_walls(&inner, &[opening(1, 500.0, 1000.0, 0.0, 2000.0)]);
        for c in 0..6 {
            let before = &walls[(c + 5) % 6];
            let after = &walls[c];
            // The outer corners match.
            assert_eq!(before.outer_end, after.outer_start);
            // Both bodies lie on opposite sides of the mitre plane through the corner.
            let (p, q) = (after.inner_start, after.outer_start);
            let side = |w: &WallGeometry| {
                w.body
                    .positions
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
                        let s = orient(p, q, x.truncate()) / p.distance(q);
                        (lo.min(s), hi.max(s))
                    })
            };
            let (b_lo, b_hi) = side(before);
            let (a_lo, a_hi) = side(after);
            let tol = 1e-6;
            assert!(
                (b_hi <= tol && a_lo >= -tol) || (a_hi <= tol && b_lo >= -tol),
                "corner {c}: before [{b_lo}, {b_hi}], after [{a_lo}, {a_hi}]"
            );
            // And neither body is flat against the plane.
            assert!(b_hi - b_lo > 1.0 && a_hi - a_lo > 1.0);
        }
    }
}
