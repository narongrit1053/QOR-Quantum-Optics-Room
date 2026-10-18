//! Validation rules and their user-facing errors (M1-SPEC §3.6), plus the non-fatal repairs run
//! on load.

use std::collections::BTreeMap;

use glam::{DQuat, DVec2};

use crate::holes::{GridOrigin, HoleGrid, ZoneShape, candidate_range, range_len};
use crate::limits::{
    LEG_SIZE, MARGIN_MAX, MAX_COORD, MAX_ID, MAX_LATTICE_AXIS, MAX_LATTICE_POINTS, MAX_ZONES, NAME_MAX_CHARS, PITCH,
    PITCH_CLEARANCE, THICKNESS, TOP_HEIGHT_MAX,
};
use crate::model::{Document, Platform, Support};
use crate::outline::OutlineError;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlatformError {
    #[error("Name cannot be empty")]
    EmptyName,
    #[error("Name is longer than 128 characters")]
    NameTooLong,
    #[error("{0}")]
    Outline(OutlineError),
    #[error("Thickness must be between 3 and 1000 mm")]
    Thickness,
    #[error("Top height must be at least the thickness and at most 5000 mm")]
    TopHeight,
    #[error("Position or rotation is not a valid number or is out of range")]
    Placement,
    #[error("{0}")]
    Grid(GridError),
    #[error("Legs: size must be 10–500 mm and the inset at least half the size")]
    Legs,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GridError {
    #[error("Pitch must be between 3 and 500 mm and at least {min:.2} mm for this thread")]
    Pitch { min: f64 },
    #[error("Edge margin must be at least {min:.2} mm (the hole radius) and at most 1000 mm")]
    Margin { min: f64 },
    #[error("Grid origin is out of range")]
    Origin,
    #[error("The grid is too dense: {cols} × {rows} positions (limit 2046 per side and 250,000 in total)")]
    TooDense { cols: u64, rows: u64 },
    #[error("At most 16 exclusion zones")]
    TooManyZones,
    #[error("Exclusion zone {} is invalid: {reason}", .zone + 1)]
    Zone { zone: usize, reason: String },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RoomError {
    #[error("Room outline {0}")]
    Outline(lab_geom::PolygonError),
    #[error("Room corner {} is too sharp or too reflex ({degrees:.0}°; allowed 20°–340°)", .corner + 1)]
    Angle { corner: usize, degrees: f64 },
    #[error("Wall height must be 500–10000 mm")]
    Height,
    #[error("Wall thickness must be 20–1000 mm")]
    Thickness,
    #[error("Wall {} is too short for its thickness", .wall + 1)]
    WallTooShort { wall: usize },
    #[error("{name} is on wall {}, which does not exist", .wall + 1)]
    UnknownWall { name: String, wall: u32 },
    #[error("{name} does not fit on wall {}: it must lie between {min:.0} and {max:.0} mm from the wall start", .wall + 1)]
    DoesNotFit { name: String, wall: u32, min: f64, max: f64 },
    #[error("{a} and {b} overlap")]
    Overlap { a: String, b: String },
    #[error(
        "{name}: width and height must be at least 300 mm, the sill at least 0, and the top at most the wall height"
    )]
    OpeningSize { name: String },
    #[error("At most 32 openings")]
    TooManyOpenings,
    #[error("Opening ids must be unique")]
    DuplicateOpeningId,
    #[error("Corners need at least 3 entries")]
    TooFewCorners,
    /// Not in M1-SPEC §3.6: a corner index past the end (a stale index from the UI).
    #[error("Room corner {} does not exist", .corner + 1)]
    CornerOutOfRange { corner: usize },
}

/// A name is valid when its trimmed form has 1..=NAME_MAX_CHARS characters.
fn name_problem(name: &str) -> Option<&'static str> {
    let count = name.trim().chars().count();
    if count == 0 {
        Some("the name cannot be empty")
    } else if count > NAME_MAX_CHARS {
        Some("the name is longer than 128 characters")
    } else {
        None
    }
}

fn in_coord_range(p: DVec2) -> bool {
    p.is_finite() && p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD
}

/// Grid rules for an outline with bounding box `bounds`: pitch, margin, origin, density, zones.
pub(crate) fn validate_grid(grid: &HoleGrid, bounds: (DVec2, DVec2)) -> Result<(), GridError> {
    let min_pitch = (grid.thread.diameter() + PITCH_CLEARANCE).max(*PITCH.start());
    let pitch_ok = |p: f64| PITCH.contains(&p) && p >= min_pitch;
    if !(pitch_ok(grid.pitch.x) && pitch_ok(grid.pitch.y)) {
        return Err(GridError::Pitch { min: min_pitch });
    }
    let min_margin = grid.thread.radius();
    if !(grid.margin >= min_margin && grid.margin <= MARGIN_MAX) {
        return Err(GridError::Margin { min: min_margin });
    }
    if let GridOrigin::At(p) = grid.origin
        && !in_coord_range(p)
    {
        return Err(GridError::Origin);
    }
    let (ri, rj) = candidate_range(grid, bounds);
    let (cols, rows) = (range_len(&ri), range_len(&rj));
    if cols > MAX_LATTICE_AXIS || rows > MAX_LATTICE_AXIS || cols * rows > MAX_LATTICE_POINTS {
        return Err(GridError::TooDense { cols, rows });
    }
    if grid.exclusions.len() > MAX_ZONES {
        return Err(GridError::TooManyZones);
    }
    for (zone, z) in grid.exclusions.iter().enumerate() {
        let invalid = |reason: &str| GridError::Zone { zone, reason: reason.into() };
        if let Some(problem) = name_problem(&z.name) {
            return Err(invalid(problem));
        }
        match z.shape {
            ZoneShape::Rect { center, size } => {
                if !in_coord_range(center) {
                    return Err(invalid("the centre is not a valid number or is out of range"));
                }
                if !(size.is_finite() && size.x > 0.0 && size.y > 0.0) {
                    return Err(invalid("the size must be greater than 0"));
                }
            }
            ZoneShape::Circle { center, radius } => {
                if !in_coord_range(center) {
                    return Err(invalid("the centre is not a valid number or is out of range"));
                }
                if !(radius.is_finite() && radius > 0.0) {
                    return Err(invalid("the radius must be greater than 0"));
                }
            }
        }
    }
    Ok(())
}

impl Platform {
    /// Every platform rule, in order: name, outline, thickness, top height, placement, grid,
    /// legs.
    pub fn validate(&self) -> Result<(), PlatformError> {
        let count = self.name.trim().chars().count();
        if count == 0 {
            return Err(PlatformError::EmptyName);
        }
        if count > NAME_MAX_CHARS {
            return Err(PlatformError::NameTooLong);
        }
        self.outline.validate().map_err(PlatformError::Outline)?;
        if !THICKNESS.contains(&self.thickness) {
            return Err(PlatformError::Thickness);
        }
        if !(self.top_height >= self.thickness && self.top_height <= TOP_HEIGHT_MAX) {
            return Err(PlatformError::TopHeight);
        }
        if !(in_coord_range(self.position) && self.rotation_z.is_finite()) {
            return Err(PlatformError::Placement);
        }
        if let Some(grid) = &self.hole_grid {
            validate_grid(grid, self.outline.bounds()).map_err(PlatformError::Grid)?;
        }
        if let Support::Legs { size, inset } = self.support
            && !(LEG_SIZE.contains(&size) && inset >= size / 2.0 && inset.is_finite())
        {
            return Err(PlatformError::Legs);
        }
        Ok(())
    }
}

/// `text` with its first character lower-cased, for "Item: message" lines.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Document {
    /// Fatal problems, human-readable and naming the item: duplicate ids across nodes, platforms
    /// and openings, ids above MAX_ID, nodes on missing platforms, platform and room errors.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        let mut seen: BTreeMap<u64, usize> = BTreeMap::new();
        let ids = self
            .platforms
            .iter()
            .map(|p| p.id.0)
            .chain(self.room.openings.iter().map(|o| o.id.0))
            .chain(self.nodes.iter().map(|n| n.id.0));
        for id in ids {
            *seen.entry(id).or_default() += 1;
        }
        for (&id, &count) in &seen {
            if count > 1 {
                problems.push(format!("Id {id} is used by {count} items"));
            }
            if id > MAX_ID {
                problems.push(format!("Id {id} is too large"));
            }
        }
        if let Err(e) = self.room.validate() {
            problems.push(format!("Room: {e}"));
        }
        for p in &self.platforms {
            if let Err(e) = p.validate() {
                problems.push(format!("Platform “{}”: {}", p.name, lower_first(&e.to_string())));
            }
        }
        for n in &self.nodes {
            if self.platform(n.platform).is_none() {
                problems.push(format!("Part “{}” is on a platform that does not exist", n.name));
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    /// Non-fatal fixes, returned as notes: `next_id` above every id, unit node rotations, and
    /// edge margins at least the hole radius.
    pub(crate) fn repair(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let max = self.max_id();
        if self.next_free_id() <= max {
            self.set_next_id(max.saturating_add(1));
            notes.push(format!("The next free id was raised to {}.", max.saturating_add(1)));
        }
        for n in &mut self.nodes {
            let length = n.rotation.length();
            if (length - 1.0).abs() > 1e-6 {
                n.rotation =
                    if length.is_finite() && length > 1e-12 { n.rotation.normalize() } else { DQuat::IDENTITY };
                notes.push(format!("The rotation of “{}” was normalised.", n.name));
            }
        }
        for p in &mut self.platforms {
            if let Some(grid) = &mut p.hole_grid {
                let radius = grid.thread.radius();
                if grid.margin < radius {
                    grid.margin = radius;
                    notes.push(format!(
                        "The edge margin of “{}” was raised to the hole radius ({radius:.2} mm).",
                        p.name
                    ));
                }
            }
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holes::{ExclusionZone, Thread};
    use crate::model::{NodeId, OpeningId, PlatformId, PlatformKind, Surface};
    use crate::outline::Outline;
    use crate::room::Opening;
    use crate::sample::demo_document;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn table() -> Platform {
        Platform {
            id: PlatformId(1),
            name: "Table A".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: v(1200.0, 900.0) },
            thickness: 210.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(HoleGrid::metric()),
            support: Support::Legs { size: 120.0, inset: 120.0 },
        }
    }

    fn err(p: Platform) -> PlatformError {
        p.validate().unwrap_err()
    }

    #[test]
    fn platform_rules() {
        assert_eq!(table().validate(), Ok(()));
        assert_eq!(err(Platform { name: "  ".into(), ..table() }), PlatformError::EmptyName);
        assert_eq!(err(Platform { name: "x".repeat(129), ..table() }), PlatformError::NameTooLong);
        assert!(Platform { name: format!(" {} ", "é".repeat(128)), ..table() }.validate().is_ok());
        assert!(matches!(
            err(Platform { outline: Outline::Rectangle { size: v(10.0, 900.0) }, ..table() }),
            PlatformError::Outline(OutlineError::SizeOutOfRange { .. })
        ));
        assert_eq!(err(Platform { thickness: 2.0, ..table() }), PlatformError::Thickness);
        assert_eq!(err(Platform { thickness: f64::NAN, ..table() }), PlatformError::Thickness);
        assert_eq!(err(Platform { top_height: 200.0, ..table() }), PlatformError::TopHeight);
        assert_eq!(err(Platform { top_height: 5000.1, ..table() }), PlatformError::TopHeight);
        assert_eq!(err(Platform { position: v(100_001.0, 0.0), ..table() }), PlatformError::Placement);
        assert_eq!(err(Platform { rotation_z: f64::INFINITY, ..table() }), PlatformError::Placement);
        assert_eq!(
            err(Platform { support: Support::Legs { size: 9.0, inset: 120.0 }, ..table() }),
            PlatformError::Legs
        );
        assert_eq!(
            err(Platform { support: Support::Legs { size: 120.0, inset: 59.0 }, ..table() }),
            PlatformError::Legs
        );
        assert!(Platform { support: Support::Fixed, hole_grid: None, ..table() }.validate().is_ok());
    }

    #[test]
    fn grid_rules() {
        let with = |grid: HoleGrid| match err(Platform { hole_grid: Some(grid), ..table() }) {
            PlatformError::Grid(e) => e,
            other => panic!("{other:?}"),
        };
        assert_eq!(with(HoleGrid { pitch: v(501.0, 25.0), ..HoleGrid::metric() }), GridError::Pitch { min: 7.0 });
        assert_eq!(with(HoleGrid { pitch: v(25.0, f64::NAN), ..HoleGrid::metric() }), GridError::Pitch { min: 7.0 });
        assert_eq!(with(HoleGrid { margin: 1001.0, ..HoleGrid::metric() }), GridError::Margin { min: 3.0 });
        let e = with(HoleGrid { thread: Thread::Unc1_4_20, margin: 3.0, pitch: v(25.4, 25.4), ..HoleGrid::metric() });
        assert_eq!(e.to_string(), "Edge margin must be at least 3.17 mm (the hole radius) and at most 1000 mm");
        assert_eq!(
            with(HoleGrid { origin: GridOrigin::At(v(f64::NAN, 0.0)), ..HoleGrid::metric() }),
            GridError::Origin
        );
        assert_eq!(with(HoleGrid { origin: GridOrigin::At(v(0.0, -2e5)), ..HoleGrid::metric() }), GridError::Origin);
        let zone = |shape| ExclusionZone { name: "Zone 1".into(), shape };
        let circle = ZoneShape::Circle { center: DVec2::ZERO, radius: 10.0 };
        let many = vec![zone(circle); 17];
        assert_eq!(with(HoleGrid { exclusions: many, ..HoleGrid::metric() }), GridError::TooManyZones);
        let zone_error = |z: ExclusionZone| with(HoleGrid { exclusions: vec![zone(circle), z], ..HoleGrid::metric() });
        assert!(matches!(
            zone_error(ExclusionZone { name: " ".into(), shape: circle }),
            GridError::Zone { zone: 1, .. }
        ));
        let e = zone_error(zone(ZoneShape::Circle { center: DVec2::ZERO, radius: 0.0 }));
        assert_eq!(e.to_string(), "Exclusion zone 2 is invalid: the radius must be greater than 0");
        assert!(matches!(
            zone_error(zone(ZoneShape::Rect { center: DVec2::ZERO, size: v(10.0, -1.0) })),
            GridError::Zone { .. }
        ));
        assert!(matches!(
            zone_error(zone(ZoneShape::Rect { center: v(f64::NAN, 0.0), size: v(10.0, 1.0) })),
            GridError::Zone { .. }
        ));
        let dense = Platform {
            outline: Outline::Rectangle { size: v(12_000.0, 12_000.0) },
            hole_grid: Some(HoleGrid { pitch: v(10.0, 30.0), ..HoleGrid::metric() }),
            ..table()
        };
        // 1198 columns is within the per-axis limit, but 1198 × 400 is above 250,000.
        assert_eq!(err(dense), PlatformError::Grid(GridError::TooDense { cols: 1198, rows: 400 }));
    }

    #[test]
    fn document_rules() {
        let doc = demo_document();
        assert_eq!(doc.validate(), Ok(()));
        let mut bad = doc.clone();
        bad.platforms[0].outline = Outline::Polygon {
            corners: [v(0.0, 0.0), v(500.0, 500.0), v(500.0, 0.0), v(0.0, 500.0)]
                .map(crate::outline::OutlineCorner::sharp)
                .to_vec(),
        };
        bad.nodes[0].id = NodeId(bad.nodes[1].id.0);
        bad.nodes[2].platform = PlatformId(77);
        bad.room.openings.push(Opening {
            id: OpeningId(1),
            name: "Door 1".into(),
            wall: 0,
            offset: 600.0,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        });
        bad.nodes[3].id = NodeId(MAX_ID + 1);
        let problems = bad.validate().unwrap_err();
        assert!(problems.contains(&"Platform “Table A”: the outline edges 1 and 3 cross".to_string()), "{problems:?}");
        assert!(problems.contains(&format!("Id {} is used by 2 items", bad.nodes[1].id.0)), "{problems:?}");
        assert!(problems.contains(&"Id 1 is used by 2 items".to_string()), "{problems:?}");
        assert!(problems.contains(&format!("Id {} is too large", MAX_ID + 1)), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("is on a platform that does not exist")), "{problems:?}");
        assert_eq!(problems.len(), 5, "{problems:?}");
        let mut room = doc.clone();
        room.room.wall_height = 20.0;
        assert_eq!(room.validate(), Err(vec!["Room: Wall height must be 500–10000 mm".to_string()]));
    }

    #[test]
    fn repairs() {
        let mut doc = demo_document();
        assert!(doc.repair().is_empty());
        doc.set_next_id(3);
        doc.nodes[0].rotation = DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        doc.nodes[1].rotation = DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0);
        doc.platforms[0].hole_grid.as_mut().unwrap().margin = 1.0;
        let notes = doc.repair();
        assert_eq!(notes.len(), 4, "{notes:?}");
        assert_eq!(doc.next_free_id(), doc.max_id() + 1);
        assert_eq!(doc.nodes[0].rotation, DQuat::IDENTITY);
        assert_eq!(doc.nodes[1].rotation, DQuat::IDENTITY);
        assert_eq!(doc.platforms[0].hole_grid.as_ref().unwrap().margin, 3.0);
        assert!(doc.repair().is_empty());
        assert_eq!(doc.validate(), Ok(()));
    }
}
