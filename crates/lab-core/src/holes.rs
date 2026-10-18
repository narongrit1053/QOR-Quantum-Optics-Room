//! Tapped-hole grids: which holes exist on a platform, their stable lattice identity and their
//! display labels (M1-SPEC §3.4, D4, D13).
//!
//! A grid is an infinite lattice (`origin + (i·pitch.x, j·pitch.y)`). A lattice point holds a
//! hole when it lies inside the outline, at least `margin` from its boundary, and with its whole
//! thread circle outside every exclusion zone. `LatticeIndex` is the stable identity; `HoleRef`
//! ("C14") is a display label relative to the candidate range (the outline bounding box shrunk by
//! the margin), so labels do not depend on exclusions or interior cut-outs.

use std::fmt;
use std::ops::RangeInclusive;

use glam::DVec2;
use lab_geom::distance_to_segment;
use serde::{Deserialize, Serialize};

use crate::limits::EPS_KEEP;
use crate::model::Platform;
use crate::units::{INCH, Mm, format_count};
use crate::validate::{PlatformError, validate_grid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Thread {
    M4,
    M6,
    Unc8_32,
    Unc1_4_20,
}

impl Thread {
    pub const ALL: [Thread; 4] = [Thread::M4, Thread::M6, Thread::Unc8_32, Thread::Unc1_4_20];

    pub fn label(self) -> &'static str {
        match self {
            Thread::M4 => "M4",
            Thread::M6 => "M6",
            Thread::Unc8_32 => "8-32",
            Thread::Unc1_4_20 => "1/4-20",
        }
    }

    /// Nominal major diameter, used to draw the hole.
    pub fn diameter(self) -> Mm {
        match self {
            Thread::M4 => 4.0,
            Thread::M6 => 6.0,
            Thread::Unc8_32 => 0.164 * INCH,
            Thread::Unc1_4_20 => 0.25 * INCH,
        }
    }

    pub fn radius(self) -> Mm {
        self.diameter() / 2.0
    }

    /// Position in [`Thread::ALL`] (UI choice index).
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Thread> {
        Thread::ALL.get(i).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleGrid {
    /// x, y spacing.
    pub pitch: DVec2,
    pub thread: Thread,
    /// Minimum distance from a hole centre to the outline boundary.
    pub margin: Mm,
    pub origin: GridOrigin,
    pub exclusions: Vec<ExclusionZone>,
}

/// Where `LatticeIndex(0, 0)` sits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GridOrigin {
    /// The pattern is centred on the outline bounding box. Not stable under outline edits; the
    /// app pins it (`At`) once parts sit on the platform (M1-SPEC D13).
    Auto,
    /// `LatticeIndex(0, 0)` at this platform-local point.
    At(DVec2),
}

/// An area of the top surface without holes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExclusionZone {
    pub name: String,
    pub shape: ZoneShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ZoneShape {
    Rect { center: DVec2, size: DVec2 },
    Circle { center: DVec2, radius: Mm },
}

impl ZoneShape {
    /// Corners of a rectangle zone (CCW); empty for a circle.
    pub fn rect_corners(&self) -> Vec<DVec2> {
        match *self {
            ZoneShape::Rect { center, size } => {
                let h = size / 2.0;
                vec![
                    DVec2::new(center.x - h.x, center.y - h.y),
                    DVec2::new(center.x + h.x, center.y - h.y),
                    DVec2::new(center.x + h.x, center.y + h.y),
                    DVec2::new(center.x - h.x, center.y + h.y),
                ]
            }
            ZoneShape::Circle { .. } => Vec::new(),
        }
    }

    /// Distance from `p` to the zone; 0 inside it.
    pub fn distance(&self, p: DVec2) -> f64 {
        match *self {
            ZoneShape::Rect { .. } => lab_geom::distance_to_region(&self.rect_corners(), p),
            ZoneShape::Circle { center, radius } => (p.distance(center) - radius).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridStandard {
    Metric,
    Imperial,
    Custom,
}

impl HoleGrid {
    /// Metric table standard: M6 on 25 mm centres. The default for new tables.
    pub fn metric() -> Self {
        Self {
            pitch: DVec2::splat(25.0),
            thread: Thread::M6,
            margin: 12.5,
            origin: GridOrigin::Auto,
            exclusions: Vec::new(),
        }
    }

    /// Imperial table standard: 1/4-20 on 1 inch centres.
    pub fn imperial() -> Self {
        Self {
            pitch: DVec2::splat(INCH),
            thread: Thread::Unc1_4_20,
            margin: 0.5 * INCH,
            origin: GridOrigin::Auto,
            exclusions: Vec::new(),
        }
    }

    /// Metric or Imperial when pitch, thread and margin match that standard within 1e-9 and the
    /// origin is Auto (zones are ignored); Custom otherwise.
    pub fn standard(&self) -> GridStandard {
        let matches = |s: &HoleGrid| {
            (self.pitch - s.pitch).abs().max_element() <= 1e-9
                && self.thread == s.thread
                && (self.margin - s.margin).abs() <= 1e-9
                && self.origin == GridOrigin::Auto
        };
        if matches(&HoleGrid::metric()) {
            GridStandard::Metric
        } else if matches(&HoleGrid::imperial()) {
            GridStandard::Imperial
        } else {
            GridStandard::Custom
        }
    }
}

/// Stable identity of a lattice point, relative to the grid origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LatticeIndex {
    pub i: i32,
    pub j: i32,
}

/// Display label of a hole, relative to the candidate range. Shown as row letter + column
/// number, e.g. `C14`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HoleRef {
    pub col: u32,
    pub row: u32,
}

impl fmt::Display for HoleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Spreadsheet-style row letters: A..Z, AA..AZ, ...
        let mut letters = Vec::new();
        let mut n = u64::from(self.row) + 1;
        while n > 0 {
            n -= 1;
            letters.push(b'A' + (n % 26) as u8);
            n /= 26;
        }
        letters.reverse();
        write!(f, "{}{}", String::from_utf8_lossy(&letters), u64::from(self.col) + 1)
    }
}

fn saturate_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The holes that exist on one platform.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleSet {
    /// Position of `LatticeIndex(0, 0)`.
    pub origin: DVec2,
    pub pitch: DVec2,
    pub thread_radius: Mm,
    /// Index of label (col 0, row 0) = the candidate range minimum.
    pub first: LatticeIndex,
    /// Candidate range size (both 0 if the range is empty).
    pub cols: u32,
    pub rows: u32,
    /// Row-major over the candidate range.
    bits: Vec<u64>,
    count: usize,
}

impl HoleSet {
    pub fn empty() -> Self {
        Self {
            origin: DVec2::ZERO,
            pitch: DVec2::ZERO,
            thread_radius: 0.0,
            first: LatticeIndex { i: 0, j: 0 },
            cols: 0,
            rows: 0,
            bits: Vec::new(),
            count: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn bit(&self, h: HoleRef) -> bool {
        if h.col >= self.cols || h.row >= self.rows {
            return false;
        }
        let k = h.row as usize * self.cols as usize + h.col as usize;
        self.bits[k / 64] & (1 << (k % 64)) != 0
    }

    pub fn contains(&self, idx: LatticeIndex) -> bool {
        self.label(idx).is_some_and(|h| self.bit(h))
    }

    pub fn contains_label(&self, h: HoleRef) -> bool {
        self.bit(h)
    }

    /// Lattice position of `idx`, whether or not a hole exists there.
    pub fn position(&self, idx: LatticeIndex) -> DVec2 {
        DVec2::new(self.origin.x + f64::from(idx.i) * self.pitch.x, self.origin.y + f64::from(idx.j) * self.pitch.y)
    }

    /// The display label of `idx`; `Some` if `idx` is inside the candidate range.
    pub fn label(&self, idx: LatticeIndex) -> Option<HoleRef> {
        let col = i64::from(idx.i) - i64::from(self.first.i);
        let row = i64::from(idx.j) - i64::from(self.first.j);
        let inside = (0..i64::from(self.cols)).contains(&col) && (0..i64::from(self.rows)).contains(&row);
        inside.then_some(HoleRef { col: col as u32, row: row as u32 })
    }

    pub fn index_of(&self, h: HoleRef) -> LatticeIndex {
        LatticeIndex {
            i: saturate_i32(i64::from(self.first.i) + i64::from(h.col)),
            j: saturate_i32(i64::from(self.first.j) + i64::from(h.row)),
        }
    }

    /// Position of the hole labelled `h`, only if that hole exists.
    pub fn hole_at(&self, h: HoleRef) -> Option<DVec2> {
        self.bit(h).then(|| self.position(self.index_of(h)))
    }

    /// Every hole, `j` ascending, then `i`.
    pub fn iter(&self) -> impl Iterator<Item = LatticeIndex> + '_ {
        let cols = self.cols as usize;
        self.bits
            .iter()
            .enumerate()
            .flat_map(|(w, &word)| {
                let mut word = word;
                std::iter::from_fn(move || {
                    (word != 0).then(|| {
                        let b = word.trailing_zeros() as usize;
                        word &= word - 1;
                        w * 64 + b
                    })
                })
            })
            .map(move |k| LatticeIndex {
                i: saturate_i32(i64::from(self.first.i) + (k % cols) as i64),
                j: saturate_i32(i64::from(self.first.j) + (k / cols) as i64),
            })
    }

    /// The existing hole nearest to `p` (exact Euclidean); ties go to the smallest `(j, i)`.
    pub fn nearest(&self, p: DVec2) -> Option<LatticeIndex> {
        let mut best: Option<(f64, LatticeIndex)> = None;
        for idx in self.iter() {
            let d2 = self.position(idx).distance_squared(p);
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, idx));
            }
        }
        best.map(|(_, idx)| idx)
    }

    /// "1,716 holes · 48 × 36 grid", or "No holes".
    pub fn summary(&self) -> String {
        match self.count {
            0 => "No holes".into(),
            1 => format!("1 hole · {} × {} grid", self.cols, self.rows),
            n => format!("{} holes · {} × {} grid", format_count(n), self.cols, self.rows),
        }
    }
}

/// Position of `LatticeIndex(0, 0)` for a grid on an outline with bounding box `bounds`.
pub fn lattice_origin(grid: &HoleGrid, bounds: (DVec2, DVec2)) -> DVec2 {
    match grid.origin {
        GridOrigin::At(p) => p,
        GridOrigin::Auto => {
            let c = grid.margin;
            let axis = |lo: f64, hi: f64, p: f64| {
                let usable = (hi - lo) - 2.0 * c;
                let n = if usable < -EPS_KEEP { 0.0 } else { (usable / p + 1e-9).floor() + 1.0 };
                if n == 0.0 { (lo + hi) / 2.0 } else { lo + ((hi - lo) - (n - 1.0) * p) / 2.0 }
            };
            let (lo, hi) = bounds;
            DVec2::new(axis(lo.x, hi.x, grid.pitch.x), axis(lo.y, hi.y, grid.pitch.y))
        }
    }
}

fn to_i32(v: f64) -> i32 {
    // `as` saturates and maps NaN to 0.
    v as i32
}

/// Candidate lattice indices `(i range, j range)`: the bounding box shrunk by the margin.
pub fn candidate_range(grid: &HoleGrid, bounds: (DVec2, DVec2)) -> (RangeInclusive<i32>, RangeInclusive<i32>) {
    let o = lattice_origin(grid, bounds);
    let c = grid.margin;
    let axis = |lo: f64, hi: f64, o: f64, p: f64| {
        let first = ((lo + c - o) / p - 1e-9).ceil();
        let last = ((hi - c - o) / p + 1e-9).floor();
        to_i32(first)..=to_i32(last)
    };
    let (lo, hi) = bounds;
    (axis(lo.x, hi.x, o.x, grid.pitch.x), axis(lo.y, hi.y, o.y, grid.pitch.y))
}

/// Number of candidates per axis (0 for an empty range).
pub(crate) fn range_len(r: &RangeInclusive<i32>) -> u64 {
    (i64::from(*r.end()) - i64::from(*r.start()) + 1).max(0) as u64
}

/// The holes of `platform`; empty when it has no grid. Fails if the outline or the grid is
/// invalid.
pub fn compute_holes(platform: &Platform) -> Result<HoleSet, PlatformError> {
    let Some(grid) = &platform.hole_grid else {
        return Ok(HoleSet::empty());
    };
    let path = platform.outline_path().map_err(PlatformError::Outline)?;
    let bounds = platform.outline.bounds();
    validate_grid(grid, bounds).map_err(PlatformError::Grid)?;
    Ok(holes_in(&path.points, grid, bounds))
}

#[derive(Clone, Copy)]
struct Edge {
    a: DVec2,
    b: DVec2,
    lo: DVec2,
    hi: DVec2,
}

/// The existence rule over the candidate range, row by row (M1-SPEC §3.4). Exact: a point is
/// kept iff it is inside `outline`, at least `margin - EPS_KEEP` from its boundary and at least
/// `thread_radius - EPS_KEEP` from every zone.
fn holes_in(outline: &[DVec2], grid: &HoleGrid, bounds: (DVec2, DVec2)) -> HoleSet {
    let origin = lattice_origin(grid, bounds);
    let (ri, rj) = candidate_range(grid, bounds);
    let mut set = HoleSet {
        origin,
        pitch: grid.pitch,
        thread_radius: grid.thread.radius(),
        first: LatticeIndex { i: *ri.start(), j: *rj.start() },
        ..HoleSet::empty()
    };
    let (cols, rows) = (range_len(&ri), range_len(&rj));
    if cols == 0 || rows == 0 {
        return set;
    }
    set.cols = cols as u32;
    set.rows = rows as u32;
    let cols = cols as usize;
    set.bits = vec![0; (cols * rows as usize).div_ceil(64)];

    let n = outline.len();
    let edges: Vec<Edge> = (0..n)
        .map(|k| {
            let (a, b) = (outline[k], outline[(k + 1) % n]);
            Edge { a, b, lo: a.min(b), hi: a.max(b) }
        })
        .collect();
    let c = grid.margin;
    let keep_edge = c - EPS_KEEP;
    let keep_zone = set.thread_radius - EPS_KEEP;
    let mut row_edges: Vec<Edge> = Vec::new();
    let mut crossings: Vec<f64> = Vec::new();
    for (row, j) in rj.enumerate() {
        let y = origin.y + f64::from(j) * grid.pitch.y;
        // Only edges within `c` of the row can be closer than the margin or cross it.
        row_edges.clear();
        row_edges.extend(edges.iter().filter(|e| e.lo.y - c <= y && y <= e.hi.y + c));
        // Same half-open rule and formula as `lab_geom::horizontal_crossings`.
        crossings.clear();
        crossings.extend(row_edges.iter().filter_map(|e| {
            let (a, b) = (e.a, e.b);
            ((a.y <= y && y < b.y) || (b.y <= y && y < a.y)).then(|| a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y))
        }));
        crossings.sort_by(f64::total_cmp);
        let mut passed = 0;
        for (col, i) in ri.clone().enumerate() {
            let x = origin.x + f64::from(i) * grid.pitch.x;
            while passed < crossings.len() && crossings[passed] <= x {
                passed += 1;
            }
            if passed % 2 == 0 {
                continue;
            }
            let q = DVec2::new(x, y);
            let too_close = row_edges
                .iter()
                .any(|e| x >= e.lo.x - c && x <= e.hi.x + c && distance_to_segment(e.a, e.b, q) < keep_edge);
            if too_close || grid.exclusions.iter().any(|z| z.shape.distance(q) < keep_zone) {
                continue;
            }
            let k = row * cols + col;
            set.bits[k / 64] |= 1 << (k % 64);
            set.count += 1;
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use lab_geom::{distance_to_boundary, distance_to_region, point_in_polygon};

    use super::*;
    use crate::model::{PlatformId, PlatformKind, Support, Surface};
    use crate::outline::{Outline, OutlineCorner, RectCorner};
    use crate::validate::GridError;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn platform(outline: Outline, grid: HoleGrid) -> Platform {
        Platform {
            id: PlatformId(1),
            name: "Table A".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline,
            thickness: 210.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(grid),
            support: Support::Legs { size: 120.0, inset: 120.0 },
        }
    }

    fn rect(w: f64, h: f64) -> Outline {
        Outline::Rectangle { size: v(w, h) }
    }

    fn holes(outline: Outline, grid: HoleGrid) -> HoleSet {
        compute_holes(&platform(outline, grid)).unwrap()
    }

    fn polygon(points: &[DVec2]) -> Outline {
        Outline::Polygon { corners: points.iter().map(|&p| OutlineCorner::sharp(p)).collect() }
    }

    /// Brute force: point-in-polygon + all-edge distance + all zones over the candidate range.
    fn oracle(p: &Platform) -> Vec<LatticeIndex> {
        let grid = p.hole_grid.as_ref().unwrap();
        let flat = p.outline_path().unwrap().points;
        let bounds = p.outline.bounds();
        let o = lattice_origin(grid, bounds);
        let (ri, rj) = candidate_range(grid, bounds);
        let mut out = Vec::new();
        for j in rj {
            for i in ri.clone() {
                let q = DVec2::new(o.x + f64::from(i) * grid.pitch.x, o.y + f64::from(j) * grid.pitch.y);
                if !point_in_polygon(&flat, q) || distance_to_boundary(&flat, q) < grid.margin - EPS_KEEP {
                    continue;
                }
                let blocked = grid.exclusions.iter().any(|z| {
                    let d = match z.shape {
                        ZoneShape::Rect { center, size } => {
                            let h = size / 2.0;
                            let corners = [
                                v(center.x - h.x, center.y - h.y),
                                v(center.x + h.x, center.y - h.y),
                                v(center.x + h.x, center.y + h.y),
                                v(center.x - h.x, center.y + h.y),
                            ];
                            distance_to_region(&corners, q)
                        }
                        ZoneShape::Circle { center, radius } => (q.distance(center) - radius).max(0.0),
                    };
                    d < grid.thread.radius() - EPS_KEEP
                });
                if !blocked {
                    out.push(LatticeIndex { i, j });
                }
            }
        }
        out
    }

    fn assert_matches_oracle(p: &Platform) {
        let set = compute_holes(p).unwrap();
        let expected = oracle(p);
        let got: Vec<LatticeIndex> = set.iter().collect();
        assert_eq!(got.len(), set.len());
        assert_eq!(got, expected, "{:?}", p.outline);
    }

    // ---- Ported M0 tests (identical expected values) ----

    #[test]
    fn metric_grid_counts_and_positions() {
        let set = holes(rect(1200.0, 900.0), HoleGrid::metric());
        assert_eq!(set.len(), 1728);
        assert_eq!((set.cols, set.rows), (48, 36));
        assert_eq!(set.hole_at(HoleRef { col: 0, row: 0 }), Some(v(-587.5, -437.5)));
        assert_eq!(set.hole_at(HoleRef { col: 47, row: 35 }), Some(v(587.5, 437.5)));
        assert!(!set.contains_label(HoleRef { col: 48, row: 0 }));
        assert_eq!(set.hole_at(HoleRef { col: 48, row: 0 }), None);
        assert_eq!(set.summary(), "1,728 holes · 48 × 36 grid");
    }

    #[test]
    fn imperial_grid_fits_whole_inches() {
        // 4 ft x 2 ft breadboard: 48 x 24 holes.
        let set = holes(rect(48.0 * INCH, 24.0 * INCH), HoleGrid::imperial());
        assert_eq!((set.cols, set.rows), (48, 24));
        assert_eq!(set.len(), 48 * 24);
    }

    #[test]
    fn nearest_hole_snaps_and_clamps() {
        let set = holes(rect(1200.0, 900.0), HoleGrid::metric());
        let label = |p: DVec2| set.label(set.nearest(p).unwrap()).unwrap();
        assert_eq!(label(v(-580.0, -440.0)), HoleRef { col: 0, row: 0 });
        assert_eq!(label(v(10_000.0, 10_000.0)), HoleRef { col: 47, row: 35 });
        let p = set.hole_at(HoleRef { col: 20, row: 10 }).unwrap();
        assert_eq!(label(p + DVec2::splat(9.0)), HoleRef { col: 20, row: 10 });
    }

    #[test]
    fn hole_labels() {
        assert_eq!(HoleRef { col: 13, row: 2 }.to_string(), "C14");
        assert_eq!(HoleRef { col: 0, row: 25 }.to_string(), "Z1");
        assert_eq!(HoleRef { col: 0, row: 26 }.to_string(), "AA1");
        assert!(HoleRef { col: u32::MAX, row: u32::MAX }.to_string().ends_with("4294967296"));
    }

    // ---- New tests ----

    #[test]
    fn h1_rounded_rectangle_loses_three_holes_per_corner() {
        let set = holes(Outline::RoundedRectangle { size: v(1200.0, 900.0), radius: 50.0 }, HoleGrid::metric());
        assert_eq!(set.len(), 1716);
        assert_eq!((set.cols, set.rows), (48, 36));
        assert!(!set.contains_label(HoleRef { col: 0, row: 0 }));
        assert!(!set.contains_label(HoleRef { col: 1, row: 0 }));
        assert!(!set.contains_label(HoleRef { col: 0, row: 1 }));
        assert!(set.contains_label(HoleRef { col: 1, row: 1 }));
    }

    #[test]
    fn h2_l_shape_keeps_the_hole_at_the_margin() {
        let outline =
            Outline::LShape { size: v(1200.0, 900.0), notch: v(400.0, 300.0), notch_corner: RectCorner::XPosYPos };
        let set = holes(outline, HoleGrid::metric());
        assert_eq!(set.len(), 1536);
        // x = 187.5 is exactly 12.5 from the notch wall x = 200: kept.
        assert!(set.contains(LatticeIndex { i: 31, j: 30 }));
        assert_eq!(set.position(LatticeIndex { i: 31, j: 30 }), v(187.5, 312.5));
        assert!(!set.contains(LatticeIndex { i: 32, j: 30 }));
    }

    #[test]
    fn h3_circle_zone() {
        let mut grid = HoleGrid::metric();
        grid.exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 50.0 },
        });
        let set = holes(rect(1200.0, 900.0), grid);
        assert_eq!(set.len(), 1716);
        // (37.5, 37.5) is 53.033 from the centre: its thread circle (r 3) clears the zone.
        assert!(set.contains(LatticeIndex { i: 25, j: 19 }));
        assert_eq!(set.position(LatticeIndex { i: 25, j: 19 }), v(37.5, 37.5));
        assert!(!set.contains(LatticeIndex { i: 24, j: 18 }));
    }

    #[test]
    fn h4_fixed_shapes_match_the_oracle() {
        let diamond = polygon(&[v(0.0, -500.0), v(700.0, 0.0), v(0.0, 500.0), v(-700.0, 0.0)]);
        let u = polygon(&[
            v(-600.0, -400.0),
            v(600.0, -400.0),
            v(600.0, 400.0),
            v(300.0, 400.0),
            v(300.0, -100.0),
            v(-300.0, -100.0),
            v(-300.0, 400.0),
            v(-600.0, 400.0),
        ]);
        let triangle = polygon(&[v(-500.0, -300.0), v(600.0, -250.0), v(-100.0, 450.0)]);
        let (s, c) = 30f64.to_radians().sin_cos();
        let rotated: Vec<DVec2> = [v(-600.0, -450.0), v(600.0, -450.0), v(600.0, 450.0), v(-600.0, 450.0)]
            .iter()
            .map(|p| v(c * p.x - s * p.y, s * p.x + c * p.y))
            .collect();
        for outline in [diamond.clone(), u, triangle, polygon(&rotated)] {
            for grid in [HoleGrid::metric(), HoleGrid::imperial()] {
                let p = platform(outline.clone(), grid);
                assert!(compute_holes(&p).unwrap().len() > 100);
                assert_matches_oracle(&p);
            }
        }
        // The diamond is symmetric in x and y.
        let set = holes(diamond, HoleGrid::metric());
        assert!(!set.is_empty());
        for idx in set.iter() {
            let p = set.position(idx);
            for mirrored in [v(-p.x, p.y), v(p.x, -p.y)] {
                let other = set.nearest(mirrored).unwrap();
                assert!(set.position(other).distance(mirrored) < 1e-9, "{p} vs {mirrored}");
            }
        }
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn uniform(state: &mut u64, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((xorshift(state) >> 11) as f64 / (1u64 << 53) as f64)
    }

    #[test]
    fn h4_random_polygons_match_the_oracle() {
        let mut total = 0;
        for seed in 1..=64u64 {
            let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let p = loop {
                // A random star-shaped polygon (simple by construction), either orientation.
                let n = 3 + (xorshift(&mut st) % 22) as usize;
                let mut angles: Vec<f64> = (0..n).map(|_| uniform(&mut st, 0.0, std::f64::consts::TAU)).collect();
                angles.sort_by(f64::total_cmp);
                let centre = v(uniform(&mut st, -300.0, 300.0), uniform(&mut st, -300.0, 300.0));
                let mut corners: Vec<OutlineCorner> = angles
                    .iter()
                    .map(|a| {
                        let r = uniform(&mut st, 150.0, 1400.0);
                        let radius =
                            if xorshift(&mut st).is_multiple_of(3) { uniform(&mut st, 0.0, 60.0) } else { 0.0 };
                        OutlineCorner { point: centre + v(a.cos(), a.sin()) * r, radius }
                    })
                    .collect();
                if xorshift(&mut st).is_multiple_of(2) {
                    corners.reverse();
                }
                let pitch = v(uniform(&mut st, 8.0, 40.0), uniform(&mut st, 8.0, 40.0));
                let thread = Thread::from_index((xorshift(&mut st) % 4) as usize).unwrap();
                let margin = uniform(&mut st, thread.radius(), 40.0);
                let origin = if xorshift(&mut st).is_multiple_of(2) {
                    GridOrigin::Auto
                } else {
                    GridOrigin::At(v(uniform(&mut st, -100.0, 100.0), uniform(&mut st, -100.0, 100.0)))
                };
                let exclusions = (0..xorshift(&mut st) % 5)
                    .map(|k| {
                        let center = centre + v(uniform(&mut st, -600.0, 600.0), uniform(&mut st, -600.0, 600.0));
                        let shape = if xorshift(&mut st).is_multiple_of(2) {
                            ZoneShape::Rect {
                                center,
                                size: v(uniform(&mut st, 10.0, 400.0), uniform(&mut st, 10.0, 400.0)),
                            }
                        } else {
                            ZoneShape::Circle { center, radius: uniform(&mut st, 5.0, 250.0) }
                        };
                        ExclusionZone { name: format!("Zone {}", k + 1), shape }
                    })
                    .collect();
                let grid = HoleGrid { pitch, thread, margin, origin, exclusions };
                let p = platform(Outline::Polygon { corners }, grid);
                if p.validate().is_ok() {
                    break p;
                }
            };
            assert_matches_oracle(&p);
            total += compute_holes(&p).unwrap().len();
        }
        assert!(total > 10_000, "the random cases are not trivially empty ({total})");
    }

    #[test]
    fn h5_nearest() {
        let outline =
            Outline::LShape { size: v(1200.0, 900.0), notch: v(400.0, 300.0), notch_corner: RectCorner::XPosYPos };
        let set = holes(outline, HoleGrid::metric());
        // A tie between (387.5, 137.5) and (412.5, 137.5): the smaller i wins.
        assert_eq!(set.position(set.nearest(v(400.0, 300.0)).unwrap()), v(387.5, 137.5));
        let mut grid = HoleGrid::metric();
        grid.exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 50.0 },
        });
        let set = holes(rect(1200.0, 900.0), grid);
        assert_eq!(set.position(set.nearest(DVec2::ZERO).unwrap()), v(-37.5, -37.5));
        assert_eq!(HoleSet::empty().nearest(DVec2::ZERO), None);
    }

    #[test]
    fn h6_limits() {
        let grid_error = |outline: Outline, grid: HoleGrid| match compute_holes(&platform(outline, grid)) {
            Err(PlatformError::Grid(e)) => e,
            other => panic!("expected a grid error, got {other:?}"),
        };
        let mut grid = HoleGrid::metric();
        grid.pitch = v(25.0, 6.9);
        assert_eq!(grid_error(rect(1200.0, 900.0), grid), GridError::Pitch { min: 7.0 });
        let mut grid = HoleGrid::metric();
        grid.margin = 2.9;
        assert_eq!(grid_error(rect(1200.0, 900.0), grid), GridError::Margin { min: 3.0 });
        // 3 mm is below every thread's minimum pitch (M4: 5 mm), so it is a pitch error ...
        let mut grid = HoleGrid { thread: Thread::M4, margin: 2.0, ..HoleGrid::metric() };
        grid.pitch = DVec2::splat(3.0);
        assert_eq!(grid_error(rect(20_000.0, 20_000.0), grid.clone()), GridError::Pitch { min: 5.0 });
        // ... and the densest valid grid on 20 m × 20 m is too dense.
        grid.pitch = DVec2::splat(5.0);
        assert!(matches!(grid_error(rect(20_000.0, 20_000.0), grid), GridError::TooDense { cols: 4000, rows: 4000 }));
        // A platform too small for its margin has no holes.
        let set = holes(rect(60.0, 60.0), HoleGrid { margin: 31.0, ..HoleGrid::metric() });
        assert!(set.is_empty());
        assert_eq!(set.summary(), "No holes");
        // Margin 30 keeps the single centre hole, which sits exactly at the limit.
        let set = holes(rect(60.0, 60.0), HoleGrid { margin: 30.0, ..HoleGrid::metric() });
        assert_eq!(set.len(), 1);
        assert_eq!(set.summary(), "1 hole · 1 × 1 grid");
        assert_eq!(set.hole_at(HoleRef { col: 0, row: 0 }), Some(DVec2::ZERO));
    }

    #[test]
    fn h7_performance() {
        let p = platform(Outline::RoundedRectangle { size: v(3600.0, 1500.0), radius: 50.0 }, HoleGrid::metric());
        let start = Instant::now();
        let set = compute_holes(&p).unwrap();
        let elapsed = start.elapsed();
        eprintln!("H7: 3600 × 1500 rounded r=50 at 25 mm took {elapsed:?}");
        assert_eq!((set.cols, set.rows), (144, 60));
        assert_eq!(set.len(), 144 * 60 - 12);
        assert!(elapsed.as_millis() < 200, "{elapsed:?}");
    }

    #[test]
    fn h8_labels_do_not_depend_on_exclusions() {
        let plain = holes(rect(1200.0, 900.0), HoleGrid::metric());
        let mut grid = HoleGrid::metric();
        // Covers the whole first column (x = -587.5).
        grid.exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Rect { center: v(-590.0, 0.0), size: v(10.0, 1000.0) },
        });
        let zoned = holes(rect(1200.0, 900.0), grid);
        assert_eq!(zoned.len(), 1728 - 36);
        assert_eq!((zoned.first, zoned.cols, zoned.rows), (plain.first, plain.cols, plain.rows));
        for row in 0..36 {
            assert!(!zoned.contains_label(HoleRef { col: 0, row }));
        }
        for idx in zoned.iter() {
            assert_eq!(zoned.label(idx), plain.label(idx));
            assert_eq!(zoned.position(idx), plain.position(idx));
        }
    }

    #[test]
    fn hole_set_accessors() {
        let set = holes(rect(1200.0, 900.0), HoleGrid { origin: GridOrigin::At(v(12.5, 12.5)), ..HoleGrid::metric() });
        assert_eq!(set.first, LatticeIndex { i: -24, j: -18 });
        assert_eq!(set.len(), 1728);
        let a1 = set.index_of(HoleRef { col: 0, row: 0 });
        assert_eq!(a1, LatticeIndex { i: -24, j: -18 });
        assert_eq!(set.position(a1), v(-587.5, -437.5));
        assert_eq!(set.label(LatticeIndex { i: 0, j: 0 }), Some(HoleRef { col: 24, row: 18 }));
        assert_eq!(set.label(LatticeIndex { i: 24, j: 0 }), None);
        assert!(!set.contains(LatticeIndex { i: 24, j: 0 }));
        assert_eq!(set.iter().next(), Some(a1));
        assert_eq!(set.iter().last(), Some(LatticeIndex { i: 23, j: 17 }));
        let mut sorted: Vec<_> = set.iter().collect();
        sorted.sort_by_key(|x| (x.j, x.i));
        assert_eq!(set.iter().collect::<Vec<_>>(), sorted);
        let empty = HoleSet::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);
        assert!(!empty.contains_label(HoleRef { col: 0, row: 0 }));
        let none = Platform { hole_grid: None, ..platform(rect(1200.0, 900.0), HoleGrid::metric()) };
        assert_eq!(compute_holes(&none).unwrap(), HoleSet::empty());
    }

    #[test]
    fn standards_and_threads() {
        assert_eq!(HoleGrid::metric().standard(), GridStandard::Metric);
        assert_eq!(HoleGrid::imperial().standard(), GridStandard::Imperial);
        let mut grid = HoleGrid::metric();
        grid.exclusions
            .push(ExclusionZone { name: "Z".into(), shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 5.0 } });
        assert_eq!(grid.standard(), GridStandard::Metric);
        grid.origin = GridOrigin::At(DVec2::ZERO);
        assert_eq!(grid.standard(), GridStandard::Custom);
        assert_eq!(HoleGrid { thread: Thread::M4, ..HoleGrid::metric() }.standard(), GridStandard::Custom);
        for (i, t) in Thread::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(Thread::from_index(i), Some(*t));
            assert_eq!(t.radius() * 2.0, t.diameter());
        }
        assert_eq!(Thread::from_index(4), None);
        assert_eq!(Thread::Unc1_4_20.label(), "1/4-20");
    }

    #[test]
    fn auto_origin_and_candidate_range() {
        let grid = HoleGrid::metric();
        let bounds = (v(-600.0, -450.0), v(600.0, 450.0));
        assert_eq!(lattice_origin(&grid, bounds), v(-587.5, -437.5));
        assert_eq!(candidate_range(&grid, bounds), (0..=47, 0..=35));
        // Off-centre bounds keep the pattern centred on the box.
        let bounds = (v(100.0, 0.0), v(160.0, 60.0));
        assert_eq!(lattice_origin(&grid, bounds), v(117.5, 17.5));
        assert_eq!(candidate_range(&grid, bounds), (0..=1, 0..=1));
        // Too small for the margin: centred, empty range.
        let grid = HoleGrid { margin: 31.0, ..HoleGrid::metric() };
        assert_eq!(lattice_origin(&grid, bounds), v(130.0, 30.0));
        assert!(candidate_range(&grid, bounds).0.is_empty());
    }
}
