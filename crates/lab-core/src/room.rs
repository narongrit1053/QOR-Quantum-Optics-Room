//! The room: a polygonal inner wall face, wall dimensions, and doors/windows (M1-SPEC §3.5, D8).
//!
//! The outline is the inner face of the walls; walls extrude outward. Openings are positioned
//! by wall index and offset along the inner face from the wall's start corner. Shape edits refit
//! openings: one that no longer fits is moved, or removed if it cannot fit, and reported.

use std::collections::BTreeMap;

use glam::DVec2;
use lab_geom::{
    OpeningSpec, PolygonLimits, WallError, WallGeometry, build_walls, distance_to_segment, interior_angle,
    outward_normal, signed_area, validate_simple_polygon,
};
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_COORD, MAX_OPENINGS, OPENING_MIN_HEIGHT, OPENING_MIN_JAMB, OPENING_MIN_WIDTH, ROOM_ANGLE_DEG, ROOM_CORNERS,
    ROOM_MIN_WALL, WALL_HEIGHT, WALL_THICKNESS,
};
use crate::model::OpeningId;
use crate::units::Mm;
use crate::validate::RoomError;

/// Tolerance of the opening fit and spacing tests (as in `lab_geom::build_walls`).
const EPS: f64 = 1e-9;
/// Height of the wall footprint quads above the floor.
const FOOTPRINT_Z: f64 = 1.0;

const ROOM_LIMITS: PolygonLimits = PolygonLimits {
    min_corners: *ROOM_CORNERS.start(),
    max_corners: *ROOM_CORNERS.end(),
    min_edge: ROOM_MIN_WALL,
    max_abs_coord: MAX_COORD,
    min_area: 1e6,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    /// Inner face of the walls, in the order the corners were entered (either orientation).
    pub outline: Vec<DVec2>,
    pub wall_height: Mm,
    pub wall_thickness: Mm,
    pub openings: Vec<Opening>,
}

impl Default for Room {
    fn default() -> Self {
        Self {
            outline: vec![
                DVec2::new(-3000.0, -2500.0),
                DVec2::new(3000.0, -2500.0),
                DVec2::new(3000.0, 2500.0),
                DVec2::new(-3000.0, 2500.0),
            ],
            wall_height: 3000.0,
            wall_thickness: 150.0,
            openings: Vec::new(),
        }
    }
}

/// A door or window: `offset` and `width` along the inner face of `wall` from its start corner,
/// `sill` and `height` above the floor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: OpeningId,
    pub name: String,
    pub wall: u32,
    pub offset: Mm,
    pub width: Mm,
    pub height: Mm,
    pub sill: Mm,
}

/// What a shape edit did to an opening that no longer fitted.
#[derive(Clone, Debug, PartialEq)]
pub enum OpeningFix {
    Moved { id: OpeningId, name: String, wall: u32 },
    Removed { id: OpeningId, name: String },
}

const COMPASS: [&str; 8] = ["North", "North-east", "East", "South-east", "South", "South-west", "West", "North-west"];

impl Room {
    pub fn validate(&self) -> Result<(), RoomError> {
        self.check().map(|_| ())
    }

    /// Wall bodies and footprints (`build_walls` with OPENING_MIN_JAMB, footprint at z = 1 mm).
    pub fn walls(&self) -> Result<Vec<WallGeometry>, RoomError> {
        self.check()
    }

    fn opening_name(&self, k: usize) -> String {
        self.openings.get(k).map(|o| o.name.clone()).unwrap_or_default()
    }

    /// Every room rule (M1-SPEC §3.6); returns the walls it had to build anyway.
    fn check(&self) -> Result<Vec<WallGeometry>, RoomError> {
        validate_simple_polygon(&self.outline, &ROOM_LIMITS).map_err(RoomError::Outline)?;
        let n = self.outline.len();
        for corner in 0..n {
            let degrees = interior_angle(&self.outline, corner).to_degrees();
            if !ROOM_ANGLE_DEG.contains(&degrees) {
                return Err(RoomError::Angle { corner, degrees });
            }
        }
        if !WALL_HEIGHT.contains(&self.wall_height) {
            return Err(RoomError::Height);
        }
        if !WALL_THICKNESS.contains(&self.wall_thickness) {
            return Err(RoomError::Thickness);
        }
        if self.openings.len() > MAX_OPENINGS {
            return Err(RoomError::TooManyOpenings);
        }
        for (k, o) in self.openings.iter().enumerate() {
            if self.openings[..k].iter().any(|p| p.id == o.id) {
                return Err(RoomError::DuplicateOpeningId);
            }
        }
        for o in &self.openings {
            if o.wall as usize >= n {
                return Err(RoomError::UnknownWall { name: o.name.clone(), wall: o.wall });
            }
            let size_ok = o.width >= OPENING_MIN_WIDTH
                && o.height >= OPENING_MIN_HEIGHT
                && o.sill >= 0.0
                && o.sill + o.height <= self.wall_height + EPS;
            if !size_ok {
                return Err(RoomError::OpeningSize { name: o.name.clone() });
            }
        }
        let specs: Vec<OpeningSpec> = self
            .openings
            .iter()
            .map(|o| OpeningSpec {
                wall: o.wall as usize,
                offset: o.offset,
                width: o.width,
                sill: o.sill,
                height: o.height,
            })
            .collect();
        build_walls(&self.outline, self.wall_thickness, self.wall_height, &specs, OPENING_MIN_JAMB, FOOTPRINT_Z)
            .map_err(|e| match e {
                WallError::BadOutline(p) => RoomError::Outline(p),
                WallError::AngleOutOfRange { corner, degrees } => RoomError::Angle { corner, degrees },
                WallError::OuterFaceTooShort { wall } => RoomError::WallTooShort { wall },
                WallError::UnknownWall { opening, wall } => {
                    RoomError::UnknownWall { name: self.opening_name(opening), wall: wall as u32 }
                }
                WallError::OpeningDoesNotFit { opening, wall, min_offset, max_end } => RoomError::DoesNotFit {
                    name: self.opening_name(opening),
                    wall: wall as u32,
                    min: min_offset,
                    max: max_end,
                },
                WallError::OpeningsOverlap { a, b } => {
                    RoomError::Overlap { a: self.opening_name(a), b: self.opening_name(b) }
                }
                WallError::OpeningTooTall { opening } => RoomError::OpeningSize { name: self.opening_name(opening) },
            })
    }

    /// Length of the inner face of `wall`; 0 for a wall that does not exist.
    pub fn wall_length(&self, wall: usize) -> Mm {
        let n = self.outline.len();
        if wall >= n {
            return 0.0;
        }
        self.outline[wall].distance(self.outline[(wall + 1) % n])
    }

    /// Usable `[min_offset, max_end]` on the inner face of `wall`, jambs included.
    pub fn opening_limits(&self, wall: usize) -> (Mm, Mm) {
        lab_geom::opening_limits(&self.outline, self.wall_thickness, wall, OPENING_MIN_JAMB)
    }

    /// The direction `wall` faces (its outward normal), +Y = North, in 8 steps.
    pub fn wall_compass(&self, wall: usize) -> &'static str {
        let n = outward_normal(&self.outline, wall);
        let bearing = n.x.atan2(n.y).to_degrees().rem_euclid(360.0);
        COMPASS[((bearing / 45.0).round() as usize) % 8]
    }

    /// Outline bounding box expanded by the wall thickness.
    pub fn bounds_outer(&self) -> (DVec2, DVec2) {
        let (lo, hi) = lab_geom::bounds(&self.outline);
        let t = DVec2::splat(self.wall_thickness);
        (lo - t, hi + t)
    }

    /// The size of an axis-aligned 4-corner rectangle centred at the origin; `None` for any other
    /// shape.
    pub fn is_centered_rectangle(&self) -> Option<DVec2> {
        if self.outline.len() != 4 {
            return None;
        }
        let (lo, hi) = lab_geom::bounds(&self.outline);
        if (lo + hi).abs().max_element() > 1e-9 {
            return None;
        }
        let at_corner = |p: &DVec2| {
            ((p.x - lo.x).abs() <= 1e-9 || (p.x - hi.x).abs() <= 1e-9)
                && ((p.y - lo.y).abs() <= 1e-9 || (p.y - hi.y).abs() <= 1e-9)
        };
        let axis_aligned = (0..4).all(|i| {
            let d = self.outline[(i + 1) % 4] - self.outline[i];
            d.x.abs() <= 1e-9 || d.y.abs() <= 1e-9
        });
        (self.outline.iter().all(at_corner) && axis_aligned && (hi - lo).min_element() > 0.0).then_some(hi - lo)
    }

    /// Inserts the midpoint of edge `after → after + 1` as a new corner. An opening on the split
    /// wall stays on the first half if its centre lies there, else it moves to the second half.
    pub fn with_corner_inserted(&self, after: usize) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        let n = self.outline.len();
        if after >= n {
            return Err(RoomError::CornerOutOfRange { corner: after });
        }
        let start = self.outline[after];
        let mid = (start + self.outline[(after + 1) % n]) / 2.0;
        let half = mid.distance(start);
        let mut room = self.clone();
        room.outline.insert(after + 1, mid);
        for o in &mut room.openings {
            let w = o.wall as usize;
            if w > after && w < n {
                o.wall += 1;
            } else if w == after && o.offset + o.width / 2.0 > half {
                o.wall += 1;
                o.offset -= half;
            }
        }
        finish(room)
    }

    /// Removes corner `index`, merging its two walls; refuses below 3 corners. Openings on the
    /// merged walls are re-projected onto the new wall.
    pub fn with_corner_removed(&self, index: usize) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        let n = self.outline.len();
        if index >= n {
            return Err(RoomError::CornerOutOfRange { corner: index });
        }
        if n <= 3 {
            return Err(RoomError::TooFewCorners);
        }
        let k = index;
        let prev = (k + n - 1) % n;
        let new_index = |e: usize| if e < k { e } else { e - 1 };
        let merged = new_index(prev);
        let new_start = self.outline[prev];
        let new_dir = (self.outline[(k + 1) % n] - new_start).normalize_or_zero();
        let mut room = self.clone();
        room.outline.remove(k);
        for o in &mut room.openings {
            let w = o.wall as usize;
            if w >= n {
                continue;
            }
            if w == prev || w == k {
                let a = self.outline[w];
                let d = (self.outline[(w + 1) % n] - a).normalize_or_zero();
                o.offset = (a + d * o.offset - new_start).dot(new_dir);
                o.wall = merged as u32;
            } else {
                o.wall = new_index(w) as u32;
            }
        }
        finish(room)
    }

    /// Moves corner `index` to `to`. Openings keep their wall and offset (then refit).
    pub fn with_corner_moved(&self, index: usize, to: DVec2) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        if index >= self.outline.len() {
            return Err(RoomError::CornerOutOfRange { corner: index });
        }
        let mut room = self.clone();
        room.outline[index] = to;
        finish(room)
    }

    /// A rectangle of `size` centred at the origin, keeping the orientation and the quadrant of
    /// corner 0 so wall indices keep their meaning. A 4-corner room keeps every opening's wall
    /// and offset; other rooms move each opening to the facing wall nearest to it.
    pub fn with_rectangle(&self, size: DVec2) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        let h = size / 2.0;
        let quads = [DVec2::new(-h.x, -h.y), DVec2::new(h.x, -h.y), DVec2::new(h.x, h.y), DVec2::new(-h.x, h.y)];
        let (lo, hi) = lab_geom::bounds(&self.outline);
        let centre = (lo + hi) / 2.0;
        let q0 = self.outline.first().map_or(0, |p| match (p.x > centre.x, p.y > centre.y) {
            (false, false) => 0,
            (true, false) => 1,
            (true, true) => 2,
            (false, true) => 3,
        });
        let ccw = signed_area(&self.outline) >= 0.0;
        let outline: Vec<DVec2> = (0..4).map(|i| quads[if ccw { (q0 + i) % 4 } else { (q0 + 4 - i) % 4 }]).collect();
        if self.outline.len() == 4 {
            let mut room = self.clone();
            room.outline = outline;
            finish(room)
        } else {
            finish(self.reshaped(outline))
        }
    }

    /// An L-shaped room from the bounding box: a notch of 1/3 × 1/3 (whole mm) at +X+Y. Keeps the
    /// orientation; each opening moves to the facing wall nearest to it.
    pub fn with_l_shape(&self) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        let (lo, hi) = lab_geom::bounds(&self.outline);
        let s = hi - lo;
        let (a, b) = ((s.x / 3.0).round(), (s.y / 3.0).round());
        let mut outline = vec![
            DVec2::new(lo.x, lo.y),
            DVec2::new(hi.x, lo.y),
            DVec2::new(hi.x, hi.y - b),
            DVec2::new(hi.x - a, hi.y - b),
            DVec2::new(hi.x - a, hi.y),
            DVec2::new(lo.x, hi.y),
        ];
        if signed_area(&self.outline) < 0.0 {
            outline.reverse();
            outline.rotate_right(1);
        }
        finish(self.reshaped(outline))
    }

    /// The same room with new wall dimensions (openings refit to the new height and corners).
    pub fn with_dimensions(&self, height: Mm, thickness: Mm) -> Result<(Room, Vec<OpeningFix>), RoomError> {
        let mut room = self.clone();
        room.wall_height = height;
        room.wall_thickness = thickness;
        finish(room)
    }

    /// `self` with a new outline; each opening moves to the wall of the new outline that faces
    /// the same way and lies nearest to the opening's centre, keeping its position along the
    /// wall. An opening with no such wall keeps its wall and offset (refit decides).
    fn reshaped(&self, outline: Vec<DVec2>) -> Room {
        let old = &self.outline;
        let n_old = old.len();
        let n_new = outline.len();
        let mut room = self.clone();
        for o in &mut room.openings {
            let w = o.wall as usize;
            if w >= n_old {
                continue;
            }
            let a = old[w];
            let d = (old[(w + 1) % n_old] - a).normalize_or_zero();
            let start = a + d * o.offset;
            let end = a + d * (o.offset + o.width);
            let centre = (start + end) / 2.0;
            let facing = outward_normal(old, w);
            let best = (0..n_new)
                .filter(|&k| outward_normal(&outline, k).dot(facing) > 1.0 - 1e-6)
                .map(|k| (distance_to_segment(outline[k], outline[(k + 1) % n_new], centre), k))
                .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            if let Some((_, k)) = best {
                let na = outline[k];
                let nd = (outline[(k + 1) % n_new] - na).normalize_or_zero();
                o.wall = k as u32;
                o.offset = (start - na).dot(nd).min((end - na).dot(nd));
            }
        }
        room.outline = outline;
        room
    }

    /// Clamps each opening into its wall's limits (offset into `[min_offset, max_end - width]`),
    /// pushes overlapping openings right by order of offset (offset = previous end + jamb), and
    /// clamps the height to `wall_height - sill`. An opening that still does not fit, or whose
    /// height would fall below 300 mm, is removed. Deterministic and idempotent.
    pub fn refit_openings(&mut self) -> Vec<OpeningFix> {
        let n = self.outline.len();
        let count = self.openings.len();
        let mut removed = vec![false; count];
        let mut moved = vec![false; count];
        let mut per_wall: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (k, o) in self.openings.iter().enumerate() {
            if (o.wall as usize) < n {
                per_wall.entry(o.wall).or_default().push(k);
            } else {
                removed[k] = true;
            }
        }
        for (wall, list) in per_wall {
            let (min, max) = self.opening_limits(wall as usize);
            if !(min.is_finite() && max.is_finite()) {
                // A degenerate outline (validation rejects it afterwards): nothing fits.
                for &k in &list {
                    removed[k] = true;
                }
                continue;
            }
            for &k in &list {
                let o = &mut self.openings[k];
                let fits = o.offset >= min - EPS && o.offset + o.width <= max + EPS;
                if fits {
                    continue;
                }
                let last = max - o.width;
                if last.is_nan() || last < min - EPS || !o.offset.is_finite() {
                    removed[k] = true;
                    continue;
                }
                o.offset = o.offset.clamp(min, last.max(min));
                moved[k] = true;
            }
            let mut order: Vec<usize> = list.into_iter().filter(|&k| !removed[k]).collect();
            order.sort_by(|&p, &q| self.openings[p].offset.total_cmp(&self.openings[q].offset).then(p.cmp(&q)));
            let mut previous_end: Option<f64> = None;
            for k in order {
                let o = &mut self.openings[k];
                if let Some(end) = previous_end
                    && o.offset < end + OPENING_MIN_JAMB - EPS
                {
                    o.offset = end + OPENING_MIN_JAMB;
                    moved[k] = true;
                }
                if o.offset + o.width > max + EPS {
                    removed[k] = true;
                    continue;
                }
                previous_end = Some(o.offset + o.width);
            }
        }
        for (k, o) in self.openings.iter_mut().enumerate() {
            if removed[k] {
                continue;
            }
            if o.sill + o.height > self.wall_height + EPS {
                o.height = self.wall_height - o.sill;
                moved[k] = true;
            }
            if o.height.is_nan() || o.height < OPENING_MIN_HEIGHT {
                removed[k] = true;
            }
        }
        let fixes = self
            .openings
            .iter()
            .enumerate()
            .filter_map(|(k, o)| {
                if removed[k] {
                    Some(OpeningFix::Removed { id: o.id, name: o.name.clone() })
                } else if moved[k] {
                    Some(OpeningFix::Moved { id: o.id, name: o.name.clone(), wall: o.wall })
                } else {
                    None
                }
            })
            .collect();
        let mut k = 0;
        self.openings.retain(|_| {
            k += 1;
            !removed[k - 1]
        });
        fixes
    }
}

/// Refit, then validate (the shape helpers never fail because of an opening).
fn finish(mut room: Room) -> Result<(Room, Vec<OpeningFix>), RoomError> {
    let fixes = room.refit_openings();
    room.validate()?;
    Ok((room, fixes))
}

#[cfg(test)]
mod tests {
    use lab_geom::PolygonError;

    use super::*;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn door(id: u64, wall: u32, offset: f64) -> Opening {
        Opening {
            id: OpeningId(id),
            name: format!("Door {id}"),
            wall,
            offset,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        }
    }

    fn room_with(outline: Vec<DVec2>, openings: Vec<Opening>) -> Room {
        let room = Room { outline, openings, ..Room::default() };
        room.validate().unwrap();
        room
    }

    /// The default room with an extra (collinear) corner in the middle of the south wall.
    fn five_corners(openings: Vec<Opening>) -> Room {
        room_with(
            vec![v(-3000.0, -2500.0), v(0.0, -2500.0), v(3000.0, -2500.0), v(3000.0, 2500.0), v(-3000.0, 2500.0)],
            openings,
        )
    }

    fn assert_idempotent(room: &Room) {
        let mut again = room.clone();
        assert!(again.refit_openings().is_empty());
        assert_eq!(&again, room);
    }

    #[test]
    fn default_room_is_valid() {
        let room = Room::default();
        room.validate().unwrap();
        assert_eq!(room.walls().unwrap().len(), 4);
        assert_eq!(room.is_centered_rectangle(), Some(v(6000.0, 5000.0)));
        assert_eq!(room.wall_length(1), 5000.0);
        assert_eq!(room.wall_length(4), 0.0);
        assert_eq!(room.bounds_outer(), (v(-3150.0, -2650.0), v(3150.0, 2650.0)));
        let compass: Vec<_> = (0..4).map(|w| room.wall_compass(w)).collect();
        assert_eq!(compass, ["South", "East", "North", "West"]);
        assert_eq!(room.opening_limits(0), (50.0, 5950.0));
    }

    #[test]
    fn compass_eight_ways() {
        // Corners at -112.5° + k·45°: wall 0 faces south, then counter-clockwise.
        let octagon: Vec<DVec2> =
            (0..8).map(|k| DVec2::from_angle((k as f64 * 45.0 - 112.5).to_radians()) * 3000.0).collect();
        let room = room_with(octagon, vec![]);
        let names: Vec<_> = (0..8).map(|w| room.wall_compass(w)).collect();
        assert_eq!(names, ["South", "South-east", "East", "North-east", "North", "North-west", "West", "South-west"]);
    }

    #[test]
    fn validation_errors() {
        let room = Room { wall_height: 400.0, ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::Height));
        let room = Room { wall_thickness: 10.0, ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::Thickness));
        let room = Room { outline: vec![v(0.0, 0.0), v(50.0, 0.0), v(0.0, 3000.0)], ..Room::default() };
        assert!(matches!(room.validate(), Err(RoomError::Outline(PolygonError::EdgeTooShort { edge: 0, .. }))));
        // A 10° spike.
        let room = Room { outline: vec![v(0.0, 0.0), v(6000.0, 0.0), v(0.0, 1000.0)], ..Room::default() };
        assert!(matches!(room.validate(), Err(RoomError::Angle { corner: 1, .. })), "{:?}", room.validate());
        let room = Room { openings: vec![door(1, 7, 600.0)], ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::UnknownWall { name: "Door 1".into(), wall: 7 }));
        let room = Room { openings: vec![door(1, 0, 5500.0)], ..Room::default() };
        let err = room.validate().unwrap_err();
        assert_eq!(err, RoomError::DoesNotFit { name: "Door 1".into(), wall: 0, min: 50.0, max: 5950.0 });
        assert_eq!(
            err.to_string(),
            "Door 1 does not fit on wall 1: it must lie between 50 and 5950 mm from the wall start"
        );
        let room = Room { openings: vec![door(1, 0, 600.0), door(2, 0, 1620.0)], ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::Overlap { a: "Door 1".into(), b: "Door 2".into() }));
        let room = Room { openings: vec![door(1, 0, 600.0), door(1, 1, 600.0)], ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::DuplicateOpeningId));
        let mut small = door(1, 0, 600.0);
        small.width = 200.0;
        let room = Room { openings: vec![small], ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::OpeningSize { name: "Door 1".into() }));
        let mut tall = door(1, 0, 600.0);
        tall.height = 3001.0;
        let room = Room { openings: vec![tall], ..Room::default() };
        assert_eq!(room.validate(), Err(RoomError::OpeningSize { name: "Door 1".into() }));
        let many: Vec<Opening> = (0..33).map(|k| door(k + 1, 0, 0.0)).collect();
        assert_eq!(Room { openings: many, ..Room::default() }.validate(), Err(RoomError::TooManyOpenings));
        // A 200 mm slot cut into the north wall: the slot's walls close it from both sides.
        let room = Room {
            outline: vec![
                v(0.0, 0.0),
                v(6000.0, 0.0),
                v(6000.0, 5000.0),
                v(3100.0, 5000.0),
                v(3100.0, 4000.0),
                v(2900.0, 4000.0),
                v(2900.0, 5000.0),
                v(0.0, 5000.0),
            ],
            ..Room::default()
        };
        assert!(matches!(room.validate(), Err(RoomError::WallTooShort { .. })), "{:?}", room.validate());
    }

    #[test]
    fn r1_insert_splits_the_wall() {
        let room = room_with(Room::default().outline, vec![door(1, 0, 600.0), door(2, 0, 4000.0), door(3, 2, 600.0)]);
        let (r, fixes) = room.with_corner_inserted(0).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.outline[1], v(0.0, -2500.0));
        assert_eq!((r.openings[0].wall, r.openings[0].offset), (0, 600.0));
        assert_eq!((r.openings[1].wall, r.openings[1].offset), (1, 1000.0));
        assert_eq!((r.openings[2].wall, r.openings[2].offset), (3, 600.0));
        assert_idempotent(&r);
        // k = n - 1: the closing wall splits; the new corner is appended.
        let room = room_with(Room::default().outline, vec![door(1, 3, 600.0), door(2, 3, 3500.0)]);
        let (r, fixes) = room.with_corner_inserted(3).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.outline.len(), 5);
        assert_eq!(r.outline[4], v(-3000.0, 0.0));
        assert_eq!((r.openings[0].wall, r.openings[0].offset), (3, 600.0));
        assert_eq!((r.openings[1].wall, r.openings[1].offset), (4, 1000.0));
        assert_eq!(Room::default().with_corner_inserted(4), Err(RoomError::CornerOutOfRange { corner: 4 }));
    }

    #[test]
    fn r1_insert_moves_a_door_on_the_split_point() {
        // The door straddles the new corner: its centre is in the first half, so it stays on
        // wall 0 and is pushed back inside its (now shorter) wall.
        let room = room_with(Room::default().outline, vec![door(1, 0, 2400.0)]);
        let (r, fixes) = room.with_corner_inserted(0).unwrap();
        assert_eq!(fixes, vec![OpeningFix::Moved { id: OpeningId(1), name: "Door 1".into(), wall: 0 }]);
        assert_eq!((r.openings[0].wall, r.openings[0].offset), (0, 1950.0));
        assert_idempotent(&r);
    }

    #[test]
    fn r1_remove_merges_walls() {
        let room = five_corners(vec![door(1, 0, 600.0), door(2, 1, 500.0), door(3, 3, 600.0)]);
        let (r, fixes) = room.with_corner_removed(1).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.outline, Room::default().outline);
        assert_eq!((r.openings[0].wall, r.openings[0].offset), (0, 600.0));
        assert_eq!((r.openings[1].wall, r.openings[1].offset), (0, 3500.0));
        assert_eq!((r.openings[2].wall, r.openings[2].offset), (2, 600.0));
        assert_idempotent(&r);
        // k = 0: walls 4 and 0 merge into the new wall n - 2 (from old corner 4 to old corner 1).
        // Door 1 started 600 mm down the west wall; it is projected onto the diagonal.
        let room = five_corners(vec![door(1, 4, 600.0), door(2, 2, 600.0)]);
        let (r, fixes) = room.with_corner_removed(0).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.outline, vec![v(0.0, -2500.0), v(3000.0, -2500.0), v(3000.0, 2500.0), v(-3000.0, 2500.0)]);
        assert_eq!(r.openings[0].wall, 3);
        let along = v(0.0, -600.0).dot((v(0.0, -2500.0) - v(-3000.0, 2500.0)).normalize());
        assert!((r.openings[0].offset - along).abs() < 1e-9);
        assert_eq!((r.openings[1].wall, r.openings[1].offset), (1, 600.0));
        assert_idempotent(&r);
        // k = n - 1: walls 3 and 4 merge into wall 3, from old corner 3 to old corner 0.
        let (r, _) = room.with_corner_removed(4).unwrap();
        assert_eq!(r.outline.len(), 4);
        assert_eq!(r.openings[0].wall, 3);
        let along = (v(-3000.0, 2500.0) + v(0.0, -1.0) * 600.0 - v(3000.0, 2500.0))
            .dot((v(-3000.0, -2500.0) - v(3000.0, 2500.0)).normalize());
        assert!((r.openings[0].offset - along).abs() < 1e-9);
        let triangle = room_with(vec![v(0.0, 0.0), v(5000.0, 0.0), v(0.0, 5000.0)], vec![]);
        assert_eq!(triangle.with_corner_removed(0), Err(RoomError::TooFewCorners));
        assert_eq!(room.with_corner_removed(5), Err(RoomError::CornerOutOfRange { corner: 5 }));
    }

    #[test]
    fn r1_move_refits_or_removes() {
        let room = room_with(Room::default().outline, vec![door(1, 0, 4000.0), door(2, 1, 600.0)]);
        // Wall 0 shrinks to 4000 mm: door 1 moves left to fit.
        let (r, fixes) = room.with_corner_moved(1, v(1000.0, -2500.0)).unwrap();
        assert_eq!(fixes, vec![OpeningFix::Moved { id: OpeningId(1), name: "Door 1".into(), wall: 0 }]);
        let (_, max) = r.opening_limits(0);
        assert!((r.openings[0].offset + 1000.0 - max).abs() < 1e-9);
        assert_eq!(r.openings[1].offset, 600.0);
        assert_idempotent(&r);
        // Wall 1 too short for a 1 m door: removed.
        let (r, fixes) = room.with_corner_moved(2, v(3000.0, -1500.0)).unwrap();
        assert_eq!(fixes, vec![OpeningFix::Removed { id: OpeningId(2), name: "Door 2".into() }]);
        assert_eq!(r.openings.len(), 1);
        assert_idempotent(&r);
        // A bad move is refused, with the reason.
        assert!(matches!(room.with_corner_moved(0, v(3000.0, 2500.0)), Err(RoomError::Outline(_))));
        assert_eq!(room.with_corner_moved(4, DVec2::ZERO), Err(RoomError::CornerOutOfRange { corner: 4 }));
    }

    #[test]
    fn refit_pushes_overlaps_and_clamps_heights() {
        let mut room =
            Room { openings: vec![door(1, 0, 600.0), door(2, 0, 1000.0), door(3, 0, 5500.0)], ..Room::default() };
        let fixes = room.refit_openings();
        assert_eq!(
            fixes,
            vec![
                OpeningFix::Moved { id: OpeningId(2), name: "Door 2".into(), wall: 0 },
                OpeningFix::Moved { id: OpeningId(3), name: "Door 3".into(), wall: 0 },
            ]
        );
        assert_eq!(room.openings[1].offset, 1650.0);
        assert_eq!(room.openings[2].offset, 4950.0);
        room.validate().unwrap();
        assert_idempotent(&room);
        // A lower wall clamps the door height; a high window that would drop below 300 mm goes.
        let window = Opening {
            id: OpeningId(4),
            name: "Window 1".into(),
            wall: 2,
            offset: 600.0,
            width: 1200.0,
            height: 1200.0,
            sill: 900.0,
        };
        room.openings.push(window);
        let (r, fixes) = room.with_dimensions(1100.0, 150.0).unwrap();
        assert_eq!(fixes.len(), 4);
        assert!(fixes.contains(&OpeningFix::Removed { id: OpeningId(4), name: "Window 1".into() }));
        assert!(r.openings.iter().all(|o| o.height == 1100.0));
        assert_idempotent(&r);
        // Degenerate outlines: refit must not panic (NaN limits), and the helpers report the
        // outline error.
        let room = Room { openings: vec![door(1, 0, 600.0), door(2, 1, 600.0)], ..Room::default() };
        let mut broken = room.clone();
        broken.outline[1] = v(f64::NAN, 0.0);
        assert!(broken.opening_limits(0).1.is_nan());
        assert_eq!(broken.refit_openings().len(), 2);
        assert!(matches!(
            room.with_corner_moved(1, v(f64::NAN, 0.0)),
            Err(RoomError::Outline(PolygonError::NonFinite { corner: 1 }))
        ));
        assert!(matches!(
            room.with_corner_moved(2, v(0.0, -2500.0)),
            Err(RoomError::Outline(PolygonError::FoldBack { corner: 1 }))
        ));
        // Openings on walls that no longer exist are removed.
        let mut room = Room { openings: vec![door(9, 5, 600.0)], ..Room::default() };
        assert_eq!(room.refit_openings(), vec![OpeningFix::Removed { id: OpeningId(9), name: "Door 9".into() }]);
        assert!(room.openings.is_empty());
    }

    #[test]
    fn rectangle_and_l_helpers() {
        let room = room_with(Room::default().outline, vec![door(1, 0, 600.0), door(2, 2, 3500.0)]);
        let (r, fixes) = room.with_rectangle(v(8000.0, 4000.0)).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.is_centered_rectangle(), Some(v(8000.0, 4000.0)));
        assert_eq!(r.outline[0], v(-4000.0, -2000.0));
        assert_eq!(r.openings, room.openings);
        // A clockwise room stays clockwise, starting in the same quadrant.
        let mut cw = Room::default();
        cw.outline.reverse();
        let (r, _) = cw.with_rectangle(v(4000.0, 4000.0)).unwrap();
        assert!(signed_area(&r.outline) < 0.0);
        assert_eq!(r.outline[0], v(-2000.0, 2000.0));

        let (l, fixes) = room.with_l_shape().unwrap();
        assert!(fixes.is_empty());
        assert_eq!(l.outline.len(), 6);
        assert_eq!(l.outline[3], v(1000.0, 833.0));
        // The north door (x from -500 to -1500) moves onto the north wall of the L (wall 4,
        // from x = 1000 to -3000), at the same position.
        assert_eq!(l.openings[0], room.openings[0]);
        assert_eq!((l.openings[1].wall, l.openings[1].offset), (4, 1500.0));
        assert_idempotent(&l);
        // And back: the L's walls map to the rectangle's walls facing the same way.
        let (r, fixes) = l.with_rectangle(v(6000.0, 5000.0)).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(r.openings, room.openings);
        // A door in the part of the north wall that the notch removes is moved to fit.
        let room = room_with(Room::default().outline, vec![door(1, 2, 600.0)]);
        let (l, fixes) = room.with_l_shape().unwrap();
        assert_eq!(fixes, vec![OpeningFix::Moved { id: OpeningId(1), name: "Door 1".into(), wall: 4 }]);
        assert_eq!(l.openings[0].offset, 50.0);
        let mut cw = room.clone();
        cw.outline.reverse();
        cw.openings.clear();
        let (l, _) = cw.with_l_shape().unwrap();
        assert!(signed_area(&l.outline) < 0.0);
        assert_eq!(l.outline[0], v(-3000.0, -2500.0));
    }
}
