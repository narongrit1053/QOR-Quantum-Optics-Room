//! Selection outline by inverted hull (M3-PLAN POLA-50): every mesh piece of the selected node and
//! of the nodes below it gets a slightly larger, front-culled, unlit accent copy as a child.

use bevy::{
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    light::NotShadowCaster,
    prelude::*,
    render::render_resource::Face,
};
use lab_core::SelectionTarget;

use crate::camera::OrbitCamera;
use crate::picking::{ACCENT, Selection};
use crate::scene::{SceneIndex, SceneNode};

/// Outline width in physical pixels.
const OUTLINE_PX: f32 = 2.0;

#[derive(Component)]
struct Outline;

pub(crate) struct OutlinePlugin;

impl Plugin for OutlinePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (rebuild_outline, resize_outline)
                .chain()
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate),
        );
    }
}

type CameraView<'a> = (&'a Camera, &'a GlobalTransform, &'a Projection);

/// Respawns the outline when the selection changed or pieces were (re)spawned; despawns it on
/// deselect. Platforms keep their tint and get no outline.
#[allow(clippy::too_many_arguments)]
fn rebuild_outline(
    mut commands: Commands,
    selection: Res<Selection>,
    index: Res<SceneIndex>,
    added: Query<(), (With<SceneNode>, Added<Aabb>)>,
    pieces: Query<(Entity, &SceneNode, &Mesh3d, &Aabb, &GlobalTransform)>,
    outlines: Query<Entity, With<Outline>>,
    camera: Query<CameraView, With<OrbitCamera>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
) {
    if !selection.is_changed() && added.is_empty() {
        return;
    }
    for e in &outlines {
        commands.entity(e).despawn();
    }
    let Some(SelectionTarget::Node(id)) = selection.selected else { return };
    let Some(doc) = index.snapshot.as_ref() else { return };
    let mut ids = doc.descendants(id);
    ids.push(id);
    let material = material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: ACCENT,
                unlit: true,
                cull_mode: Some(Face::Front),
                ..default()
            })
        })
        .clone();
    let camera = camera.single().ok();
    for (piece, node, mesh, aabb, global) in &pieces {
        if ids.contains(&node.0) {
            commands.spawn((
                Outline,
                ChildOf(piece),
                Mesh3d(mesh.0.clone()),
                MeshMaterial3d(material.clone()),
                NotShadowCaster,
                camera.map_or(Transform::IDENTITY, |c| hull_transform(c, aabb, global)),
            ));
        }
    }
}

/// Keeps the hull about `OUTLINE_PX` wide while the camera moves (the camera only moves on
/// input or easing, so this costs nothing when idle).
fn resize_outline(
    camera: Query<CameraView, (With<OrbitCamera>, Changed<GlobalTransform>)>,
    mut outlines: Query<(&ChildOf, &mut Transform), With<Outline>>,
    pieces: Query<(&Aabb, &GlobalTransform), With<SceneNode>>,
) {
    let Ok(camera) = camera.single() else { return };
    for (parent, mut transform) in &mut outlines {
        if let Ok((aabb, global)) = pieces.get(parent.parent()) {
            let t = hull_transform(camera, aabb, global);
            if *transform != t {
                *transform = t;
            }
        }
    }
}

/// Uniform scale about the piece's box centre so its largest half-extent grows by `OUTLINE_PX`.
/// shortcut: uniform scale makes thin sides of elongated pieces thinner than 2 px; use a
/// normal-extruded hull mesh if that looks wrong.
fn hull_transform((camera, eye, projection): CameraView, aabb: &Aabb, global: &GlobalTransform) -> Transform {
    let centre = Vec3::from(aabb.center);
    let world_centre = global.transform_point(centre);
    let height_px = camera.physical_viewport_size().map_or(1.0, |s| s.y.max(1) as f32);
    let world_per_px = match projection {
        Projection::Perspective(p) => 2.0 * eye.translation().distance(world_centre) * (p.fov / 2.0).tan(),
        Projection::Orthographic(o) => o.area.height(),
        _ => 0.0,
    } / height_px;
    let radius = (Vec3::from(aabb.half_extents) * global.scale()).max_element().max(1e-6);
    let k = 1.0 + OUTLINE_PX * world_per_px / radius;
    Transform { translation: centre * (1.0 - k), scale: Vec3::splat(k), ..default() }
}
