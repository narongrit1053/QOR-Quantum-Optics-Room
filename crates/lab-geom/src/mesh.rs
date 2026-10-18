//! Triangle meshes: extruded platforms, flat polygons and circles.

use glam::{DVec2, DVec3};

use crate::fillet::FlatPath;
use crate::triangulate::TriangulateError;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<DVec2>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn append(&mut self, other: &TriMesh) {
        let _ = other;
        unimplemented!()
    }

    pub fn triangle_count(&self) -> usize {
        unimplemented!()
    }

    /// Test helper: every undirected edge (positions quantised to 1e-6 mm) is used by exactly
    /// two triangles in opposite directions.
    pub fn is_closed(&self) -> bool {
        unimplemented!()
    }
}

pub struct PlatformMeshes {
    pub top: TriMesh,
    pub body: TriMesh,
}

/// top: face at z_top, normal +Z, uv = local (x, y) in mm. body: side walls + bottom face at
/// z_bottom (normal −Z). Side normals: per-edge outward; averaged at `smooth` vertices; sharp
/// vertices are duplicated per edge.
pub fn extrude(
    path: &FlatPath,
    z_bottom: f64,
    z_top: f64,
) -> Result<PlatformMeshes, TriangulateError> {
    let _ = (path, z_bottom, z_top);
    unimplemented!()
}

/// Flat polygon at z; up = true gives normal +Z with CCW winding from above. uv = (x, y) * uv_scale.
pub fn flat_polygon(
    points: &[DVec2],
    z: f64,
    up: bool,
    uv_scale: f64,
) -> Result<TriMesh, TriangulateError> {
    let _ = (points, z, up, uv_scale);
    unimplemented!()
}

/// CCW circle approximation.
pub fn circle_points(
    center: DVec2,
    radius: f64,
    tolerance: f64,
    max_segments: usize,
) -> Vec<DVec2> {
    let _ = (center, radius, tolerance, max_segments);
    unimplemented!()
}
