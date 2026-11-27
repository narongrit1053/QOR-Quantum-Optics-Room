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
/// Colliding parts outline: red at selection width.
const COLLIDE: Color = Color::srgb(0.95, 0.2, 0.2);

/// Width in physical px.
#[derive(Component)]
pub(crate) struct Outline(f32);

/// Marks a collision hull so hover/selection rebuilds keep it alive.
#[derive(Component)]
pub(crate) struct CollideOutline;

/// M9 G. Nodes outlined like hover (`ViewportRequest::HighlightNodes`).
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Highlighted(Vec<lab_core::NodeId>);

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

/// Respawns selection/hover hulls on selection, hover, highlight, collision or piece changes;
/// collision hulls respawn only on collision, piece or capture changes and are hidden while selected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_outline(
    mut commands: Commands,
    selection: Res<Selection>,
    capturing: Res<crate::export::Capturing>,
    index: Res<SceneIndex>,
    pending: Res<crate::host::PendingRequests>,
    mut highlighted: ResMut<Highlighted>,
    collisions: Res<crate::collide::Collisions>,
    added: Query<(), (With<SceneNode>, Added<Aabb>)>,
    pieces: Query<(Entity, &SceneNode, &Mesh3d, &Aabb, &GlobalTransform)>,
    sel_outlines: Query<Entity, (With<Outline>, Without<CollideOutline>)>,
    mut collide_outlines: Query<(Entity, &ChildOf, &mut Visibility), With<CollideOutline>>,
    camera: Query<CameraView, (With<OrbitCamera>, Without<TopPane>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<(Handle<StandardMaterial>, Handle<StandardMaterial>, Handle<StandardMaterial>)>>,
) {
    for request in &pending.0 {
        if let crate::FrameRequest::HighlightNodes(ids) = request {
            highlighted.set_if_neq(Highlighted(ids.clone()));
        }
    }
    let collide_dirty = collisions.is_changed() || !added.is_empty() || capturing.is_changed();
    let sel_dirty = collide_dirty || selection.is_changed() || highlighted.is_changed();
    if !sel_dirty {
        return;
    }
    if collide_dirty {
        for (e, _, _) in &collide_outlines {
            commands.entity(e).despawn();
        }
    }
    for e in &sel_outlines {
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
    let mut selected = match selection.selected {
        Some(SelectionTarget::Node(id)) => with_below(id),
        _ => Vec::new(),
    };
    for &id in &selection.extra {
        selected.extend(with_below(id));
    }
    let colliding: Vec<_> = collisions.0.iter().flat_map(|&(a, b)| [a, b]).collect();
    let mut hovered: Vec<_> = highlighted.0.iter().flat_map(|&id| with_below(id)).collect();
    if let Some(SelectionTarget::Node(id)) = selection.hovered {
        hovered.extend(with_below(id));
    }
    if !collide_dirty && selection.is_changed() {
        for (_, parent, mut vis) in &mut collide_outlines {
            if let Ok((_, node, ..)) = pieces.get(parent.parent()) {
                let want = if selected.contains(&node.0) { Visibility::Hidden } else { Visibility::Inherited };
                if *vis != want {
                    *vis = want;
                }
            }
        }
    }
    let (select_mat, collide_mat, hover_mat) = material
        .get_or_insert_with(|| {
            let mut hull = |base_color| {
                materials.add(StandardMaterial { base_color, unlit: true, cull_mode: Some(Face::Front), ..default() })
            };
            (hull(OUTLINE), hull(COLLIDE), hull(HOVER))
        })
        .clone();
    let camera = camera.single().ok();
    for (piece, node, mesh, aabb, global) in &pieces {
        let is_selected = selected.contains(&node.0);
        let is_colliding = colliding.contains(&node.0);
        if collide_dirty && is_colliding {
            let vis = if is_selected { Visibility::Hidden } else { Visibility::Inherited };
            spawn_hull(&mut commands, camera, (piece, mesh, aabb, global), OUTLINE_PX, &collide_mat, vis)
                .insert(CollideOutline);
        }
        let (px, material) = if is_selected {
            (OUTLINE_PX, &select_mat)
        } else if hovered.contains(&node.0) && !is_colliding {
            (HOVER_PX, &hover_mat)
        } else {
            continue;
        };
        spawn_hull(&mut commands, camera, (piece, mesh, aabb, global), px, material, Visibility::Inherited);
    }
}

fn spawn_hull<'a>(
    commands: &'a mut Commands,
    camera: Option<CameraView>,
    (piece, mesh, aabb, global): (Entity, &Mesh3d, &Aabb, &GlobalTransform),
    px: f32,
    material: &Handle<StandardMaterial>,
    visibility: Visibility,
) -> EntityCommands<'a> {
    let transform = camera.map_or(Transform::IDENTITY, |c| hull_transform(c, aabb, global, px));
    commands.spawn((
        Outline(px),
        ChildOf(piece),
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(material.clone()),
        NotShadowCaster,
        visibility,
        transform,
        global.mul_transform(transform),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::{Document, NodeId};

    fn test_app() -> App {
        let mut app = App::new();
        app.insert_resource(Selection::default())
            .insert_resource(crate::export::Capturing::default())
            .insert_resource(SceneIndex { snapshot: Some(Document::new("test")), ..default() })
            .insert_resource(crate::host::PendingRequests::default())
            .insert_resource(Highlighted::default())
            .insert_resource(crate::collide::Collisions(vec![(NodeId(1), NodeId(2))]))
            .insert_resource(Assets::<StandardMaterial>::default())
            .add_systems(Update, rebuild_outline);
        app.world_mut().spawn((
            SceneNode(NodeId(1)),
            Mesh3d::default(),
            Aabb::default(),
            GlobalTransform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
        ));
        app.world_mut().spawn((
            SceneNode(NodeId(3)),
            Mesh3d::default(),
            Aabb::default(),
            GlobalTransform::from_translation(Vec3::new(4.0, 5.0, 6.0)),
        ));
        app
    }

    #[test]
    fn collision_hulls_survive_hover_and_hide_while_selected() {
        let mut app = test_app();
        app.update();

        let collide_id = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(Entity, &Visibility, &GlobalTransform), With<CollideOutline>>();
            let items: Vec<_> = q.iter(world).collect();
            assert_eq!(items.len(), 1);
            assert_eq!(*items[0].1, Visibility::Inherited);
            assert_eq!(items[0].2.translation(), Vec3::new(1.0, 2.0, 3.0));
            items[0].0
        };

        // Hovering a colliding node does not spawn a hover hull or respawn the collision hull.
        app.world_mut().resource_mut::<Selection>().hovered = Some(SelectionTarget::Node(NodeId(1)));
        app.update();
        {
            let world = app.world_mut();
            let mut cq = world.query_filtered::<Entity, With<CollideOutline>>();
            assert_eq!(cq.iter(world).collect::<Vec<_>>(), vec![collide_id]);
            let mut sq = world.query_filtered::<Entity, (With<Outline>, Without<CollideOutline>)>();
            assert_eq!(sq.iter(world).count(), 0);
        }

        // Hovering a non-colliding node spawns a hover hull without touching the collision hull.
        app.world_mut().resource_mut::<Selection>().hovered = Some(SelectionTarget::Node(NodeId(3)));
        app.update();
        {
            let world = app.world_mut();
            let mut cq = world.query_filtered::<Entity, With<CollideOutline>>();
            assert_eq!(cq.iter(world).collect::<Vec<_>>(), vec![collide_id]);
            let mut sq = world.query_filtered::<(&Outline, &GlobalTransform), Without<CollideOutline>>();
            let items: Vec<_> = sq.iter(world).collect();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].0.0, HOVER_PX);
            assert_eq!(items[0].1.translation(), Vec3::new(4.0, 5.0, 6.0));
        }

        // Selecting the colliding node hides its collision hull (without despawning) and spawns selection hull.
        app.world_mut().resource_mut::<Selection>().selected = Some(SelectionTarget::Node(NodeId(1)));
        app.update();
        {
            let world = app.world_mut();
            let mut cq = world.query_filtered::<(Entity, &Visibility), With<CollideOutline>>();
            assert_eq!(cq.iter(world).map(|(e, v)| (e, *v)).collect::<Vec<_>>(), vec![(collide_id, Visibility::Hidden)]);
            let mut sq = world.query_filtered::<&Outline, Without<CollideOutline>>();
            assert_eq!(sq.iter(world).map(|o| o.0).collect::<Vec<_>>(), vec![OUTLINE_PX, HOVER_PX]);
        }

        // Deselecting restores Visibility::Inherited on the same collision hull entity.
        app.world_mut().resource_mut::<Selection>().selected = None;
        app.update();
        {
            let world = app.world_mut();
            let mut cq = world.query_filtered::<(Entity, &Visibility), With<CollideOutline>>();
            assert_eq!(cq.iter(world).map(|(e, v)| (e, *v)).collect::<Vec<_>>(), vec![(collide_id, Visibility::Inherited)]);
        }

        // Capturing removes both kinds; ending capture respawns them.
        app.world_mut().resource_mut::<crate::export::Capturing>().0 = true;
        app.update();
        {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<Outline>>();
            assert_eq!(q.iter(world).count(), 0);
        }
        app.world_mut().resource_mut::<crate::export::Capturing>().0 = false;
        app.update();
        {
            let world = app.world_mut();
            let mut cq = world.query_filtered::<Entity, With<CollideOutline>>();
            assert_eq!(cq.iter(world).count(), 1);
            let mut sq = world.query_filtered::<Entity, (With<Outline>, Without<CollideOutline>)>();
            assert_eq!(sq.iter(world).count(), 1);
        }
    }
}
