//! Conversion of `lab_geom` triangle meshes (document space: mm, +Z up) into Bevy meshes
//! (render space: metres, +Y up).

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::scene::{normal_to_render, to_render};

/// `Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())`, i.e.
/// MAIN_WORLD | RENDER_WORLD, which CPU ray casting (picking) needs.
/// POSITION = to_render(p), NORMAL = normal_to_render(n), UV_0 = uv as f32 (unconverted mm for
/// platform tops), Indices::U32. The axis change has det = +1, so winding is preserved.
pub(crate) fn trimesh_to_bevy(m: &lab_geom::TriMesh) -> Mesh {
    let positions: Vec<[f32; 3]> = m.positions.iter().map(|&p| to_render(p).to_array()).collect();
    let normals: Vec<[f32; 3]> = m.normals.iter().map(|&n| normal_to_render(n).to_array()).collect();
    let uvs: Vec<[f32; 2]> = m.uvs.iter().map(|uv| [uv.x as f32, uv.y as f32]).collect();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(m.indices.clone()))
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues;
    use lab_core::glam::{DVec2, DVec3};

    use super::*;

    fn attribute(mesh: &Mesh, id: bevy::mesh::MeshVertexAttribute) -> Vec<[f32; 3]> {
        match mesh.attribute(id) {
            Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
            other => panic!("unexpected attribute {other:?}"),
        }
    }

    fn square() -> Vec<DVec2> {
        vec![DVec2::new(-300.0, -200.0), DVec2::new(300.0, -200.0), DVec2::new(300.0, 200.0), DVec2::new(-300.0, 200.0)]
    }

    #[test]
    fn conversion_keeps_vertices_and_maps_axes() {
        let top = lab_geom::flat_polygon(&square(), 0.0, true, 1.0).unwrap();
        let mesh = trimesh_to_bevy(&top);
        assert_eq!(mesh.count_vertices(), top.positions.len());
        assert_eq!(mesh.indices().map(|i| i.len()), Some(top.indices.len()));
        // A +Z normal maps to +Y.
        for n in attribute(&mesh, Mesh::ATTRIBUTE_NORMAL) {
            assert_eq!(Vec3::from(n), Vec3::Y);
        }
        assert_eq!(normal_to_render(DVec3::Z), Vec3::Y);
        // Positions are converted; UVs stay in platform-local mm.
        let positions = attribute(&mesh, Mesh::ATTRIBUTE_POSITION);
        for (p, q) in positions.iter().zip(&top.positions) {
            assert!((Vec3::from(*p) - to_render(*q)).length() < 1e-7);
        }
        match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(VertexAttributeValues::Float32x2(uvs)) => {
                for (uv, q) in uvs.iter().zip(&top.positions) {
                    assert_eq!(*uv, [q.x as f32, q.y as f32]);
                }
            }
            other => panic!("unexpected uvs {other:?}"),
        }
    }

    /// Every triangle's geometric normal (CCW = front in Bevy) agrees with its vertex normals,
    /// for the top (seen from above) and for the whole extruded body.
    #[test]
    fn winding_is_preserved() {
        let path = lab_geom::FlatPath {
            points: vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(800.0, 0.0),
                DVec2::new(800.0, 300.0),
                DVec2::new(300.0, 300.0),
                DVec2::new(300.0, 600.0),
                DVec2::new(0.0, 600.0),
            ],
            smooth: vec![false; 6],
            source_corner: (0..6).collect(),
        };
        let meshes = lab_geom::extrude(&path, -60.0, 0.0).unwrap();
        for (trimesh, is_top) in [(&meshes.top, true), (&meshes.body, false)] {
            let mesh = trimesh_to_bevy(trimesh);
            let positions = attribute(&mesh, Mesh::ATTRIBUTE_POSITION);
            let normals = attribute(&mesh, Mesh::ATTRIBUTE_NORMAL);
            let indices: Vec<usize> = mesh.indices().unwrap().iter().collect();
            assert!(!indices.is_empty());
            for tri in indices.chunks(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(positions[i]));
                let face = (b - a).cross(c - a);
                let normal = Vec3::from(normals[tri[0]]) + Vec3::from(normals[tri[1]]) + Vec3::from(normals[tri[2]]);
                assert!(face.dot(normal) > 0.0, "triangle {tri:?} faces away from its normals");
                if is_top {
                    // Seen from above (+Y), the top triangles are front-facing.
                    assert!(face.y > 0.0);
                }
            }
        }
    }
}
