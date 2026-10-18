//! Hard limits on document values (M1-SPEC §1.1). Validation enforces them; the UI uses them for
//! hints. Lengths are millimetres.

use std::ops::RangeInclusive;

/// Largest absolute value of any coordinate or position.
pub const MAX_COORD: f64 = 100_000.0;

/// Outline bounding box extent, per axis.
pub const PLATFORM_MIN_EXTENT: f64 = 50.0;
pub const PLATFORM_MAX_EXTENT: f64 = 20_000.0;
/// Smallest platform outline area, in mm².
pub const PLATFORM_MIN_AREA: f64 = 2_500.0;
pub const OUTLINE_MAX_CORNERS: usize = 128;
pub const OUTLINE_MIN_EDGE: f64 = 1.0;
/// Largest arc-chord sagitta when fillets are flattened.
pub const FLATTEN_TOLERANCE: f64 = 0.02;
pub const ARC_MAX_SEGMENTS: usize = 64;
pub const FLATTEN_MAX_POINTS: usize = 2048;

pub const THICKNESS: RangeInclusive<f64> = 3.0..=1000.0;
/// Highest top surface; the top height must also be at least the thickness.
pub const TOP_HEIGHT_MAX: f64 = 5000.0;

/// Hole pitch per axis; the pitch must also be at least the thread diameter + [`PITCH_CLEARANCE`].
pub const PITCH: RangeInclusive<f64> = 3.0..=500.0;
pub const PITCH_CLEARANCE: f64 = 1.0;
/// Largest edge margin; the margin must also be at least the thread radius.
pub const MARGIN_MAX: f64 = 1000.0;
/// Candidate columns or rows per axis (2048 texture limit including the one-texel border).
pub const MAX_LATTICE_AXIS: u64 = 2046;
pub const MAX_LATTICE_POINTS: u64 = 250_000;
/// Exclusion zones per platform (shader uniform array size).
pub const MAX_ZONES: usize = 16;

/// Leg size; the inset must also be at least half the size.
pub const LEG_SIZE: RangeInclusive<f64> = 10.0..=500.0;
pub const LEG_MAX_SPAN: f64 = 2200.0;
pub const MAX_LEGS: usize = 12;

pub const ROOM_CORNERS: RangeInclusive<usize> = 3..=64;
/// Shortest wall, measured on the inner face.
pub const ROOM_MIN_WALL: f64 = 100.0;
/// Interior angle at a room corner, in degrees.
pub const ROOM_ANGLE_DEG: RangeInclusive<f64> = 20.0..=340.0;
pub const WALL_HEIGHT: RangeInclusive<f64> = 500.0..=10_000.0;
pub const WALL_THICKNESS: RangeInclusive<f64> = 20.0..=1000.0;

pub const OPENING_MIN_WIDTH: f64 = 300.0;
pub const OPENING_MIN_HEIGHT: f64 = 300.0;
/// Wall left between an opening and a corner or the next opening.
pub const OPENING_MIN_JAMB: f64 = 50.0;
pub const MAX_OPENINGS: usize = 32;

/// Names are trimmed and must have 1..=NAME_MAX_CHARS characters.
pub const NAME_MAX_CHARS: usize = 128;
/// Ids cross into Slint `int`.
pub const MAX_ID: u64 = i32::MAX as u64;
pub const HISTORY_LIMIT: usize = 500;
/// Comparisons favour keeping a hole that sits exactly at a limit.
pub const EPS_KEEP: f64 = 1e-6;
