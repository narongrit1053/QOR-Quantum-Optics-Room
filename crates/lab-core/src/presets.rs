//! Platform presets, automatic naming and the File > New document (M1-SPEC §3.10).
//!
//! Hints carry a Thorlabs part number only where the spec lists one; T7 checks each against
//! thorlabs.com before release and blanks any it cannot confirm.

use glam::DVec2;

use crate::holes::{GridStandard, HoleGrid};
use crate::model::{Document, OpeningId, Platform, PlatformId, PlatformKind, Support, Surface};
use crate::outline::Outline;
use crate::room::Opening;
use crate::units::{INCH, Mm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetGroup {
    TableMetric,
    TableImperial,
    BreadboardMetric,
    BreadboardImperial,
    SteelBreadboardMetric,
    SteelBreadboardImperial,
    Custom,
}

#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub group: PresetGroup,
    /// "W × D unit", W ≥ D, W along +X.
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: PlatformKind,
    pub surface: Surface,
    /// Outline size in mm (W, D).
    pub size: (f64, f64),
    pub thickness: Mm,
    /// Metric or Imperial.
    pub grid: GridStandard,
    pub legs: bool,
}

const TABLE_HINT: &str = "Nexus-class optical table";
const STEEL_HINT: &str = "Steel honeycomb breadboard";
/// Leg size and inset for preset tables.
const LEG_SIZE: Mm = 120.0;
const LEG_INSET: Mm = 150.0;
/// Top height of a new table.
const TABLE_TOP: Mm = 800.0;
/// Gap kept between a new platform and the existing ones.
const PLACEMENT_GAP: Mm = 100.0;

const fn table(id: &'static str, label: &'static str, w: f64, d: f64, thickness: f64, imperial: bool) -> Preset {
    let (group, grid, scale) = if imperial {
        (PresetGroup::TableImperial, GridStandard::Imperial, INCH)
    } else {
        (PresetGroup::TableMetric, GridStandard::Metric, 1.0)
    };
    Preset {
        id,
        group,
        label,
        hint: TABLE_HINT,
        kind: PlatformKind::OpticalTable,
        surface: Surface::Stainless,
        size: (w * scale, d * scale),
        thickness,
        grid,
        legs: true,
    }
}

const fn board(id: &'static str, label: &'static str, hint: &'static str, w: f64, d: f64, imperial: bool) -> Preset {
    let (group, grid, scale) = if imperial {
        (PresetGroup::BreadboardImperial, GridStandard::Imperial, INCH)
    } else {
        (PresetGroup::BreadboardMetric, GridStandard::Metric, 1.0)
    };
    Preset {
        id,
        group,
        label,
        hint,
        kind: PlatformKind::Breadboard,
        surface: Surface::BlackAnodized,
        size: (w * scale, d * scale),
        thickness: 12.7,
        grid,
        legs: false,
    }
}

const fn steel(id: &'static str, label: &'static str, w: f64, d: f64, imperial: bool) -> Preset {
    let (group, grid, scale, thickness) = if imperial {
        (PresetGroup::SteelBreadboardImperial, GridStandard::Imperial, INCH, 61.0)
    } else {
        (PresetGroup::SteelBreadboardMetric, GridStandard::Metric, 1.0, 60.0)
    };
    Preset {
        id,
        group,
        label,
        hint: STEEL_HINT,
        kind: PlatformKind::Breadboard,
        surface: Surface::Stainless,
        size: (w * scale, d * scale),
        thickness,
        grid,
        legs: false,
    }
}

pub const PRESETS: &[Preset] = &[
    table("table-m-1200x900", "1200 × 900 mm", 1200.0, 900.0, 210.0, false),
    table("table-m-1500x1200", "1500 × 1200 mm", 1500.0, 1200.0, 210.0, false),
    table("table-m-1800x1200", "1800 × 1200 mm", 1800.0, 1200.0, 210.0, false),
    table("table-m-2400x1200", "2400 × 1200 mm", 2400.0, 1200.0, 310.0, false),
    table("table-m-2400x1500", "2400 × 1500 mm", 2400.0, 1500.0, 310.0, false),
    table("table-m-3000x1500", "3000 × 1500 mm", 3000.0, 1500.0, 310.0, false),
    table("table-m-3600x1500", "3600 × 1500 mm", 3600.0, 1500.0, 310.0, false),
    table("table-i-48x36", "48 × 36 in", 48.0, 36.0, 210.0, true),
    table("table-i-60x48", "60 × 48 in", 60.0, 48.0, 210.0, true),
    table("table-i-72x48", "72 × 48 in", 72.0, 48.0, 210.0, true),
    table("table-i-96x48", "96 × 48 in", 96.0, 48.0, 310.0, true),
    table("table-i-96x60", "96 × 60 in", 96.0, 60.0, 310.0, true),
    table("table-i-120x60", "120 × 60 in", 120.0, 60.0, 310.0, true),
    table("table-i-144x60", "144 × 60 in", 144.0, 60.0, 310.0, true),
    board("bb-m-150x150", "150 × 150 mm", "Thorlabs MB1515/M", 150.0, 150.0, false),
    board("bb-m-300x300", "300 × 300 mm", "Thorlabs MB3030/M", 300.0, 300.0, false),
    board("bb-m-450x300", "450 × 300 mm", "Thorlabs MB3045/M", 450.0, 300.0, false),
    board("bb-m-450x450", "450 × 450 mm", "Thorlabs MB4545/M", 450.0, 450.0, false),
    board("bb-m-600x450", "600 × 450 mm", "Thorlabs MB4560/M", 600.0, 450.0, false),
    board("bb-m-600x600", "600 × 600 mm", "Thorlabs MB6060/M", 600.0, 600.0, false),
    board("bb-m-900x600", "900 × 600 mm", "Thorlabs MB6090/M", 900.0, 600.0, false),
    board("bb-i-6x6", "6 × 6 in", "Thorlabs MB6", 6.0, 6.0, true),
    board("bb-i-12x6", "12 × 6 in", "", 12.0, 6.0, true), // MB612 is double-density (T7)
    board("bb-i-12x12", "12 × 12 in", "Thorlabs MB12", 12.0, 12.0, true),
    board("bb-i-18x12", "18 × 12 in", "Thorlabs MB1218", 18.0, 12.0, true),
    board("bb-i-24x18", "24 × 18 in", "Thorlabs MB1824", 24.0, 18.0, true),
    board("bb-i-24x24", "24 × 24 in", "Thorlabs MB2424", 24.0, 24.0, true),
    board("bb-i-36x24", "36 × 24 in", "Thorlabs MB2436", 36.0, 24.0, true),
    steel("bbs-m-600x450", "600 × 450 mm", 600.0, 450.0, false),
    steel("bbs-m-900x600", "900 × 600 mm", 900.0, 600.0, false),
    steel("bbs-m-1200x900", "1200 × 900 mm", 1200.0, 900.0, false),
    steel("bbs-i-24x18", "24 × 18 in", 24.0, 18.0, true),
    steel("bbs-i-36x24", "36 × 24 in", 36.0, 24.0, true),
    steel("bbs-i-48x36", "48 × 36 in", 48.0, 36.0, true),
    Preset {
        id: "custom",
        group: PresetGroup::Custom,
        label: "Custom",
        hint: "Edit the corners in the inspector",
        kind: PlatformKind::Custom,
        surface: Surface::Stainless,
        size: (900.0, 600.0),
        thickness: 60.0,
        grid: GridStandard::Metric,
        legs: false,
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

fn preset_outline(preset: &Preset) -> Outline {
    let size = DVec2::new(preset.size.0, preset.size.1);
    match preset.group {
        PresetGroup::Custom => Outline::Rectangle { size }.to_polygon(),
        _ => Outline::Rectangle { size },
    }
}

fn preset_grid(preset: &Preset) -> HoleGrid {
    match preset.grid {
        GridStandard::Imperial => HoleGrid::imperial(),
        GridStandard::Metric | GridStandard::Custom => HoleGrid::metric(),
    }
}

fn preset_support(preset: &Preset) -> Support {
    if preset.legs { Support::Legs { size: LEG_SIZE, inset: LEG_INSET } } else { Support::Fixed }
}

/// Distance between two room-space boxes in plan (0 when they overlap).
fn plan_gap(a: (glam::DVec3, glam::DVec3), b: (glam::DVec3, glam::DVec3)) -> f64 {
    let dx = (b.0.x - a.1.x).max(a.0.x - b.1.x).max(0.0);
    let dy = (b.0.y - a.1.y).max(a.0.y - b.1.y).max(0.0);
    DVec2::new(dx, dy).length()
}

/// Where a new platform goes on the floor: the room centre if it is clear of every other
/// platform by 100 mm, otherwise 100 mm to the right of the right-most platform, at y = 0.
fn free_position(doc: &Document, p: &Platform) -> DVec2 {
    let (lo, hi) = lab_geom::bounds(&doc.room.outline);
    let centre = (lo + hi) / 2.0;
    let centre = if centre.is_finite() { centre } else { DVec2::ZERO };
    let at_centre = Platform { position: centre, ..p.clone() }.room_aabb();
    if doc.platforms.iter().all(|q| plan_gap(at_centre, doc.platform_room_aabb(q.id).unwrap_or_else(|| q.room_aabb())) >= PLACEMENT_GAP) {
        return centre;
    }
    let right = doc.platforms.iter().map(|q| doc.platform_room_aabb(q.id).unwrap_or_else(|| q.room_aabb()).1.x).fold(f64::NEG_INFINITY, f64::max);
    let (local_lo, _) = p.outline.bounds();
    DVec2::new(right + PLACEMENT_GAP - local_lo.x, 0.0)
}

/// A new platform from `preset`, with id `id` (no id is allocated). Named with
/// [`next_platform_name`]. A non-table preset with `on` (the selected platform) sits on top of
/// it at the same position and rotation; otherwise tables stand 800 mm high and everything else
/// lies on the floor, at a free spot.
pub fn platform_from_preset(doc: &Document, preset: &Preset, id: PlatformId, on: Option<PlatformId>) -> Platform {
    let mut p = Platform { select_lock: false,
        id,
        name: next_platform_name(doc, preset.kind),
        kind: preset.kind,
        surface: preset.surface,
        outline: preset_outline(preset),
        thickness: preset.thickness,
        top_height: preset.thickness,
        position: DVec2::ZERO,
        rotation_z: 0.0,
        hole_grid: Some(preset_grid(preset)),
        support: preset_support(preset),
    };
    let is_table = preset.kind == PlatformKind::OpticalTable;
    match on.and_then(|base| doc.platform(base)) {
        Some(base) if !is_table => {
            p.position = base.position;
            p.rotation_z = base.rotation_z;
            p.top_height = base.top_height + p.thickness;
        }
        _ => {
            p.top_height = if is_table { TABLE_TOP } else { p.thickness };
            p.position = free_position(doc, &p);
        }
    }
    p
}

/// Replaces kind, surface, outline, thickness, grid (keeping existing exclusions) and support;
/// keeps id, name, position, rotation and top height.
pub fn apply_preset(platform: &Platform, preset: &Preset) -> Platform {
    let mut grid = preset_grid(preset);
    if let Some(old) = &platform.hole_grid {
        grid.exclusions = old.exclusions.clone();
    }
    Platform {
        kind: preset.kind,
        surface: preset.surface,
        outline: preset_outline(preset),
        thickness: preset.thickness,
        hole_grid: Some(grid),
        support: preset_support(preset),
        ..platform.clone()
    }
}

/// Spreadsheet-style letters: 0 → A, 25 → Z, 26 → AA.
fn letters(mut n: u64) -> String {
    let mut out = Vec::new();
    n += 1;
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// The first unused name of the kind's sequence: "Table A".."Table Z", "Table AA"…;
/// "Breadboard 1"…; "Platform 1"….
pub fn next_platform_name(doc: &Document, kind: PlatformKind) -> String {
    let used = |name: &str| doc.platforms.iter().any(|p| p.name.trim() == name);
    (0u64..)
        .map(|k| match kind {
            PlatformKind::OpticalTable => format!("Table {}", letters(k)),
            PlatformKind::Breadboard => format!("Breadboard {}", k + 1),
            PlatformKind::Custom => format!("Platform {}", k + 1),
        })
        .find(|name| !used(name))
        .expect("INVARIANT: an unbounded sequence has an unused name")
}

/// File > New: "Untitled" with the default room, "Door 1" on wall 1, and "Table A"
/// (1200 × 900 mm) at the origin.
pub fn default_document() -> Document {
    let mut doc = Document::new("Untitled");
    let table_preset = preset("table-m-1200x900").expect("INVARIANT: the default table preset exists");
    let id = PlatformId(doc.allocate_id());
    let mut table = platform_from_preset(&doc, table_preset, id, None);
    table.position = DVec2::ZERO;
    doc.platforms.push(table);
    let door = OpeningId(doc.allocate_id());
    doc.room.openings.push(Opening {
        id: door,
        name: "Door 1".into(),
        wall: 0,
        offset: 600.0,
        width: 1000.0,
        height: 2100.0,
        sill: 0.0,
    });
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holes::{ExclusionZone, ZoneShape, compute_holes};

    #[test]
    fn pr1_every_preset_builds_a_valid_platform() {
        let doc = default_document();
        let table = doc.platforms[0].id;
        let mut ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len(), "preset ids are unique");
        for preset in PRESETS {
            assert!(preset.size.0 >= preset.size.1, "{}: W ≥ D", preset.id);
            for on in [None, Some(table)] {
                let p = platform_from_preset(&doc, preset, PlatformId(doc.next_free_id()), on);
                assert_eq!(p.validate(), Ok(()), "{} on {on:?}", preset.id);
                let holes = compute_holes(&p).unwrap();
                assert!(!holes.is_empty(), "{}", preset.id);
                assert_eq!(p.hole_grid.as_ref().unwrap().standard(), preset.grid);
            }
            assert_eq!(super::preset(preset.id).map(|p| p.id), Some(preset.id));
        }
        assert!(super::preset("nope").is_none());
    }

    #[test]
    fn pr1_breadboard_hole_counts() {
        let doc = Document::new("Test");
        let count = |id: &str| {
            let p = platform_from_preset(&doc, preset(id).unwrap(), PlatformId(1), None);
            let holes = compute_holes(&p).unwrap();
            (holes.cols, holes.rows, holes.len())
        };
        assert_eq!(preset("bb-m-600x600").unwrap().hint, "Thorlabs MB6060/M");
        assert_eq!(count("bb-m-600x600"), (24, 24, 576));
        assert_eq!(preset("bb-i-12x12").unwrap().hint, "Thorlabs MB12");
        assert_eq!(count("bb-i-12x12"), (12, 12, 144));
    }

    #[test]
    fn pr1_breadboard_placement() {
        let doc = default_document();
        let table = doc.platforms[0].clone();
        let bb = preset("bb-m-600x600").unwrap();
        let floor = platform_from_preset(&doc, bb, PlatformId(10), None);
        assert_eq!(floor.top_height, 12.7);
        assert_eq!(floor.support, Support::Fixed);
        assert_eq!(floor.kind, PlatformKind::Breadboard);
        assert_eq!(floor.surface, Surface::BlackAnodized);
        assert_eq!(floor.name, "Breadboard 1");
        // The room centre is taken by Table A: the new board goes 100 mm to its right.
        assert_eq!(floor.position, DVec2::new(600.0 + 100.0 + 300.0, 0.0));
        let on_table = platform_from_preset(&doc, bb, PlatformId(10), Some(table.id));
        assert_eq!(on_table.top_height, table.top_height + 12.7);
        assert_eq!(on_table.position, table.position);
        assert_eq!(on_table.rotation_z, table.rotation_z);
        // A table is never stacked.
        let t = platform_from_preset(&doc, preset("table-m-2400x1200").unwrap(), PlatformId(11), Some(table.id));
        assert_eq!(t.top_height, 800.0);
        assert_eq!(t.name, "Table B");
        assert_eq!(t.support, Support::Legs { size: 120.0, inset: 150.0 });
        assert_eq!(t.position, DVec2::new(600.0 + 100.0 + 1200.0, 0.0));
        // An empty room: the room centre.
        let empty = Document::new("Empty");
        assert_eq!(platform_from_preset(&empty, bb, PlatformId(1), None).position, DVec2::ZERO);
        // Custom: a 900 × 600 polygon on the floor.
        let custom = platform_from_preset(&empty, preset("custom").unwrap(), PlatformId(1), None);
        assert_eq!(custom.outline.kind_index(), 3);
        assert_eq!(custom.outline.bounds(), (DVec2::new(-450.0, -300.0), DVec2::new(450.0, 300.0)));
        assert_eq!(custom.name, "Platform 1");
        assert_eq!(custom.top_height, 60.0);
    }

    #[test]
    fn naming() {
        let mut doc = Document::new("Names");
        assert_eq!(next_platform_name(&doc, PlatformKind::OpticalTable), "Table A");
        let preset_table = preset("table-m-1200x900").unwrap();
        for k in 0..27u64 {
            let p = platform_from_preset(&doc, preset_table, PlatformId(k + 1), None);
            doc.platforms.push(p);
        }
        assert_eq!(doc.platforms[25].name, "Table Z");
        assert_eq!(doc.platforms[26].name, "Table AA");
        assert_eq!(next_platform_name(&doc, PlatformKind::OpticalTable), "Table AB");
        doc.platforms.remove(1);
        assert_eq!(next_platform_name(&doc, PlatformKind::OpticalTable), "Table B");
        assert_eq!(next_platform_name(&doc, PlatformKind::Breadboard), "Breadboard 1");
        assert_eq!(next_platform_name(&doc, PlatformKind::Custom), "Platform 1");
        assert_eq!(letters(701), "ZZ");
        assert_eq!(letters(702), "AAA");
    }

    #[test]
    fn apply_preset_keeps_identity_and_zones() {
        let doc = default_document();
        let mut p = doc.platforms[0].clone();
        p.position = DVec2::new(10.0, 20.0);
        p.rotation_z = 0.3;
        p.top_height = 900.0;
        let zone =
            ExclusionZone { name: "Zone 1".into(), shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 40.0 } };
        p.hole_grid.as_mut().unwrap().exclusions.push(zone.clone());
        let applied = apply_preset(&p, preset("table-i-60x48").unwrap());
        assert_eq!(
            (applied.id, &applied.name, applied.position, applied.rotation_z, applied.top_height),
            (p.id, &p.name, p.position, p.rotation_z, 900.0)
        );
        assert_eq!(applied.outline, Outline::Rectangle { size: DVec2::new(60.0 * INCH, 48.0 * INCH) });
        let grid = applied.hole_grid.as_ref().unwrap();
        assert_eq!(grid.pitch, DVec2::splat(INCH));
        assert_eq!(grid.exclusions, vec![zone]);
        assert_eq!(applied.validate(), Ok(()));
        let bb = apply_preset(&Platform { hole_grid: None, ..p }, preset("bb-m-600x600").unwrap());
        assert_eq!(bb.support, Support::Fixed);
        assert!(bb.hole_grid.as_ref().unwrap().exclusions.is_empty());
    }

    #[test]
    fn default_document_contents() {
        let doc = default_document();
        assert_eq!(doc.validate(), Ok(()));
        assert_eq!(doc.name, "Untitled");
        assert_eq!(doc.platforms.len(), 1);
        let table = &doc.platforms[0];
        assert_eq!(table.name, "Table A");
        assert_eq!(table.position, DVec2::ZERO);
        assert_eq!(table.top_height, 800.0);
        assert_eq!(table.outline, Outline::Rectangle { size: DVec2::new(1200.0, 900.0) });
        assert_eq!(doc.room.outline, crate::room::Room::default().outline);
        let door = &doc.room.openings[0];
        assert_eq!(
            (door.name.as_str(), door.wall, door.offset, door.width, door.height, door.sill),
            ("Door 1", 0, 600.0, 1000.0, 2100.0, 0.0)
        );
        assert!(doc.nodes.is_empty());
        assert!(doc.next_free_id() > doc.max_id());
    }
}
