//! Selection outline by inverted hull (M3-PLAN POLA-50): every mesh piece of the selected node and
//! of the nodes below it gets a slightly larger, front-culled, unlit accent copy as a child.
//! M9 D: the hovered node (and `HighlightNodes`) get a thinner, fainter one.

use bevy::{
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    light::NotShadowCaster,
    prelude::*,
    render::render_resource::Face,
};
use lab_core::SelectionTarget;

use crate::camera::{OrbitCamera, TopPane};
use crate::picking::Selection;
use crate::scene::{SceneIndex, SceneNode};

/// Outline width in physical pixels.
const OUTLINE_PX: f32 = 2.0;
/// Distinct from the accent tint of the selected part, so the outline reads over it (POLA-50).
const OUTLINE: Color = Color::srgb(1.0, 0.62, 0.1);
/// M9 D. Hover outline: thinner and fainter than the selection's.
const HOVER_PX: f32 = 1.0;
const HOVER: Color = Color::srgb(0.55, 0.7, 0.9);

/// Width in physical px.
#[derive(Component)]
pub(crate) struct Outline(f32);

/// M9 G. Nodes outlined like hover (`ViewportRequest::HighlightNodes`).
#[derive(Resource, Default, PartialEq)]
struct Highlighted(Vec<lab_core::NodeId>);

pub(crate) struct OutlinePlugin;

impl Plugin for OutlinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Highlighted>().add_systems(
            PostUpdate,
            (rebuild_outline, resize_outline)
                .chain()
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate),
        );
    }
}

type ChangedMainCamera = (With<OrbitCamera>, Without<TopPane>, Changed<GlobalTransform>);
type CameraView<'a> = (&'a Camera, &'a GlobalTransform, &'a Projection);

/// Respawns the outline when the selection changed or pieces were (re)spawned; despawns it on
/// deselect. Platforms keep their tint and get no outline.
#[allow(clippy::too_many_arguments)]
fn rebuild_outline(
    mut commands: Commands,
    selection: Res<Selection>,
    capturing: Res<crate::export::Capturing>,
    index: Res<SceneIndex>,
    pending: Res<crate::host::PendingRequests>,
    mut highlighted: ResMut<Highlighted>,
    added: Query<(), (With<SceneNode>, Added<Aabb>)>,
    pieces: Query<(Entity, &SceneNode, &Mesh3d, &Aabb, &GlobalTransform)>,
    outlines: Query<Entity, With<Outline>>,
    camera: Query<CameraView, (With<OrbitCamera>, Without<TopPane>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<(Handle<StandardMaterial>, Handle<StandardMaterial>)>>,
) {
    for request in &pending.0 {
        if let crate::FrameRequest::HighlightNodes(ids) = request {
            highlighted.set_if_neq(Highlighted(ids.clone()));
        }
    }
    if !selection.is_changed() && !capturing.is_changed() && !highlighted.is_changed() && added.is_empty() {
        return;
    }
    for e in &outlines {
        commands.entity(e).despawn();
    }
    if capturing.0 {
        return;
    }
    let Some(doc) = index.snapshot.as_ref() else { return };
    let with_below = |id| {
        let mut ids = doc.descendants(id);
        ids.push(id);
        ids
    };
    let selected = match selection.selected {
        Some(SelectionTarget::Node(id)) => with_below(id),
        _ => Vec::new(),
    };
    let mut hovered: Vec<_> = highlighted.0.iter().flat_map(|&id| with_below(id)).collect();
    if let Some(SelectionTarget::Node(id)) = selection.hovered {
        hovered.extend(with_below(id));
    }
    let (select_mat, hover_mat) = material
        .get_or_insert_with(|| {
            let mut hull = |base_color| {
                materials.add(StandardMaterial { base_color, unlit: true, cull_mode: Some(Face::Front), ..default() })
            };
            (hull(OUTLINE), hull(HOVER))
        })
        .clone();
    let camera = camera.single().ok();
    for (piece, node, mesh, aabb, global) in &pieces {
        let (px, material) = if selected.contains(&node.0) {
            (OUTLINE_PX, &select_mat)
        } else if hovered.contains(&node.0) {
            (HOVER_PX, &hover_mat)
        } else {
            continue;
        };
        commands.spawn((
            Outline(px),
            ChildOf(piece),
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(material.clone()),
            NotShadowCaster,
            camera.map_or(Transform::IDENTITY, |c| hull_transform(c, aabb, global, px)),
        ));
    }
}

/// Keeps the hull about `OUTLINE_PX` wide while the camera moves (the camera only moves on
/// input or easing, so this costs nothing when idle).
fn resize_outline(
    camera: Query<CameraView, ChangedMainCamera>,
    mut outlines: Query<(&ChildOf, &Outline, &mut Transform)>,
    pieces: Query<(&Aabb, &GlobalTransform), With<SceneNode>>,
) {
    let Ok(camera) = camera.single() else { return };
    for (parent, outline, mut transform) in &mut outlines {
        if let Ok((aabb, global)) = pieces.get(parent.parent()) {
            let t = hull_transform(camera, aabb, global, outline.0);
            if *transform != t {
                *transform = t;
            }
        }
    }
}

/// Per-axis scale about the piece's box centre so every half-extent grows by `OUTLINE_PX`, also
/// on the thin sides of elongated pieces (POLA-104).
/// shortcut: exact for box-like pieces; curved or rotated-inside-the-piece shapes get a rim that
/// varies around them. Use a normal-extruded hull mesh if that looks wrong.
fn hull_transform((camera, eye, projection): CameraView, aabb: &Aabb, global: &GlobalTransform, px: f32) -> Transform {
    let centre = Vec3::from(aabb.center);
    let world_centre = global.transform_point(centre);
    let height_px = camera.physical_viewport_size().map_or(1.0, |s| s.y.max(1) as f32);
    let world_per_px = match projection {
        Projection::Perspective(p) => 2.0 * eye.translation().distance(world_centre) * (p.fov / 2.0).tan(),
        Projection::Orthographic(o) => o.area.height(),
        _ => 0.0,
    } / height_px;
    let half = (Vec3::from(aabb.half_extents) * global.scale().abs()).max(Vec3::splat(1e-6));
    let k = Vec3::ONE + px * world_per_px / half;
    Transform { translation: centre * (Vec3::ONE - k), scale: k, ..default() }
}
