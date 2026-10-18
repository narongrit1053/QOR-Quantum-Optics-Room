//! Mitred room walls with real openings.

use glam::DVec2;

use crate::mesh::TriMesh;
use crate::polygon::PolygonError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpeningSpec {
    pub wall: usize,
    pub offset: f64,
    pub width: f64,
    pub sill: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
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

/// Usable inner-face interval `[min_offset, max_end]` on `wall`, already including `min_jamb`.
pub fn opening_limits(inner: &[DVec2], thickness: f64, wall: usize, min_jamb: f64) -> (f64, f64) {
    let _ = (inner, thickness, wall, min_jamb);
    unimplemented!()
}

pub fn build_walls(
    inner: &[DVec2],
    thickness: f64,
    height: f64,
    openings: &[OpeningSpec],
    min_jamb: f64,
    footprint_z: f64,
) -> Result<Vec<WallGeometry>, WallError> {
    let _ = (inner, thickness, height, openings, min_jamb, footprint_z);
    unimplemented!()
}
