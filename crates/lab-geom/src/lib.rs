//! Pure geometry for LabSetup (f64): polygon validation, fillets, triangulation, distances,
//! extrusion and mitred walls. No GPU, UI or I/O dependencies.
//!
//! Every function accepts either polygon orientation unless its documentation says otherwise.

pub mod distance;
pub mod fillet;
pub mod mesh;
pub mod polygon;
pub mod triangulate;
pub mod walls;

pub use distance::{
    distance_to_boundary, distance_to_region, distance_to_segment, horizontal_crossings,
    point_in_polygon,
};
pub use fillet::{FilletCorner, FilletError, FlatPath, flatten_fillets};
pub use mesh::{PlatformMeshes, TriMesh, circle_points, extrude, flat_polygon};
pub use polygon::{
    PolygonError, PolygonLimits, bounds, interior_angle, orient, outward_normal,
    segments_intersect, signed_area, validate_simple_polygon,
};
pub use triangulate::{TriangulateError, triangulate};
pub use walls::{OpeningSpec, WallError, WallGeometry, build_walls, opening_limits};
