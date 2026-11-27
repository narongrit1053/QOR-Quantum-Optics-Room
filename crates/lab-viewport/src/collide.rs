//! Collision detection between placed part nodes (broad-phase OBB + narrow-phase mesh SAT).
//! Runs only when the scene or catalog changes, or when async GLB meshes finish loading.

use bevy::{
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};
use lab_core::{Document, NodeId, Parent, Shape};

use crate::glb::GlbLoading;
use crate::host::{EventSender, PendingRequests, SettleFrames};
use crate::scene::{MM, SceneIndex, SceneNode};
use crate::{CatalogRes, FrameRequest, ViewportEvent};

/// Half-millimetre penetration tolerance (render metres) so parts that merely touch do not collide.
const COLLIDE_TOL: f32 = 0.5 * MM;
/// Max filtered triangle-pair tests per piece pair before falling back to the OBB result.
const TRI_PAIR_BUDGET: usize = 200_000;

/// Pairs of colliding part node ids, sorted and deduped with `a < b`.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Collisions(pub Vec<(NodeId, NodeId)>);

pub(crate) struct CollidePlugin;

impl Plugin for CollidePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Collisions>().add_systems(
            PostUpdate,
            detect_collisions
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate)
                .before(crate::outline::rebuild_outline),
        );
    }
}

/// Oriented bounding box in render space (metres, Y up).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Obb {
    pub center: Vec3,
    pub axes: [Vec3; 3],
    pub half: Vec3,
}

fn obb_from_aabb(aabb: &Aabb, global: &GlobalTransform) -> (Obb, Vec3, Vec3) {
    let affine = global.affine();
    let center = global.transform_point(Vec3::from(aabb.center));
    let m = affine.matrix3;
    let raw = [m.x_axis, m.y_axis, m.z_axis];
    let mut axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let mut lens = [0.0f32; 3];
    for i in 0..3 {
        let len = raw[i].length();
        lens[i] = len;
        if len > 1e-8 {
            axes[i] = (raw[i] / len).into();
        }
    }
    // Shrink by 0.5 mm in render units so parts that merely touch do not collide.
    let half = (Vec3::from(aabb.half_extents) * Vec3::from_array(lens) - Vec3::splat(COLLIDE_TOL)).max(Vec3::ZERO);
    let world_half = Vec3::new(
        half.x * axes[0].x.abs() + half.y * axes[1].x.abs() + half.z * axes[2].x.abs(),
        half.x * axes[0].y.abs() + half.y * axes[1].y.abs() + half.z * axes[2].y.abs(),
        half.x * axes[0].z.abs() + half.y * axes[1].z.abs() + half.z * axes[2].z.abs(),
    );
    (Obb { center, axes, half }, center - world_half, center + world_half)
}

fn aabb_overlap(min_a: Vec3, max_a: Vec3, min_b: Vec3, max_b: Vec3) -> bool {
    !(max_a.x < min_b.x
        || max_b.x < min_a.x
        || max_a.y < min_b.y
        || max_b.y < min_a.y
        || max_a.z < min_b.z
        || max_b.z < min_a.z)
}

/// 15-axis separating-axis test between two oriented bounding boxes.
pub(crate) fn obb_overlap(a: &Obb, b: &Obb) -> bool {
    let t = b.center - a.center;
    let separated = |axis: Vec3| -> bool {
        if axis.length_squared() < 1e-10 {
            return false;
        }
        let dist = t.dot(axis).abs();
        let ra = a.half.x * a.axes[0].dot(axis).abs()
            + a.half.y * a.axes[1].dot(axis).abs()
            + a.half.z * a.axes[2].dot(axis).abs();
        let rb = b.half.x * b.axes[0].dot(axis).abs()
            + b.half.y * b.axes[1].dot(axis).abs()
            + b.half.z * b.axes[2].dot(axis).abs();
        dist >= ra + rb
    };
    for i in 0..3 {
        if separated(a.axes[i]) || separated(b.axes[i]) {
            return false;
        }
    }
    for i in 0..3 {
        for j in 0..3 {
            if separated(a.axes[i].cross(b.axes[j])) {
                return false;
            }
        }
    }
    true
}

/// Separating-axis test between two triangles in render space.
/// Requires `> 0.5 * MM` penetration along every axis so touching/skimming triangles do not collide.
pub(crate) fn tri_tri_intersect(a: [Vec3; 3], b: [Vec3; 3]) -> bool {
    let ua = [a[1] - a[0], a[2] - a[1], a[0] - a[2]].map(Vec3::normalize_or_zero);
    let ub = [b[1] - b[0], b[2] - b[1], b[0] - b[2]].map(Vec3::normalize_or_zero);
    let na = ua[0].cross(ua[1]);
    let nb = ub[0].cross(ub[1]);
    if na.length_squared() < 1e-10 || nb.length_squared() < 1e-10 {
        return false;
    }

    let separated = |axis: Vec3| -> bool {
        let len_sq = axis.length_squared();
        if len_sq < 1e-10 {
            return false;
        }
        let u = axis / len_sq.sqrt();
        let (mut min_a, mut max_a) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in a {
            let p = v.dot(u);
            min_a = min_a.min(p);
            max_a = max_a.max(p);
        }
        let (mut min_b, mut max_b) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in b {
            let p = v.dot(u);
            min_b = min_b.min(p);
            max_b = max_b.max(p);
        }
        (min_b - max_a).max(min_a - max_b) >= -COLLIDE_TOL
    };

    if separated(na) || separated(nb) {
        return false;
    }
    for i in 0..3 {
        for j in 0..3 {
            if separated(ua[i].cross(ub[j])) {
                return false;
            }
        }
    }
    let una = na.normalize();
    let unb = nb.normalize();
    for i in 0..3 {
        if separated(una.cross(ua[i])) || separated(unb.cross(ub[i])) {
            return false;
        }
    }
    true
}

type WorldTri = ([Vec3; 3], Vec3, Vec3);

fn piece_triangles(mesh: &Mesh, global: &GlobalTransform) -> Option<Vec<WorldTri>> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let Some(VertexAttributeValues::Float32x3(raw)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return None;
    };
    let verts: Vec<Vec3> = raw.iter().map(|&p| global.transform_point(Vec3::from(p))).collect();
    let push_tri = |out: &mut Vec<WorldTri>, i0: usize, i1: usize, i2: usize| {
        if let (Some(&v0), Some(&v1), Some(&v2)) = (verts.get(i0), verts.get(i1), verts.get(i2)) {
            out.push(([v0, v1, v2], v0.min(v1).min(v2), v0.max(v1).max(v2)));
        }
    };
    let mut tris = Vec::new();
    match mesh.indices() {
        Some(Indices::U16(idx)) => {
            tris.reserve(idx.len() / 3);
            for c in idx.chunks_exact(3) {
                push_tri(&mut tris, c[0] as usize, c[1] as usize, c[2] as usize);
            }
        }
        Some(Indices::U32(idx)) => {
            tris.reserve(idx.len() / 3);
            for c in idx.chunks_exact(3) {
                push_tri(&mut tris, c[0] as usize, c[1] as usize, c[2] as usize);
            }
        }
        None => {
            tris.reserve(verts.len() / 3);
            for k in 0..(verts.len() / 3) {
                let i = k * 3;
                push_tri(&mut tris, i, i + 1, i + 2);
            }
        }
    }
    Some(tris)
}

fn meshes_collide(
    tris_a: &[WorldTri],
    wmin_a: Vec3,
    wmax_a: Vec3,
    tris_b: &[WorldTri],
    wmin_b: Vec3,
    wmax_b: Vec3,
) -> bool {
    let cand_a: Vec<_> = tris_a.iter().filter(|(_, tmin, tmax)| aabb_overlap(*tmin, *tmax, wmin_b, wmax_b)).collect();
    let cand_b: Vec<_> = tris_b.iter().filter(|(_, tmin, tmax)| aabb_overlap(*tmin, *tmax, wmin_a, wmax_a)).collect();
    // shortcut: cap narrow-phase triangle pair tests at 200_000 per piece pair and fall back to OBB;
    // add a per-mesh BVH/grid when high-poly CAD imports hit this.
    if cand_a.len().saturating_mul(cand_b.len()) > TRI_PAIR_BUDGET {
        return true;
    }
    for &(ta, min_ta, max_ta) in &cand_a {
        for &(tb, min_tb, max_tb) in &cand_b {
            if aabb_overlap(*min_ta, *max_ta, *min_tb, *max_tb) && tri_tri_intersect(*ta, *tb) {
                return true;
            }
        }
    }
    false
}

fn is_ancestor(doc: &Document, ancestor: NodeId, mut id: NodeId) -> bool {
    for _ in 0..doc.nodes.len() {
        let Some(node) = doc.node(id) else { return false };
        match node.parent {
            Parent::Node(p) if p == ancestor => return true,
            Parent::Node(p) => id = p,
            Parent::Platform(_) | Parent::Room => return false,
        }
    }
    false
}

struct PieceBox<'a> {
    id: NodeId,
    obb: Obb,
    wmin: Vec3,
    wmax: Vec3,
    mesh: &'a Mesh3d,
    global: GlobalTransform,
}

#[allow(clippy::too_many_arguments)]
fn detect_collisions(
    pending: Res<PendingRequests>,
    index: Res<SceneIndex>,
    catalog: Res<CatalogRes>,
    loading: Res<GlbLoading>,
    added: Query<(), (With<SceneNode>, Or<(Added<SceneNode>, Added<Mesh3d>, Changed<Mesh3d>, Added<Aabb>)>)>,
    pieces: Query<(&SceneNode, &Mesh3d, &Aabb, &GlobalTransform)>,
    meshes: Res<Assets<Mesh>>,
    mut collisions: ResMut<Collisions>,
    events: Res<EventSender>,
    mut settle: ResMut<SettleFrames>,
    mut dirty: Local<bool>,
) {
    if catalog.is_changed() || !added.is_empty() {
        *dirty = true;
    }
    for request in &pending.0 {
        if matches!(request, FrameRequest::LoadDocument(_) | FrameRequest::ApplyChanges(..)) {
            *dirty = true;
        }
    }
    if !*dirty || loading.0 {
        return;
    }
    *dirty = false;

    let mut pairs = Vec::new();
    if let Some(doc) = index.snapshot.as_ref() {
        let boxes: Vec<PieceBox<'_>> = pieces
            .iter()
            .filter_map(|(scene_node, mesh, aabb, global)| {
                let node = doc.node(scene_node.0)?;
                if !matches!(node.shape, Shape::Part) {
                    return None;
                }
                let (obb, wmin, wmax) = obb_from_aabb(aabb, global);
                Some(PieceBox { id: scene_node.0, obb, wmin, wmax, mesh, global: *global })
            })
            .collect();
        let mut piece_tris: Vec<Option<Option<Vec<WorldTri>>>> = vec![None; boxes.len()];

        for i in 0..boxes.len() {
            for j in (i + 1)..boxes.len() {
                let (a, b) = (&boxes[i], &boxes[j]);
                if a.id == b.id || is_ancestor(doc, a.id, b.id) || is_ancestor(doc, b.id, a.id) {
                    continue;
                }
                let pair = (a.id.min(b.id), a.id.max(b.id));
                if pairs.contains(&pair) {
                    continue;
                }
                if !aabb_overlap(a.wmin, a.wmax, b.wmin, b.wmax) || !obb_overlap(&a.obb, &b.obb) {
                    continue;
                }
                for idx in [i, j] {
                    if piece_tris[idx].is_none() {
                        piece_tris[idx] = Some(meshes.get(&boxes[idx].mesh.0).and_then(|m| piece_triangles(m, &boxes[idx].global)));
                    }
                }
                let (Some(ta), Some(tb)) = (
                    piece_tris[i].as_ref().and_then(|o| o.as_deref()),
                    piece_tris[j].as_ref().and_then(|o| o.as_deref()),
                ) else {
                    continue;
                };
                if meshes_collide(ta, a.wmin, a.wmax, tb, b.wmin, b.wmax) {
                    pairs.push(pair);
                }
            }
        }
        pairs.sort();
        pairs.dedup();
    }

    if collisions.0 != pairs {
        collisions.0 = pairs.clone();
        events.send(ViewportEvent::Collisions(pairs));
        settle.0 = settle.0.max(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    fn obb(center: Vec3, yaw: f32, half: Vec3) -> Obb {
        let q = Quat::from_rotation_y(yaw);
        Obb { center, axes: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z], half }
    }

    #[test]
    fn obb_overlap_detects_overlapping_boxes() {
        let a = obb(Vec3::ZERO, 0.0, Vec3::splat(0.02));
        let b = obb(Vec3::new(0.025, 0.0, 0.01), 0.4, Vec3::splat(0.02));
        assert!(obb_overlap(&a, &b));
    }

    #[test]
    fn obb_overlap_separates_rotated_boxes() {
        // Two 45°-rotated thin bars whose world AABBs overlap, yet the OBBs are clearly separated.
        let a = obb(Vec3::ZERO, FRAC_PI_4, Vec3::new(0.10, 0.01, 0.01));
        let b = obb(Vec3::new(0.03, 0.0, 0.03), FRAC_PI_4, Vec3::new(0.10, 0.01, 0.01));
        assert!(!obb_overlap(&a, &b));
    }

    #[test]
    fn tri_tri_intersect_crossing_triangles() {
        let a = [Vec3::new(-0.01, 0.0, -0.01), Vec3::new(0.01, 0.0, -0.01), Vec3::new(0.0, 0.0, 0.01)];
        let b = [Vec3::new(0.0, -0.01, -0.005), Vec3::new(0.0, 0.01, -0.005), Vec3::new(0.0, 0.0, 0.005)];
        assert!(tri_tri_intersect(a, b));
    }

    #[test]
    fn tri_tri_intersect_parallel_separated() {
        let a = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.02, 0.0, 0.0), Vec3::new(0.0, 0.02, 0.0)];
        let b = [Vec3::new(0.0, 0.0, 0.005), Vec3::new(0.02, 0.0, 0.005), Vec3::new(0.0, 0.02, 0.005)];
        assert!(!tri_tri_intersect(a, b));
    }

    #[test]
    fn tri_tri_intersect_touching_or_within_tenth_mm_is_false() {
        let a = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.02, 0.0, 0.0), Vec3::new(0.0, 0.02, 0.0)];
        // Touching along the X edge.
        let touch = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.02, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.02)];
        assert!(!tri_tri_intersect(a, touch));
        // Overlapping by 0.1 mm across the edge (below the 0.5 mm penetration threshold).
        let skim = [
            Vec3::new(0.0, 0.1 * MM, -0.01),
            Vec3::new(0.02, 0.1 * MM, -0.01),
            Vec3::new(0.0, -0.02, 0.01),
        ];
        assert!(!tri_tri_intersect(a, skim));
    }

    #[test]
    fn meshes_collide_rejects_non_intersecting_meshes_with_overlapping_aabbs() {
        // Two L-shaped / disjoint meshes (u16 and unindexed) whose bounding boxes overlap,
        // like two neighbouring post holders whose thumbscrews overlap in OBB space.
        let m_a = Mesh::new(PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[0.0, 0.0, 0.0], [0.02, 0.0, 0.0], [0.0, 0.02, 0.0]],
            )
            .with_inserted_indices(Indices::U16(vec![0, 1, 2]));
        let m_b = Mesh::new(PrimitiveTopology::TriangleList, default()).with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.015, 0.015, 0.0], [0.03, 0.015, 0.0], [0.015, 0.03, 0.0]],
        );
        let ta = piece_triangles(&m_a, &GlobalTransform::IDENTITY).unwrap();
        let tb = piece_triangles(&m_b, &GlobalTransform::IDENTITY).unwrap();
        // Broad-phase AABBs overlap ([0..0.02] vs [0.015..0.03]), yet triangles are disjoint.
        assert!(!meshes_collide(&ta, Vec3::ZERO, Vec3::splat(0.02), &tb, Vec3::splat(0.015), Vec3::splat(0.03)));

        // Shifting mesh B onto mesh A across the Z plane produces a real collision.
        let m_c = Mesh::new(PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[0.004, 0.004, -0.005], [0.004, 0.012, -0.005], [0.004, 0.004, 0.005]],
            )
            .with_inserted_indices(Indices::U32(vec![0, 1, 2]));
        let tc = piece_triangles(&m_c, &GlobalTransform::IDENTITY).unwrap();
        assert!(meshes_collide(
            &ta,
            Vec3::ZERO,
            Vec3::splat(0.02),
            &tc,
            Vec3::new(0.0, 0.0, -0.005),
            Vec3::new(0.02, 0.02, 0.005),
        ));
    }
}
