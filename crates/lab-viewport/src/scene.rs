//! Builds Bevy entities from document snapshots: room, platforms (with hole grids), nodes and
//! lighting, kept in sync incrementally (M1-SPEC §5.3).
//!
//! Render space is Bevy's: metres, +Y up. Document space is millimetres, +Z up. Convert only
//! through [`to_render`], [`rotation_to_render`], [`normal_to_render`] and [`from_render`].

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;

use bevy::{
    ecs::system::SystemParam,
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, GlobalAmbientLight, NotShadowCaster},
    math::{DQuat, DVec3},
    prelude::*,
};
use lab_core::{Document, Finish, Node, NodeId, PlatformId, SelectionTarget, Shape, Support};

use crate::hole_material::HoleTopMaterial;
use crate::host::{EventSender, PendingRequests, SettleFrames};
use crate::picking::{HighlightLook, PickId, Selection};
use crate::platform::{PlatformAssets, PlatformRoot, platform_root_transform, spawn_platform_parts};
use crate::room::spawn_room;
use crate::sync::{SyncOp, plan_changes, plan_full};
use crate::{FrameRequest, Quality, ViewportEvent};

/// Render units per millimetre.
pub(crate) const MM: f32 = 0.001;

pub(crate) fn to_render(p: DVec3) -> Vec3 {
    Vec3::new(p.x as f32, p.z as f32, -p.y as f32) * MM
}

/// Document (Z-up) rotation → render (Y-up) rotation.
pub(crate) fn rotation_to_render(q: DQuat) -> Quat {
    let change = Quat::from_rotation_x(-FRAC_PI_2);
    change * q.as_quat() * change.inverse()
}

/// Document direction → render direction (no scaling).
pub(crate) fn normal_to_render(n: DVec3) -> Vec3 {
    Vec3::new(n.x as f32, n.z as f32, -n.y as f32)
}

/// Render-space point → document millimetres.
#[cfg_attr(not(test), allow(dead_code))] // Used by room.rs (T4c).
pub(crate) fn from_render(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, -v.z as f64, v.y as f64) * 1000.0
}

/// Everything spawned from a document; despawned when another document is loaded.
#[derive(Component)]
pub(crate) struct SceneEntity;

/// Large background geometry (floor, walls).
#[derive(Component)]
pub(crate) struct Backdrop;

#[derive(Component, Clone, Copy)]
pub(crate) struct SceneNode(pub NodeId);

/// The entities built from the current snapshot.
#[derive(Resource, Default)]
pub(crate) struct SceneIndex {
    pub snapshot: Option<Document>,
    pub room: Vec<Entity>,
    pub platforms: BTreeMap<PlatformId, PlatformEntities>,
    pub nodes: BTreeMap<NodeId, Entity>,
}

/// `parts` = top, body, legs (children of `root`). The nodes on the platform are children of
/// `root` too, but not parts, so rebuilding the parts keeps them.
pub(crate) struct PlatformEntities {
    pub root: Entity,
    pub parts: Vec<Entity>,
}

pub(crate) struct ScenePlugin {
    pub document: Document,
}

#[derive(Resource)]
struct InitialDocument(Option<Document>);

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InitialDocument(Some(self.document.clone())))
            .init_resource::<SceneIndex>()
            // Transparent, so the UI's background gradient shows behind the scene.
            .insert_resource(ClearColor(Color::NONE))
            .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.9, 1.0), brightness: 160.0, ..default() })
            .insert_resource(DirectionalLightShadowMap { size: 1024 })
            .add_systems(Startup, (spawn_lighting, queue_initial_document))
            .add_systems(PreUpdate, apply_scene_requests);
    }
}

fn spawn_lighting(mut commands: Commands, quality: Res<Quality>) {
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: quality.shadows(), ..default() },
        Transform::from_xyz(2.5, 6.0, 3.5).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
        // One cascade over the working area keeps shadows affordable on integrated GPUs.
        CascadeShadowConfigBuilder { num_cascades: 1, maximum_distance: 8.0, ..default() }.build(),
    ));
    // Cool fill from the opposite side, no shadows.
    commands.spawn((
        DirectionalLight { illuminance: 2_500.0, color: Color::srgb(0.8, 0.88, 1.0), ..default() },
        Transform::from_xyz(-3.0, 3.0, -2.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
    ));
}

/// The document passed to `start` is loaded like any other, in the first frame.
fn queue_initial_document(mut initial: ResMut<InitialDocument>, mut pending: ResMut<PendingRequests>) {
    if let Some(doc) = initial.0.take() {
        pending.0.insert(0, FrameRequest::LoadDocument(Box::new(doc)));
    }
}

#[derive(SystemParam)]
pub(crate) struct SceneAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    hole_materials: ResMut<'w, Assets<HoleTopMaterial>>,
    images: ResMut<'w, Assets<Image>>,
}

/// Carries out `LoadDocument` (full rebuild) and `ApplyChanges` (incremental) requests in order.
/// Everything before the last `LoadDocument` of a batch is superseded by it.
#[allow(clippy::too_many_arguments)]
fn apply_scene_requests(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    mut index: ResMut<SceneIndex>,
    existing: Query<Entity, With<SceneEntity>>,
    mut assets: SceneAssets,
    mut settle: ResMut<SettleFrames>,
    mut selection: ResMut<Selection>,
    events: Res<EventSender>,
) {
    let start = pending.0.iter().rposition(|r| matches!(r, FrameRequest::LoadDocument(_))).unwrap_or(0);
    let mut touched = false;
    for request in &pending.0[start..] {
        let (doc, ops) = match request {
            FrameRequest::LoadDocument(doc) => {
                for entity in &existing {
                    commands.entity(entity).try_despawn();
                }
                *index = SceneIndex::default();
                if selection.selected.take().is_some() {
                    events.send(ViewportEvent::SelectionChanged(None));
                }
                if selection.hovered.take().is_some() {
                    events.send(ViewportEvent::HoverChanged(None));
                }
                (doc, plan_full(doc))
            }
            FrameRequest::ApplyChanges(doc, changes) => {
                let ops = match &index.snapshot {
                    Some(old) => plan_changes(old, doc, changes),
                    None => plan_full(doc),
                };
                (doc, ops)
            }
            _ => continue,
        };
        touched = true;
        let spawns =
            ops.iter().any(|op| matches!(op, SyncOp::SpawnRoom | SyncOp::BuildPlatform(_) | SyncOp::SpawnNode(_)));
        for op in ops {
            apply_op(&mut commands, &mut index, doc, op, &mut assets, &events);
        }
        index.snapshot = Some((**doc).clone());
        if spawns {
            settle.0 = settle.0.max(2);
        }
    }
    if !touched {
        return;
    }

    // Forget targets that no longer exist.
    let Some(doc) = &index.snapshot else { return };
    if let Some(t) = selection.selected
        && !t.exists_in(doc)
    {
        selection.selected = None;
        events.send(ViewportEvent::SelectionChanged(None));
    }
    if let Some(t) = selection.hovered
        && !t.exists_in(doc)
    {
        selection.hovered = None;
        events.send(ViewportEvent::HoverChanged(None));
    }
}

fn apply_op(
    commands: &mut Commands,
    index: &mut SceneIndex,
    doc: &Document,
    op: SyncOp,
    assets: &mut SceneAssets,
    events: &EventSender,
) {
    match op {
        SyncOp::DespawnNode(id) => {
            if let Some(entity) = index.nodes.remove(&id) {
                commands.entity(entity).try_despawn();
            }
        }
        SyncOp::DespawnPlatform(id) => {
            if let Some(platform) = index.platforms.remove(&id) {
                // Recursive: the parts and the node children go with the root.
                commands.entity(platform.root).try_despawn();
            }
            if let Some(old) = &index.snapshot {
                for node in old.nodes_on(id) {
                    index.nodes.remove(&node.id);
                }
            }
        }
        SyncOp::DespawnRoom => {
            for entity in index.room.drain(..) {
                commands.entity(entity).try_despawn();
            }
        }
        SyncOp::SpawnRoom => {
            for entity in index.room.drain(..) {
                commands.entity(entity).try_despawn();
            }
            index.room = spawn_room(commands, &doc.room, &mut assets.meshes, &mut assets.materials);
        }
        SyncOp::BuildPlatform(id) => {
            let Some(p) = doc.platform(id) else { return };
            let transform = platform_root_transform(doc, p);
            let root = match index.platforms.remove(&id) {
                Some(existing) => {
                    for part in existing.parts {
                        commands.entity(part).try_despawn();
                    }
                    commands.entity(existing.root).insert(transform);
                    existing.root
                }
                None => commands.spawn((SceneEntity, PlatformRoot(id), transform, Visibility::default())).id(),
            };
            let mut platform_assets = PlatformAssets {
                meshes: &mut assets.meshes,
                materials: &mut assets.materials,
                hole_materials: &mut assets.hole_materials,
                images: &mut assets.images,
            };
            let parts = spawn_platform_parts(commands, root, p, &mut platform_assets).unwrap_or_else(|error| {
                events.send(ViewportEvent::SceneError(format!("Platform “{}” could not be drawn: {error}", p.name)));
                Vec::new()
            });
            index.platforms.insert(id, PlatformEntities { root, parts });
            place_stacked(commands, index, doc);
        }
        SyncOp::PlacePlatform(id) => {
            if let (Some(p), Some(platform)) = (doc.platform(id), index.platforms.get(&id)) {
                commands.entity(platform.root).insert(platform_root_transform(doc, p));
            }
            place_stacked(commands, index, doc);
        }
        SyncOp::SpawnNode(id) => {
            let Some(node) = doc.node(id) else { return };
            // shortcut: room-rooted nodes (Parent::Room) are not drawn yet; POLA-43 adds them.
            let Some(root) = doc.root_platform(id).and_then(|p| index.platforms.get(&p)).map(|p| p.root) else {
                events.send(ViewportEvent::SceneError(format!("“{}” could not be drawn: its platform is missing.", node.name)));
                return;
            };
            if let Some(old) = index.nodes.remove(&id) {
                commands.entity(old).try_despawn();
            }
            let (position, rotation) = doc.node_pose(id).unwrap_or((node.position, node.rotation));
            let entity = spawn_node(commands, root, node, position, rotation, &mut assets.meshes, &mut assets.materials);
            index.nodes.insert(id, entity);
        }
    }
}

/// Re-places every stacked (`OnPlatform`) platform, so one moved anywhere below carries it.
fn place_stacked(commands: &mut Commands, index: &SceneIndex, doc: &Document) {
    for p in doc.platforms.iter().filter(|p| matches!(p.support, Support::OnPlatform { .. })) {
        if let Some(platform) = index.platforms.get(&p.id) {
            commands.entity(platform.root).insert(platform_root_transform(doc, p));
        }
    }
}

/// A node as a child of its root platform's root, at its pose in that platform's frame.
fn spawn_node(
    commands: &mut Commands,
    root: Entity,
    node: &Node,
    position: DVec3,
    rotation: DQuat,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Entity {
    let transform = Transform {
        translation: to_render(position),
        rotation: rotation_to_render(rotation),
        ..default()
    };
    let mesh = match &node.shape {
        // Document Y maps to render −Z, so box depth and height swap places.
        Shape::Box { size } => meshes.add(Cuboid::new(size.x as f32 * MM, size.z as f32 * MM, size.y as f32 * MM)),
        // Bevy cylinders run along local Y, which is the document's local Z after conversion.
        Shape::Cylinder { radius, length } => meshes.add(Cylinder::new(*radius as f32 * MM, *length as f32 * MM)),
        // shortcut: a 10 mm placeholder cube until POLA-43 draws catalog meshes.
        Shape::Part => meshes.add(Cuboid::new(10.0 * MM, 10.0 * MM, 10.0 * MM)),
    };
    let material = finish_material(&node.finish);
    let base_color = material.base_color;
    let handle = materials.add(material);
    let owner = SelectionTarget::Node(node.id);

    let mut entity = commands.spawn((
        SceneEntity,
        SceneNode(node.id),
        ChildOf(root),
        Mesh3d(mesh),
        MeshMaterial3d(handle.clone()),
        HighlightLook::Standard { material: handle, base_color, owner },
        transform,
    ));
    if node.pickable {
        entity.insert(PickId(owner));
    }
    if matches!(node.finish, Finish::Glass | Finish::Beam { .. }) {
        entity.insert(NotShadowCaster);
    }
    entity.id()
}

fn finish_material(finish: &Finish) -> StandardMaterial {
    match finish {
        Finish::AnodizedBlack => StandardMaterial {
            base_color: Color::srgb_u8(29, 30, 33),
            metallic: 0.3,
            perceptual_roughness: 0.42,
            ..default()
        },
        Finish::Stainless => StandardMaterial {
            base_color: Color::srgb_u8(201, 204, 209),
            metallic: 1.0,
            perceptual_roughness: 0.28,
            ..default()
        },
        Finish::Mirror => StandardMaterial {
            base_color: Color::srgb_u8(240, 242, 245),
            metallic: 1.0,
            perceptual_roughness: 0.05,
            ..default()
        },
        Finish::Glass => StandardMaterial {
            base_color: Color::srgba(0.78, 0.88, 1.0, 0.3),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.06,
            reflectance: 0.6,
            ..default()
        },
        Finish::Painted { rgb } => StandardMaterial {
            base_color: Color::srgb_u8(rgb[0], rgb[1], rgb[2]),
            metallic: 0.1,
            perceptual_roughness: 0.5,
            ..default()
        },
        Finish::Beam { wavelength_nm } => StandardMaterial {
            base_color: beam_color(*wavelength_nm),
            unlit: true,
            ..default()
        },
    }
}

/// False colour for a beam. Visible wavelengths get their approximate hue; IR and UV get
/// distinct saturated colours, because beams must stay visible on screen.
fn beam_color(wavelength_nm: f64) -> Color {
    match wavelength_nm {
        w if w < 380.0 => Color::srgb_u8(191, 90, 242),  // UV: violet
        w if w < 450.0 => Color::srgb_u8(94, 92, 230),   // violet-blue
        w if w < 495.0 => Color::srgb_u8(10, 132, 255),  // blue
        w if w < 570.0 => Color::srgb_u8(48, 209, 88),   // green
        w if w < 590.0 => Color::srgb_u8(255, 214, 10),  // yellow
        w if w < 620.0 => Color::srgb_u8(255, 159, 10),  // orange
        w if w <= 700.0 => Color::srgb_u8(255, 69, 58),  // red
        _ => Color::srgb_u8(255, 55, 95),                // near/far IR: pink-red
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_convert_from_z_up_millimetres() {
        assert_eq!(to_render(DVec3::new(1000.0, 0.0, 0.0)), Vec3::X);
        assert_eq!(to_render(DVec3::new(0.0, 0.0, 1000.0)), Vec3::Y);
        assert_eq!(to_render(DVec3::new(0.0, 1000.0, 0.0)), -Vec3::Z);
    }

    #[test]
    fn rotations_convert_consistently_with_positions() {
        let q = DQuat::from_rotation_z(0.7) * DQuat::from_rotation_y(0.3);
        let v = DVec3::new(10.0, -20.0, 30.0);
        let expected = to_render(q * v);
        let actual = rotation_to_render(q) * to_render(v);
        assert!((expected - actual).length() < 1e-6);
    }

    #[test]
    fn axes_convert_back_and_normals_match_positions() {
        let p = DVec3::new(1234.5, -678.25, 910.0);
        assert!((from_render(to_render(p)) - p).length() < 1e-3);
        for n in [DVec3::X, DVec3::Y, DVec3::Z, DVec3::new(0.3, -0.4, 0.5)] {
            assert!((normal_to_render(n) - to_render(n * 1000.0)).length() < 1e-6);
        }
    }
}
