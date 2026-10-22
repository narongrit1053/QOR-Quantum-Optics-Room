//! Library drag-and-drop into the scene (M2-PLAN POLA-43): a translucent ghost of the dragged
//! part follows the pointer, snapped to the nearest platform hole or mated to a compatible
//! connector of the part under the pointer; a drop asks the app to place it.
//!
//! The viewport never edits the document: the drop becomes [`ViewportEvent::PlaceRequested`],
//! and the app turns that into a command.

use bevy::{
    light::NotShadowCaster,
    math::{DQuat, DVec2, DVec3},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_catalog::Catalog;
use lab_core::part::PartDef;
use lab_core::{Attach, Document, NodeId, Parent, PlatformId, SelectionTarget, Shape, compute_holes};

use crate::camera::{OrbitCamera, camera_at};
use crate::host::{EventSender, PendingRequests, SettleFrames, ViewportSize};
use crate::picking::PickId;
use crate::scene::{PartMeshes, SceneIndex, from_render, part_mesh, rotation_to_render, to_render};
use crate::{CatalogRes, DragItem, FrameRequest, PartDrag, PlaceRequest, StackSpec, ViewportEvent};

/// The parts above the root of `spec`, bottom to top: catalog id, how it mates to the part below,
/// and its pose relative to that part. `None` when a part or a stack connector is missing.
pub fn stack_chain(catalog: &Catalog, spec: &StackSpec) -> Option<Vec<(String, Attach, DVec3, DQuat)>> {
    // (part, its connector, the connector of the part below, slide)
    let mut links: Vec<(&str, &str, &str, f64)> = Vec::new();
    if spec.base.is_some() {
        links.push((&spec.holder, "bottom", "top", 0.0));
    }
    links.push((&spec.post, "bottom", "bore", spec.slide));
    links.push((&spec.mount, "base_tap", "top", 0.0));
    if let Some(optic) = &spec.optic {
        links.push((optic, "edge", "optic_seat", 0.0));
    }
    let mut below = spec.base.as_deref().unwrap_or(&spec.holder);
    let mut out = Vec::new();
    for (part, own, parent, slide) in links {
        let child = catalog.get(part)?.connector(own)?;
        let parent_conn = catalog.get(below)?.connector(parent)?;
        let (position, rotation) = lab_mates::mate_pose(&parent_conn.frame, &child.frame, slide, 0.0);
        let attach = Attach::Mate { self_conn: own.into(), parent_conn: parent.into(), slide, twist: 0.0 };
        out.push((part.to_string(), attach, position, rotation));
        below = part;
    }
    Some(out)
}

/// What the pointer is over, in room coordinates (mm, +Z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Hit {
    Node(NodeId, DVec3),
    Platform(PlatformId, DVec3),
    Floor(DVec3),
}

/// The placement for dragging `def` over `hit`, and its pose in the room frame (for the ghost).
/// `free` (Shift) skips hole snapping. `None` when the hit cannot hold the part.
pub(crate) fn placement(
    doc: &Document,
    catalog: &Catalog,
    item: &DragItem,
    def: &PartDef,
    hit: Hit,
    free: bool,
) -> Option<(PlaceRequest, (DVec3, DQuat))> {
    match hit {
        Hit::Node(id, point) => {
            let root = doc.root_platform(id);
            if let DragItem::Part(_) = item
                && let Some(found) = mate_on_node(doc, catalog, item, def, id, point) {
                return Some(found);
            }
            // No compatible connector: place on whatever the part stands on.
            match root {
                Some(p) => placement(doc, catalog, item, def, Hit::Platform(p, point), free),
                None => placement(doc, catalog, item, def, Hit::Floor(point), free),
            }
        }
        Hit::Platform(pid, point) => {
            let (origin, angle) = doc.platform_pose(pid)?;
            let local = DQuat::from_rotation_z(-angle) * (point - origin);
            let platform = doc.platform(pid)?;
            let snapped = (!free)
                .then(|| platform_holes(platform))
                .flatten()
                .and_then(|holes| lab_mates::snap(&holes, local.truncate(), holes.pitch.max_element()));
            let (attach, at) = match snapped {
                Some(hit) => (Attach::Hole { hole: hit.hole }, hit.position),
                None => (Attach::Free, local.truncate()),
            };
            let (position, rotation) = lab_mates::seat_pose(def, at, 0.0);
            let request = PlaceRequest { item: item.clone(), parent: Parent::Platform(pid), attach, position, rotation };
            let turn = DQuat::from_rotation_z(angle);
            Some((request, (origin + turn * position, turn * rotation)))
        }
        Hit::Floor(point) => {
            let (position, rotation) = lab_mates::seat_pose(def, DVec2::new(point.x, point.y), 0.0);
            let request = PlaceRequest { item: item.clone(), parent: Parent::Room, attach: Attach::Free, position, rotation };
            Some((request, (position, rotation)))
        }
    }
}

thread_local! {
    /// The holes of the last platform dragged over; recomputed only when that platform changes.
    static HOLES: std::cell::RefCell<Option<(lab_core::Platform, std::rc::Rc<lab_core::HoleSet>)>> = Default::default();
}

fn platform_holes(platform: &lab_core::Platform) -> Option<std::rc::Rc<lab_core::HoleSet>> {
    HOLES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((p, holes)) = &*cache
            && p == platform
        {
            return Some(holes.clone());
        }
        let holes = std::rc::Rc::new(compute_holes(platform).ok()?);
        *cache = Some((platform.clone(), holes.clone()));
        Some(holes)
    })
}

/// Mates `def` to the connector of node `id` nearest to `point` that has a compatible partner.
fn mate_on_node(
    doc: &Document,
    catalog: &Catalog,
    item: &DragItem,
    def: &PartDef,
    id: NodeId,
    point: DVec3,
) -> Option<(PlaceRequest, (DVec3, DQuat))> {
    let node = doc.node(id).filter(|n| n.shape == Shape::Part)?;
    let parent_def = catalog.get(&node.part)?;
    // The node's pose in the room frame.
    let (pos, rot) = doc.node_pose(id)?;
    let (origin, turn) = match doc.root_platform(id) {
        Some(p) => doc.platform_pose(p).map(|(o, a)| (o, DQuat::from_rotation_z(a)))?,
        None => (DVec3::ZERO, DQuat::IDENTITY),
    };
    let (room_pos, room_rot) = (origin + turn * pos, turn * rot);
    let local = room_rot.inverse() * (point - room_pos);
    let (ci, pi) = lab_mates::compatible_pairs(def, parent_def).into_iter().min_by(|a, b| {
        let d = |&(_, p): &(usize, usize)| parent_def.connectors[p].frame.origin.distance(local);
        d(a).total_cmp(&d(b))
    })?;
    let (child, parent) = (&def.connectors[ci], &parent_def.connectors[pi]);
    let (lo, hi) = lab_mates::slide_range(&parent.kind);
    let parent = &lab_core::part::ConnectorDef {
        frame: lab_mates::connector_frame(parent_def, &parent.name, &node.params)?,
        ..parent.clone()
    };
    // Slide: the pointer's position along the slide axis (pose is linear in slide), clamped.
    let p0 = lab_mates::mate_pose(&parent.frame, &child.frame, 0.0, 0.0).0;
    let axis = lab_mates::mate_pose(&parent.frame, &child.frame, 1.0, 0.0).0 - p0;
    let along = if axis.length_squared() > 1e-12 { (local - p0).dot(axis) / axis.length_squared() } else { 0.0 };
    let slide = along.clamp(lo, hi);
    // Twist: keep the yaw the part had while dragged over the platform (its X along the
    // platform's X), the nearest whole degree.
    let want = turn * DVec3::X;
    let twist = (0..360)
        .map(|d| (d as f64).to_radians())
        .min_by(|a, b| {
            let fit = |t: &f64| (room_rot * lab_mates::mate_pose(&parent.frame, &child.frame, slide, *t).1 * DVec3::X).dot(want);
            fit(b).total_cmp(&fit(a))
        })
        .unwrap_or(0.0);
    let (position, rotation) = lab_mates::mate_pose(&parent.frame, &child.frame, slide, twist);
    let request = PlaceRequest {
        item: item.clone(),
        parent: Parent::Node(id),
        attach: Attach::Mate { self_conn: child.name.clone(), parent_conn: parent.name.clone(), slide, twist },
        position,
        rotation,
    };
    Some((request, (room_pos + room_rot * position, room_rot * rotation)))
}

/// The ghost of the current drag: a root entity (room frame) with one child per mesh piece.
#[derive(Resource, Default)]
struct Ghost {
    entity: Option<Entity>,
    item: Option<DragItem>,
    material: Option<Handle<StandardMaterial>>,
    /// Set by `drag_part` for `update_ghost`: build the ghost of `item` anew.
    rebuild: bool,
    /// Set by `drag_part` for `update_ghost`: the ghost's room-frame pose (`Some(None)` hides it).
    pose: Option<Option<(DVec3, DQuat)>>,
}

pub(crate) struct PlacePlugin;

impl Plugin for PlacePlugin {
    fn build(&self, app: &mut App) {
        // Two systems: MeshRayCast reads `Assets<Mesh>`, which building the ghost writes.
        app.init_resource::<Ghost>().add_systems(Update, (drag_part, update_ghost).chain());
    }
}

fn drag_part(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    targets: Query<&PickId>,
    mut ray_cast: MeshRayCast,
    mut ghost: ResMut<Ghost>,
    (index, catalog, events, full): (Res<SceneIndex>, Res<CatalogRes>, Res<EventSender>, Res<ViewportSize>),
) {
    for request in &pending.0 {
        let FrameRequest::DragPart(drag) = request else { continue };
        let (pos, mods, item) = match drag {
            PartDrag::Move { item, pos, mods } => (*pos, *mods, item.clone()),
            PartDrag::Drop { pos, mods } => match ghost.item.clone() {
                Some(item) => (*pos, *mods, item),
                None => continue,
            },
            PartDrag::Cancel => {
                clear(&mut commands, &mut ghost);
                continue;
            }
        };
        let Some(doc) = &index.snapshot else { continue };
        let Some(def) = catalog.0.get(item.root()) else { continue };

        // Ray from the pointer into the scene; the ghost has no PickId, so it is never hit.
        let Some(((), camera, camera_transform, p)) = camera_at(cameras.iter().map(|(c, t)| ((), c, t)), &full, pos) else { continue };
        let Ok(ray) = camera.viewport_to_world(camera_transform, p) else { continue };
        let filter = |entity: Entity| targets.contains(entity);
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        let hit = match ray_cast.cast_ray(ray, &settings).first().map(|(e, h)| (targets.get(*e).ok(), h.point)) {
            Some((Some(PickId(SelectionTarget::Node(n))), p)) => Some(Hit::Node(*n, from_render(p))),
            Some((Some(PickId(SelectionTarget::Platform(p))), point)) => Some(Hit::Platform(*p, from_render(point))),
            // The room (floor or a wall), or nothing: the floor plane under the pointer.
            _ => ray
                .intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
                .map(|t| Hit::Floor(from_render(ray.get_point(t)))),
        };
        let placed = hit.and_then(|hit| placement(doc, &catalog.0, &item, def, hit, mods.shift));

        if matches!(drag, PartDrag::Drop { .. }) {
            if let Some((request, _)) = placed {
                events.send(ViewportEvent::PlaceRequested(request));
            }
            clear(&mut commands, &mut ghost);
            continue;
        }
        if ghost.item.as_ref() != Some(&item) {
            clear(&mut commands, &mut ghost);
            ghost.item = Some(item);
            ghost.rebuild = true;
        }
        ghost.pose = Some(placed.map(|(_, pose)| pose));
    }
}

type GhostAssets<'w> = (
    ResMut<'w, Assets<Mesh>>,
    ResMut<'w, Assets<StandardMaterial>>,
    ResMut<'w, PartMeshes>,
    ResMut<'w, SettleFrames>,
);

/// Builds the ghost when the dragged item changed and moves it to its pose.
fn update_ghost(
    mut commands: Commands,
    mut ghost: ResMut<Ghost>,
    (catalog, events): (Res<CatalogRes>, Res<EventSender>),
    (mut meshes, mut materials, mut part_meshes, mut settle): GhostAssets,
) {
    let Some(pose) = ghost.pose.take() else { return };
    if std::mem::take(&mut ghost.rebuild)
        && let Some(item) = ghost.item.clone()
    {
        let material = ghost
            .material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::srgba(10.0 / 255.0, 132.0 / 255.0, 1.0, 0.45),
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 0.5,
                    ..default()
                })
            })
            .clone();
        let root = commands.spawn((Transform::default(), Visibility::Hidden)).id();
        // Every part of the item with its pose relative to the root part.
        let mut parts = vec![(item.root().to_string(), DVec3::ZERO, DQuat::IDENTITY)];
        if let DragItem::Stack(spec) = &item {
            for (id, _, position, rotation) in stack_chain(&catalog.0, spec).unwrap_or_default() {
                let &(_, p, r) = parts.last().unwrap();
                parts.push((id, p + r * position, r * rotation));
            }
        }
        for (id, position, rotation) in parts {
            match part_mesh(&catalog.0, &id, &Default::default()) {
                Ok(part) => {
                    let transform =
                        Transform { translation: to_render(position), rotation: rotation_to_render(rotation), ..default() };
                    for (_, mesh) in part_meshes.pieces(&mut meshes, &part) {
                        commands.spawn((
                            ChildOf(root),
                            Mesh3d(mesh),
                            MeshMaterial3d(material.clone()),
                            NotShadowCaster,
                            transform,
                        ));
                    }
                }
                Err(error) => events.send(ViewportEvent::SceneError(format!("{id} has no preview: {error}."))),
            }
        }
        ghost.entity = Some(root);
        settle.0 = settle.0.max(2);
    }
    let Some(entity) = ghost.entity else { return };
    match pose {
        Some((position, rotation)) => commands.entity(entity).insert((
            Transform { translation: to_render(position), rotation: rotation_to_render(rotation), ..default() },
            Visibility::Inherited,
        )),
        None => commands.entity(entity).insert(Visibility::Hidden),
    };
}

fn clear(commands: &mut Commands, ghost: &mut Ghost) {
    if let Some(entity) = ghost.entity.take() {
        commands.entity(entity).try_despawn();
    }
    ghost.item = None;
    ghost.rebuild = false;
    ghost.pose = None;
}

#[cfg(test)]
mod tests {
    use lab_core::Node;

    use super::*;

    fn def(json: serde_json::Value) -> PartDef {
        serde_json::from_value(json).unwrap()
    }

    fn holder() -> PartDef {
        def(serde_json::json!({
            "id": "t:PH50/M", "name": "PH50/M", "category": "posts/holders",
            "geometry": { "kind": "parametric", "generator": "post_holder" },
            "connectors": [
                { "name": "seat", "kind": "hole_grid_seat", "frame": { "origin": [0,0,0], "z": [0,0,-1], "x": [1,0,0] } },
                { "name": "bore", "kind": "bore", "diameter": 12.7, "gender": "female", "range": [-45.0, -5.0],
                  "frame": { "origin": [0,0,50], "z": [0,0,1], "x": [1,0,0] } }
            ]
        }))
    }

    fn post() -> PartDef {
        def(serde_json::json!({
            "id": "t:TR50/M", "name": "TR50/M", "category": "posts/posts",
            "geometry": { "kind": "parametric", "generator": "post" },
            "connectors": [
                { "name": "bottom", "kind": "bore", "diameter": 12.7, "gender": "male",
                  "frame": { "origin": [0,0,0], "z": [0,0,-1], "x": [1,0,0] } }
            ]
        }))
    }

    /// The holder snaps to a hole of the sample table; the post mates into the holder's bore.
    #[test]
    fn holder_snaps_and_post_mates() {
        let mut doc = lab_core::sample::demo_document();
        let catalog = Catalog::from_defs(vec![holder(), post()]);
        let pid = doc.platforms[0].id;
        let (origin, angle) = doc.platform_pose(pid).unwrap();
        let point = origin + DQuat::from_rotation_z(angle) * DVec3::new(3.0, -4.0, 0.0);
        let item = DragItem::Part("t:PH50/M".into());
        let (request, _) = placement(&doc, &catalog, &item, &holder(), Hit::Platform(pid, point), false).unwrap();
        assert!(matches!(request.attach, Attach::Hole { .. }), "{request:?}");
        assert_eq!(request.position.z, 0.0);

        let id = NodeId(doc.next_free_id());
        doc.nodes.push(Node {
            note: String::new(),
            id,
            name: "PH50/M".into(),
            part: "t:PH50/M".into(),
            parent: request.parent,
            attach: request.attach.clone(),
            position: request.position,
            rotation: request.rotation,
            shape: Shape::Part,
            finish: lab_core::Finish::AnodizedBlack,
            params: Default::default(),
            pickable: true,
            locked: false,
        });
        let (pos, _) = doc.node_pose(id).unwrap();
        let top = origin + DQuat::from_rotation_z(angle) * (pos + DVec3::new(0.0, 0.0, 49.0));
        let item = DragItem::Part("t:TR50/M".into());
        let (request, (room, _)) = placement(&doc, &catalog, &item, &post(), Hit::Node(id, top), false).unwrap();
        assert_eq!(request.parent, Parent::Node(id));
        assert!(matches!(request.attach, Attach::Mate { slide, .. } if slide == -5.0), "{request:?}");
        // Post origin 5 mm below the holder top (50 mm): z = 45 in the holder frame.
        assert!((request.position - DVec3::new(0.0, 0.0, 45.0)).length() < 1e-9, "{request:?}");
        assert!((room.z - (origin.z + 45.0)).abs() < 1e-9);
    }
}
