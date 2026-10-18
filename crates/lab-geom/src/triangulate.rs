//! Ear-clipping triangulation of simple polygons.

use glam::DVec2;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TriangulateError {
    #[error("degenerate polygon")]
    Degenerate,
    #[error("no ear found ({remaining} corners left)")]
    NoEarFound { remaining: usize },
}

/// Triangles CCW seen from +Z, as indices into `points`.
pub fn triangulate(points: &[DVec2]) -> Result<Vec<[u32; 3]>, TriangulateError> {
    let _ = points;
    unimplemented!()
}
