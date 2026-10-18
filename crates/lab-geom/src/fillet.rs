//! Corner fillets flattened into a closed polyline.

use glam::DVec2;

use crate::polygon::{PolygonError, PolygonLimits};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilletCorner {
    pub point: DVec2,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlatPath {
    /// Flattened closed loop, same orientation as the input.
    pub points: Vec<DVec2>,
    /// `true` = arc-interior vertex (side walls share its normal).
    pub smooth: Vec<bool>,
    /// Input corner that produced each point.
    pub source_corner: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FilletError {
    #[error("corner {} radius cannot be negative", .corner + 1)]
    NegativeRadius { corner: usize },
    #[error("corner {} radius is too large; the maximum here is {max_radius:.1} mm", .corner + 1)]
    TooLarge { corner: usize, max_radius: f64 },
    #[error("{0}")]
    Polygon(PolygonError),
}

pub fn flatten_fillets(
    corners: &[FilletCorner],
    tolerance: f64,
    max_points: usize,
    limits: &PolygonLimits,
) -> Result<FlatPath, FilletError> {
    let _ = (corners, tolerance, max_points, limits);
    unimplemented!()
}
