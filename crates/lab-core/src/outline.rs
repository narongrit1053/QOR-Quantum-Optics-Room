//! Platform outlines: parametric kinds and custom polygons with per-corner fillets.
//!
//! Outlines live in the platform frame. Parametric kinds centre their bounding box on the frame
//! origin; custom polygons are stored exactly as typed, in the order the user typed the corners
//! (either orientation), so corner numbers never renumber themselves (M1-SPEC D7).

use glam::DVec2;
use lab_geom::{FilletCorner, FilletError, FlatPath, PolygonLimits};
use serde::{Deserialize, Serialize};

use crate::limits::{
    FLATTEN_MAX_POINTS, FLATTEN_TOLERANCE, MAX_COORD, OUTLINE_MAX_CORNERS, OUTLINE_MIN_EDGE, PLATFORM_MAX_EXTENT,
    PLATFORM_MIN_AREA, PLATFORM_MIN_EXTENT,
};
use crate::units::Mm;

/// Smallest leg of material an L-shape notch must leave on each side.
const L_MIN_LEG: Mm = 50.0;

/// Polygon limits for platform outlines.
pub(crate) const OUTLINE_LIMITS: PolygonLimits = PolygonLimits {
    min_corners: 3,
    max_corners: OUTLINE_MAX_CORNERS,
    min_edge: OUTLINE_MIN_EDGE,
    max_abs_coord: MAX_COORD,
    min_area: PLATFORM_MIN_AREA,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outline {
    Rectangle {
        size: DVec2,
    },
    RoundedRectangle {
        size: DVec2,
        radius: Mm,
    },
    /// A rectangle with a rectangular `notch` (x, y extent) cut from `notch_corner`.
    LShape {
        size: DVec2,
        notch: DVec2,
        notch_corner: RectCorner,
    },
    Polygon {
        corners: Vec<OutlineCorner>,
    },
}

/// A corner of an axis-aligned rectangle. UI order 0..3 = ↙ ↘ ↗ ↖.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RectCorner {
    XNegYNeg,
    XPosYNeg,
    XPosYPos,
    XNegYPos,
}

impl RectCorner {
    pub const ALL: [RectCorner; 4] =
        [RectCorner::XNegYNeg, RectCorner::XPosYNeg, RectCorner::XPosYPos, RectCorner::XNegYPos];

    /// UI index 0..3.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<RectCorner> {
        RectCorner::ALL.get(i).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineCorner {
    pub point: DVec2,
    #[serde(default)]
    pub radius: Mm,
}

impl OutlineCorner {
    /// A corner with no fillet.
    pub fn sharp(point: DVec2) -> Self {
        Self { point, radius: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OutlineError {
    #[error("Size must be between {min} and {max} mm")]
    SizeOutOfRange { min: f64, max: f64 },
    #[error("Corner radius is too large; the maximum here is {max:.1} mm")]
    RadiusTooLarge { max: f64 },
    #[error("The cut-out must leave at least 50 mm on each side")]
    BadNotch,
    #[error("Corner {} radius is too large; the maximum here is {max:.1} mm", .corner + 1)]
    FilletTooLarge { corner: usize, max: f64 },
    #[error("Corner {} radius cannot be negative", .corner + 1)]
    NegativeRadius { corner: usize },
    #[error("The outline {0}")]
    Polygon(lab_geom::PolygonError),
    #[error("Only custom outlines have editable corners")]
    NotAPolygon,
    #[error("An outline needs at least 3 corners")]
    TooFewCorners,
    /// Not in M1-SPEC §3.3: a corner index past the end (a stale index from the UI).
    #[error("Corner {} does not exist", .corner + 1)]
    CornerOutOfRange { corner: usize },
}

fn size_error() -> OutlineError {
    OutlineError::SizeOutOfRange { min: PLATFORM_MIN_EXTENT, max: PLATFORM_MAX_EXTENT }
}

fn extent_ok(v: f64) -> bool {
    (PLATFORM_MIN_EXTENT..=PLATFORM_MAX_EXTENT).contains(&v)
}

/// Canonical L corners (M1-SPEC §3.3): CCW, starting at the lexicographically smallest (y, x).
fn l_corners(size: DVec2, notch: DVec2, notch_corner: RectCorner) -> Vec<DVec2> {
    let (hx, hy) = (size.x / 2.0, size.y / 2.0);
    let (a, b) = (notch.x, notch.y);
    let (mx, my) = match notch_corner {
        RectCorner::XPosYPos => (false, false),
        RectCorner::XNegYPos => (true, false),
        RectCorner::XPosYNeg => (false, true),
        RectCorner::XNegYNeg => (true, true),
    };
    let flip = |v: f64, mirror: bool| if mirror { -v + 0.0 } else { v };
    let mut points: Vec<DVec2> = [(-hx, -hy), (hx, -hy), (hx, hy - b), (hx - a, hy - b), (hx - a, hy), (-hx, hy)]
        .iter()
        .map(|&(x, y)| DVec2::new(flip(x, mx), flip(y, my)))
        .collect();
    if mx != my {
        points.reverse();
    }
    let first = (0..points.len())
        .min_by(|&i, &j| {
            let (p, q) = (points[i], points[j]);
            p.y.total_cmp(&q.y).then(p.x.total_cmp(&q.x))
        })
        .unwrap_or(0);
    points.rotate_left(first);
    points
}

impl Outline {
    /// The canonical corner list: CCW and centred for the parametric kinds, as stored for
    /// polygons (M1-SPEC §3.3).
    pub fn corners(&self) -> Vec<OutlineCorner> {
        match self {
            Outline::Rectangle { size } => rect_points(*size).map(OutlineCorner::sharp).to_vec(),
            Outline::RoundedRectangle { size, radius } => {
                rect_points(*size).map(|point| OutlineCorner { point, radius: *radius }).to_vec()
            }
            Outline::LShape { size, notch, notch_corner } => {
                l_corners(*size, *notch, *notch_corner).into_iter().map(OutlineCorner::sharp).collect()
            }
            Outline::Polygon { corners } => corners.clone(),
        }
    }

    /// The same geometry as a custom polygon.
    pub fn to_polygon(&self) -> Outline {
        Outline::Polygon { corners: self.corners() }
    }

    /// Bounding box of the corner points (fillets never extend it).
    pub fn bounds(&self) -> (DVec2, DVec2) {
        match self {
            Outline::Rectangle { size } | Outline::RoundedRectangle { size, .. } | Outline::LShape { size, .. } => {
                (-*size / 2.0, *size / 2.0)
            }
            Outline::Polygon { corners } => {
                let points: Vec<DVec2> = corners.iter().map(|c| c.point).collect();
                lab_geom::bounds(&points)
            }
        }
    }

    /// 0 rectangle, 1 rounded rectangle, 2 L-shape, 3 custom polygon.
    pub fn kind_index(&self) -> u8 {
        match self {
            Outline::Rectangle { .. } => 0,
            Outline::RoundedRectangle { .. } => 1,
            Outline::LShape { .. } => 2,
            Outline::Polygon { .. } => 3,
        }
    }

    /// Custom polygons only: inserts the midpoint of edge `after → after + 1` (radius 0) at index
    /// `after + 1`. The result is validated.
    pub fn with_corner_inserted(&self, after: usize) -> Result<Outline, OutlineError> {
        let Outline::Polygon { corners } = self else {
            return Err(OutlineError::NotAPolygon);
        };
        let n = corners.len();
        if after >= n {
            return Err(OutlineError::CornerOutOfRange { corner: after });
        }
        let mid = (corners[after].point + corners[(after + 1) % n].point) / 2.0;
        let mut corners = corners.clone();
        corners.insert(after + 1, OutlineCorner::sharp(mid));
        let outline = Outline::Polygon { corners };
        outline.validate()?;
        Ok(outline)
    }

    /// Custom polygons only: removes corner `index`; refuses at 3 corners. The result is
    /// validated.
    pub fn with_corner_removed(&self, index: usize) -> Result<Outline, OutlineError> {
        let Outline::Polygon { corners } = self else {
            return Err(OutlineError::NotAPolygon);
        };
        if index >= corners.len() {
            return Err(OutlineError::CornerOutOfRange { corner: index });
        }
        if corners.len() <= 3 {
            return Err(OutlineError::TooFewCorners);
        }
        let mut corners = corners.clone();
        corners.remove(index);
        let outline = Outline::Polygon { corners };
        outline.validate()?;
        Ok(outline)
    }

    pub fn validate(&self) -> Result<(), OutlineError> {
        self.flatten().map(|_| ())
    }

    /// Validates and flattens the outline (FLATTEN_TOLERANCE, FLATTEN_MAX_POINTS).
    pub(crate) fn flatten(&self) -> Result<FlatPath, OutlineError> {
        match self {
            Outline::Rectangle { size } => check_size(*size)?,
            Outline::RoundedRectangle { size, radius } => {
                check_size(*size)?;
                let max = size.x.min(size.y) / 2.0;
                if *radius < 0.0 {
                    return Err(OutlineError::NegativeRadius { corner: 0 });
                }
                if radius.is_nan() || *radius > max {
                    return Err(OutlineError::RadiusTooLarge { max });
                }
            }
            Outline::LShape { size, notch, .. } => {
                check_size(*size)?;
                let fits = |cut: f64, side: f64| (L_MIN_LEG..=side - L_MIN_LEG).contains(&cut);
                if !(fits(notch.x, size.x) && fits(notch.y, size.y)) {
                    return Err(OutlineError::BadNotch);
                }
            }
            Outline::Polygon { corners } => {
                if corners.len() < 3 {
                    return Err(OutlineError::TooFewCorners);
                }
                let (lo, hi) = self.bounds();
                let extent = hi - lo;
                // Non-finite corners are reported by the polygon check below.
                let finite = corners.iter().all(|c| c.point.is_finite());
                if finite && !(extent_ok(extent.x) && extent_ok(extent.y)) {
                    return Err(size_error());
                }
            }
        }
        let corners: Vec<FilletCorner> =
            self.corners().iter().map(|c| FilletCorner { point: c.point, radius: c.radius }).collect();
        lab_geom::flatten_fillets(&corners, FLATTEN_TOLERANCE, FLATTEN_MAX_POINTS, &OUTLINE_LIMITS).map_err(|e| match e
        {
            FilletError::NegativeRadius { corner } => OutlineError::NegativeRadius { corner },
            FilletError::TooLarge { corner, max_radius } => match self {
                Outline::RoundedRectangle { size, .. } => {
                    OutlineError::RadiusTooLarge { max: size.x.min(size.y) / 2.0 }
                }
                _ => OutlineError::FilletTooLarge { corner, max: max_radius },
            },
            FilletError::Polygon(p) => OutlineError::Polygon(p),
        })
    }
}

fn rect_points(size: DVec2) -> [DVec2; 4] {
    let (hx, hy) = (size.x / 2.0, size.y / 2.0);
    [DVec2::new(-hx, -hy), DVec2::new(hx, -hy), DVec2::new(hx, hy), DVec2::new(-hx, hy)]
}

fn check_size(size: DVec2) -> Result<(), OutlineError> {
    if extent_ok(size.x) && extent_ok(size.y) { Ok(()) } else { Err(size_error()) }
}

#[cfg(test)]
mod tests {
    use lab_geom::{PolygonError, signed_area};

    use super::*;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn poly(points: &[(f64, f64)]) -> Outline {
        Outline::Polygon { corners: points.iter().map(|&(x, y)| OutlineCorner::sharp(v(x, y))).collect() }
    }

    fn rect(w: f64, h: f64) -> Outline {
        Outline::Rectangle { size: v(w, h) }
    }

    #[test]
    fn o1_one_test_per_error_variant() {
        assert_eq!(rect(40.0, 900.0).validate(), Err(size_error()));
        assert_eq!(rect(1200.0, 20_001.0).validate(), Err(size_error()));
        assert_eq!(rect(f64::NAN, 900.0).validate(), Err(size_error()));
        assert_eq!(
            Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: 451.0 }.validate(),
            Err(OutlineError::RadiusTooLarge { max: 450.0 })
        );
        assert_eq!(
            Outline::LShape { size: v(1200.0, 900.0), notch: v(1160.0, 300.0), notch_corner: RectCorner::XPosYPos }
                .validate(),
            Err(OutlineError::BadNotch)
        );
        assert_eq!(
            Outline::LShape { size: v(1200.0, 900.0), notch: v(400.0, 49.0), notch_corner: RectCorner::XNegYNeg }
                .validate(),
            Err(OutlineError::BadNotch)
        );
        let mut corners = rect(1200.0, 900.0).corners();
        corners[2].radius = 1000.0;
        let err = Outline::Polygon { corners: corners.clone() }.validate().unwrap_err();
        // Edge 2 (900 mm, sharp neighbour) limits corner 3 to 900 mm.
        assert!(
            matches!(err, OutlineError::FilletTooLarge { corner: 2, max } if (max - 900.0).abs() < 1e-6),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "Corner 3 radius is too large; the maximum here is 900.0 mm");
        corners[2].radius = -1.0;
        assert_eq!(Outline::Polygon { corners }.validate(), Err(OutlineError::NegativeRadius { corner: 2 }));
        let bow_tie = poly(&[(0.0, 0.0), (500.0, 500.0), (500.0, 0.0), (0.0, 500.0)]);
        let err = bow_tie.validate().unwrap_err();
        assert_eq!(err, OutlineError::Polygon(PolygonError::SelfIntersecting { a: 0, b: 2 }));
        assert_eq!(err.to_string(), "The outline edges 1 and 3 cross");
        assert_eq!(rect(1200.0, 900.0).with_corner_inserted(0), Err(OutlineError::NotAPolygon));
        assert_eq!(poly(&[(0.0, 0.0), (500.0, 0.0)]).validate(), Err(OutlineError::TooFewCorners));
        let triangle = poly(&[(0.0, 0.0), (500.0, 0.0), (0.0, 500.0)]);
        assert_eq!(triangle.with_corner_removed(1), Err(OutlineError::TooFewCorners));
        assert_eq!(triangle.with_corner_removed(3), Err(OutlineError::CornerOutOfRange { corner: 3 }));
        assert_eq!(triangle.with_corner_inserted(3), Err(OutlineError::CornerOutOfRange { corner: 3 }));
    }

    #[test]
    fn rounded_rectangle_negative_radius() {
        assert_eq!(
            Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: -1.0 }.validate(),
            Err(OutlineError::NegativeRadius { corner: 0 })
        );
        assert!(Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: 450.0 }.validate().is_ok());
        assert!(Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: 0.0 }.validate().is_ok());
    }

    #[test]
    fn polygon_errors_keep_their_corner_numbers() {
        let spike = poly(&[(0.0, 0.0), (500.0, 0.0), (500.0, 500.0), (500.0, 100.0)]);
        assert!(matches!(spike.validate(), Err(OutlineError::Polygon(PolygonError::FoldBack { .. }))));
        let short = poly(&[(0.0, 0.0), (500.0, 0.0), (500.0, 0.5), (500.0, 500.0)]);
        assert!(matches!(short.validate(), Err(OutlineError::Polygon(PolygonError::EdgeTooShort { edge: 1, .. }))));
        let nan = poly(&[(0.0, 0.0), (f64::NAN, 0.0), (0.0, 500.0)]);
        assert_eq!(nan.validate(), Err(OutlineError::Polygon(PolygonError::NonFinite { corner: 1 })));
        let far = poly(&[(0.0, 0.0), (1e6, 0.0), (0.0, 500.0)]);
        assert_eq!(far.validate(), Err(size_error()));
        // Collinear corners are allowed, even with a radius (treated as sharp).
        let mut corners = rect(1200.0, 900.0).corners();
        corners.insert(1, OutlineCorner { point: v(0.0, -450.0), radius: 30.0 });
        assert!(Outline::Polygon { corners }.validate().is_ok());
        // Clockwise polygons are valid as typed.
        assert!(poly(&[(-300.0, -200.0), (-300.0, 200.0), (300.0, 200.0), (300.0, -200.0)]).validate().is_ok());
    }

    #[test]
    fn o1_to_polygon_keeps_the_flattened_path() {
        let outlines = [
            rect(1200.0, 900.0),
            Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: 50.0 },
            Outline::LShape { size: v(1200.0, 900.0), notch: v(400.0, 300.0), notch_corner: RectCorner::XNegYPos },
        ];
        for outline in outlines {
            let a = outline.flatten().unwrap();
            let b = outline.to_polygon().flatten().unwrap();
            assert_eq!(a, b);
            assert_eq!(outline.bounds(), outline.to_polygon().bounds());
        }
    }

    #[test]
    fn o1_insert_and_remove_at_both_ends() {
        let square = poly(&[(0.0, 0.0), (600.0, 0.0), (600.0, 600.0), (0.0, 600.0)]);
        let Outline::Polygon { corners } = square.with_corner_inserted(0).unwrap() else { unreachable!() };
        assert_eq!(corners.len(), 5);
        assert_eq!(corners[1], OutlineCorner::sharp(v(300.0, 0.0)));
        let Outline::Polygon { corners } = square.with_corner_inserted(3).unwrap() else { unreachable!() };
        assert_eq!(corners.len(), 5);
        assert_eq!(corners[4], OutlineCorner::sharp(v(0.0, 300.0)));
        assert_eq!(corners[0].point, v(0.0, 0.0));
        let Outline::Polygon { corners } = square.with_corner_removed(0).unwrap() else { unreachable!() };
        assert_eq!(
            corners.iter().map(|c| c.point).collect::<Vec<_>>(),
            vec![v(600.0, 0.0), v(600.0, 600.0), v(0.0, 600.0)]
        );
        let Outline::Polygon { corners } = square.with_corner_removed(3).unwrap() else { unreachable!() };
        assert_eq!(
            corners.iter().map(|c| c.point).collect::<Vec<_>>(),
            vec![v(0.0, 0.0), v(600.0, 0.0), v(600.0, 600.0)]
        );
        let arrow = poly(&[(0.0, 0.0), (600.0, 300.0), (0.0, 600.0), (300.0, 300.0)]);
        assert!(arrow.with_corner_removed(3).is_ok());
        // Removing a corner that leaves crossing edges is refused with the reason.
        let zig = poly(&[(0.0, 0.0), (600.0, 0.0), (600.0, 600.0), (300.0, 100.0), (0.0, 600.0)]);
        assert!(zig.validate().is_ok());
        assert!(matches!(
            zig.with_corner_removed(1),
            Err(OutlineError::Polygon(PolygonError::SelfIntersecting { .. }))
        ));
    }

    #[test]
    fn o1_l_corners_for_every_notch_corner() {
        let size = v(1200.0, 900.0);
        let notch = v(400.0, 300.0);
        for corner in RectCorner::ALL {
            let outline = Outline::LShape { size, notch, notch_corner: corner };
            let points: Vec<DVec2> = outline.corners().iter().map(|c| c.point).collect();
            assert_eq!(points.len(), 6);
            assert!(signed_area(&points) > 0.0, "{corner:?}");
            assert!((signed_area(&points) - (1200.0 * 900.0 - 400.0 * 300.0)).abs() < 1e-6);
            // The first corner has the lexicographically smallest (y, x).
            assert!(points.iter().all(|p| (points[0].y, points[0].x) <= (p.y, p.x)), "{corner:?}");
            // The notch is the bbox corner that is not inside the outline.
            let (sx, sy) = match corner {
                RectCorner::XNegYNeg => (-1.0, -1.0),
                RectCorner::XPosYNeg => (1.0, -1.0),
                RectCorner::XPosYPos => (1.0, 1.0),
                RectCorner::XNegYPos => (-1.0, 1.0),
            };
            let path = outline.flatten().unwrap().points;
            assert!(!lab_geom::point_in_polygon(&path, v(sx * 550.0, sy * 400.0)), "{corner:?}");
            assert!(lab_geom::point_in_polygon(&path, v(-sx * 550.0, sy * 400.0)), "{corner:?}");
            assert!(lab_geom::point_in_polygon(&path, v(sx * 550.0, -sy * 400.0)), "{corner:?}");
            // The notch bbox: the reflex corner sits at (±(600-400), ±(450-300)).
            assert!(points.contains(&v(sx * 200.0, sy * 150.0)), "{corner:?}: {points:?}");
            assert_eq!(outline.bounds(), (-size / 2.0, size / 2.0));
            assert_eq!(outline.kind_index(), 2);
        }
    }

    #[test]
    fn rect_corner_indices() {
        for (i, c) in RectCorner::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
            assert_eq!(RectCorner::from_index(i), Some(*c));
        }
        assert_eq!(RectCorner::from_index(4), None);
    }

    #[test]
    fn outline_json_shape() {
        let json = serde_json::to_string(&rect(1200.0, 900.0)).unwrap();
        assert_eq!(json, r#"{"Rectangle":{"size":[1200.0,900.0]}}"#);
        let corner: OutlineCorner = serde_json::from_str(r#"{"point":[1.0,2.0]}"#).unwrap();
        assert_eq!(corner, OutlineCorner::sharp(v(1.0, 2.0)));
    }
}
