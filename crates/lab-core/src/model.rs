//! Document types: room, platforms (tables/breadboards) and placed nodes.

use std::fmt;

use glam::{DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::units::{INCH, Mm};

/// Bumped whenever the serialized layout changes; older files are migrated on load.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlatformId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    pub room: Room,
    pub platforms: Vec<Platform>,
    pub nodes: Vec<Node>,
    next_id: u64,
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            room: Room::default(),
            platforms: Vec::new(),
            nodes: Vec::new(),
            next_id: 1,
        }
    }

    /// Returns a fresh id, unique across nodes and platforms in this document.
    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn node_index(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    pub fn platform(&self, id: PlatformId) -> Option<&Platform> {
        self.platforms.iter().find(|p| p.id == id)
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, LoadError> {
        let doc: Document = serde_json::from_str(json)?;
        if doc.schema_version > SCHEMA_VERSION {
            return Err(LoadError::NewerSchema(doc.schema_version));
        }
        Ok(doc)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("project file is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project was saved by a newer LabSetup (schema {0})")]
    NewerSchema(u32),
}

/// The lab room. A rectangle for now; arbitrary outlines and openings come later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    /// Floor size along room X and Y, centred on the room origin.
    pub size: DVec2,
    pub height: Mm,
}

impl Default for Room {
    fn default() -> Self {
        Self { size: DVec2::new(6000.0, 5000.0), height: 3000.0 }
    }
}

/// An optical table, breadboard or custom platform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub id: PlatformId,
    pub name: String,
    /// Top-surface size along platform X and Y. Rectangular for now.
    pub size: DVec2,
    pub thickness: Mm,
    /// Height of the top surface above the floor.
    pub top_height: Mm,
    /// Centre of the platform in room coordinates.
    pub position: DVec2,
    /// Rotation about room +Z.
    pub rotation_z: f64,
    pub hole_grid: Option<HoleGrid>,
    pub support: Support,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Support {
    /// Four legs, inset from the corners, reaching the floor.
    Legs { size: Mm, inset: Mm },
    /// Sits directly on whatever is below it (e.g. a breadboard on a table).
    Fixed,
}

/// A regular grid of tapped holes on a platform's top surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleGrid {
    pub pitch: Mm,
    /// Distance from the platform edge to the first hole centre.
    pub margin: Mm,
    pub thread: Thread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Thread {
    M4,
    M6,
    Unc8_32,
    Unc1_4_20,
}

impl Thread {
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
}

impl HoleGrid {
    /// Metric table standard: M6 on 25 mm centres.
    pub fn metric() -> Self {
        Self { pitch: 25.0, margin: 12.5, thread: Thread::M6 }
    }

    /// Imperial table standard: 1/4-20 on 1 inch centres.
    pub fn imperial() -> Self {
        Self { pitch: INCH, margin: 0.5 * INCH, thread: Thread::Unc1_4_20 }
    }

    /// Number of hole columns (along X) and rows (along Y) that fit on a surface of `size`.
    pub fn counts(&self, size: DVec2) -> (u32, u32) {
        let fit = |len: Mm| {
            let usable = len - 2.0 * self.margin;
            if usable < 0.0 { 0 } else { (usable / self.pitch + 1e-9).floor() as u32 + 1 }
        };
        (fit(size.x), fit(size.y))
    }

    /// Centre of hole (`col`, `row`) in platform coordinates, or `None` if it is off the grid.
    pub fn hole_position(&self, size: DVec2, hole: HoleRef) -> Option<DVec2> {
        let (cols, rows) = self.counts(size);
        if hole.col >= cols || hole.row >= rows {
            return None;
        }
        Some(DVec2::new(
            -size.x / 2.0 + self.margin + hole.col as f64 * self.pitch,
            -size.y / 2.0 + self.margin + hole.row as f64 * self.pitch,
        ))
    }

    /// The hole closest to `point` (platform coordinates), clamped to the grid.
    pub fn nearest_hole(&self, size: DVec2, point: DVec2) -> Option<HoleRef> {
        let (cols, rows) = self.counts(size);
        if cols == 0 || rows == 0 {
            return None;
        }
        let index = |p: f64, half: f64, count: u32| {
            let i = ((p + half - self.margin) / self.pitch).round();
            i.clamp(0.0, (count - 1) as f64) as u32
        };
        Some(HoleRef { col: index(point.x, size.x / 2.0, cols), row: index(point.y, size.y / 2.0, rows) })
    }
}

/// A hole on a platform grid. Displayed as row letter + column number, e.g. `C14`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HoleRef {
    pub col: u32,
    pub row: u32,
}

impl fmt::Display for HoleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Spreadsheet-style row letters: A..Z, AA..AZ, ...
        let mut letters = Vec::new();
        let mut n = self.row + 1;
        while n > 0 {
            n -= 1;
            letters.push(b'A' + (n % 26) as u8);
            n /= 26;
        }
        letters.reverse();
        write!(f, "{}{}", String::from_utf8_lossy(&letters), self.col + 1)
    }
}

/// A placed object. In M0 every node carries a primitive shape; catalog parts with meshes and
/// connectors replace this in M2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    /// Catalog part id, e.g. `generic:post-12.7`.
    pub part: String,
    pub platform: PlatformId,
    /// Centre of the shape, relative to the centre of the platform's top surface.
    pub position: DVec3,
    pub rotation: DQuat,
    pub shape: Shape,
    pub finish: Finish,
    pub pickable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Box { size: DVec3 },
    /// Axis along local +Z, centred on the node position.
    Cylinder { radius: Mm, length: Mm },
}

impl Shape {
    /// Half-extent along local +Z, i.e. how far the shape reaches above its centre.
    pub fn half_height(&self) -> Mm {
        match self {
            Shape::Box { size } => size.z / 2.0,
            Shape::Cylinder { length, .. } => length / 2.0,
        }
    }
}

/// Surface finish. The viewport decides how each finish is rendered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Finish {
    AnodizedBlack,
    Stainless,
    Mirror,
    Glass,
    Painted { rgb: [u8; 3] },
    /// A visualised laser beam (not a physical part).
    Beam { wavelength_nm: f64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_grid_counts_and_positions() {
        let grid = HoleGrid::metric();
        let size = DVec2::new(1200.0, 900.0);
        assert_eq!(grid.counts(size), (48, 36));
        let first = grid.hole_position(size, HoleRef { col: 0, row: 0 }).unwrap();
        assert_eq!(first, DVec2::new(-587.5, -437.5));
        let last = grid.hole_position(size, HoleRef { col: 47, row: 35 }).unwrap();
        assert_eq!(last, DVec2::new(587.5, 437.5));
        assert!(grid.hole_position(size, HoleRef { col: 48, row: 0 }).is_none());
    }

    #[test]
    fn imperial_grid_fits_whole_inches() {
        let grid = HoleGrid::imperial();
        // 4 ft x 2 ft breadboard: 48 x 24 holes.
        assert_eq!(grid.counts(DVec2::new(48.0 * INCH, 24.0 * INCH)), (48, 24));
    }

    #[test]
    fn nearest_hole_snaps_and_clamps() {
        let grid = HoleGrid::metric();
        let size = DVec2::new(1200.0, 900.0);
        let hole = grid.nearest_hole(size, DVec2::new(-580.0, -440.0)).unwrap();
        assert_eq!(hole, HoleRef { col: 0, row: 0 });
        let far = grid.nearest_hole(size, DVec2::new(10_000.0, 10_000.0)).unwrap();
        assert_eq!(far, HoleRef { col: 47, row: 35 });
        let p = grid.hole_position(size, HoleRef { col: 20, row: 10 }).unwrap();
        assert_eq!(grid.nearest_hole(size, p + DVec2::splat(9.0)).unwrap(), HoleRef { col: 20, row: 10 });
    }

    #[test]
    fn hole_labels() {
        assert_eq!(HoleRef { col: 13, row: 2 }.to_string(), "C14");
        assert_eq!(HoleRef { col: 0, row: 25 }.to_string(), "Z1");
        assert_eq!(HoleRef { col: 0, row: 26 }.to_string(), "AA1");
    }
}
