//! M9 F. Floating part box: a second camera on its own render layer draws a copy of one node's
//! meshes (part frame, identity pose) into the box texture. Spawned on `PartBox(Some)`, despawned
//! on `PartBox(None)`, so a closed box costs nothing. Left-drag orbits, the wheel zooms, a click
//! picks the copy and reports `FeaturePoint` in the node's part frame, like the main view.

use bevy::{
    camera::{ClearColorConfig, ManualTextureViewHandle, RenderTarget, primitives::Aabb, visibility::RenderLayers},
    math::DVec3,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::{NodeId, SelectionTarget};

use crate::glb::GlbLod;
use crate::host::{EventSender, MsaaSamples, PendingRequests, SettleFrames};
use crate::scene::{SceneIndex, from_render};
use crate::{FrameRequest, PointerButton, ViewportEvent, ViewportInput};

const BOX_LAYER: usize = 3;
/// The box texture's view handle; the main frame views count up from 0.
pub(crate) const BOX_VIEW: ManualTextureViewHandle = ManualTextureViewHandle(u32::MAX);
const FOV: f32 = 0.6;
const ORBIT_RADIANS_PER_PX: f32 = 0.008;
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// The box camera; the host leaves its render target alone.
#[derive(Component)]
pub(crate) struct PartBoxCamera;

/// Camera, light and mesh copies of the open box.
#[derive(Component)]
struct BoxEntity;

#[derive(Component)]
struct BoxPiece;

/// Set by the host: a box texture is attached this frame (the camera renders only then).
#[derive(Resource, Default)]
pub(crate) struct PartBoxActive(pub bool);

struct Orbit {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Orbit {
    fn transform(&self) -> Transform {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Transform::from_translation(self.focus + self.distance * Vec3::new(cp * sy, sp, cp * cy)).looking_at(self.focus, Vec3::Y)
    }
}

struct Open {
    node: NodeId,
    orbit: Orbit,
    /// Press position, last position, dragging.
    press: Option<(Vec2, Vec2, bool)>,
}

#[derive(Resource, Default)]
struct PartBox(Option<Open>);

pub(crate) struct PartBoxPlugin;

impl Plugin for PartBoxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartBoxActive>().init_resource::<PartBox>().add_systems(Update, update);
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    mut state: ResMut<PartBox>,
    active: Res<PartBoxActive>,
    (index, events, msaa, mut settle, lods): (Res<SceneIndex>, Res<EventSender>, Res<MsaaSamples>, ResMut<SettleFrames>, Query<&GlbLod>),
    spawned: Query<Entity, With<BoxEntity>>,
    sources: Query<(&GlobalTransform, Option<&Children>)>,
    pieces: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &GlobalTransform, Option<&Aabb>)>,
    pickable: Query<(), With<BoxPiece>>,
    mut cameras: Query<(&mut Camera, &GlobalTransform, &mut Transform), With<PartBoxCamera>>,
    mut ray_cast: MeshRayCast,
) {
    for request in &pending.0 {
        match request {
            FrameRequest::PartBox(spec) => {
                for e in &spawned {
                    commands.entity(e).despawn();
                }
                state.0 = None;
                let Some(spec) = spec else { continue };
                let Some(&root) = index.nodes.get(&spec.node) else { continue };
                let Ok((root_gt, _)) = sources.get(root) else { continue };
                let to_part = root_gt.affine().inverse();
                let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                let descendants: Vec<Entity> = descendants(root, &sources, &lods);
                for e in descendants {
                    let Ok((mesh, material, gt, aabb)) = pieces.get(e) else { continue };
                    let rel = Transform::from_matrix(Mat4::from(to_part * gt.affine()));
                    if let Some(aabb) = aabb {
                        for corner in corners(aabb) {
                            let p = rel.transform_point(corner);
                            (min, max) = (min.min(p), max.max(p));
                        }
                    }
                    commands.spawn((
                        BoxEntity,
                        BoxPiece,
                        Mesh3d(mesh.0.clone()),
                        MeshMaterial3d(material.0.clone()),
                        rel,
                        RenderLayers::layer(BOX_LAYER),
                    ));
                }
                if min.x > max.x {
                    (min, max) = (Vec3::splat(-0.02), Vec3::splat(0.02));
                }
                let radius = ((max - min).length() / 2.0).max(0.005);
                let orbit = Orbit {
                    focus: (min + max) / 2.0,
                    yaw: std::f32::consts::FRAC_PI_4,
                    pitch: 0.6,
                    distance: radius / (FOV / 2.0).sin() * 1.15,
                };
                commands.spawn((
                    BoxEntity,
                    PartBoxCamera,
                    Camera3d::default(),
                    Camera {
                        is_active: false,
                        order: 3,
                        clear_color: ClearColorConfig::Custom(Color::srgb(0.16, 0.17, 0.19)),
                        ..default()
                    },
                    match msaa.0 { 8 => Msaa::Sample8, 4 => Msaa::Sample4, 2 => Msaa::Sample2, _ => Msaa::Off },
                    RenderTarget::TextureView(BOX_VIEW),
                    Projection::Perspective(PerspectiveProjection { fov: FOV, near: 0.0005, far: 10.0, ..default() }),
                    orbit.transform(),
                    RenderLayers::layer(BOX_LAYER),
                ));
                commands.spawn((
                    BoxEntity,
                    DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: false, ..default() },
                    Transform::from_xyz(2.5, 6.0, 3.5).looking_at(Vec3::ZERO, Vec3::Y),
                    RenderLayers::layer(BOX_LAYER),
                ));
                commands.spawn((
                    BoxEntity,
                    DirectionalLight { illuminance: 3_000.0, color: Color::srgb(0.8, 0.88, 1.0), shadow_maps_enabled: false, ..default() },
                    Transform::from_xyz(-3.0, 2.0, -2.0).looking_at(Vec3::ZERO, Vec3::Y),
                    RenderLayers::layer(BOX_LAYER),
                ));
                // New meshes and materials need a frame or two before they show.
                settle.0 = settle.0.max(2);
                state.0 = Some(Open { node: spec.node, orbit, press: None });
            }
            FrameRequest::PartBoxInput(input) => {
                let Some(open) = &mut state.0 else { continue };
                match *input {
                    ViewportInput::PointerDown { button: PointerButton::Left, pos, .. } => open.press = Some((pos, pos, false)),
                    ViewportInput::PointerMove { pos, .. } => {
                        if let Some((start, last, dragging)) = &mut open.press {
                            *dragging |= pos.distance(*start) > DRAG_THRESHOLD_PX;
                            if *dragging {
                                let d = pos - *last;
                                open.orbit.yaw -= d.x * ORBIT_RADIANS_PER_PX;
                                open.orbit.pitch = (open.orbit.pitch + d.y * ORBIT_RADIANS_PER_PX).clamp(-1.55, 1.55);
                            }
                            *last = pos;
                        }
                    }
                    ViewportInput::PointerUp { button: PointerButton::Left, pos, .. } => {
                        if !matches!(open.press.take(), Some((_, _, false))) {
                            continue;
                        }
                        let Ok((camera, gt, _)) = cameras.single() else { continue };
                        let Ok(ray) = camera.viewport_to_world(gt, pos) else { continue };
                        let filter = |e: Entity| pickable.contains(e);
                        let settings = MeshRayCastSettings::default().with_filter(&filter);
                        let Some((_, hit)) = ray_cast.cast_ray(ray, &settings).first() else { continue };
                        let n = hit.normal;
                        events.send(ViewportEvent::FeaturePoint {
                            target: SelectionTarget::Node(open.node),
                            point: from_render(hit.point),
                            normal: DVec3::new(n.x as f64, -n.z as f64, n.y as f64).normalize_or_zero(),
                        });
                    }
                    ViewportInput::PointerLeave => open.press = None,
                    ViewportInput::Scroll { delta, .. } => open.orbit.distance *= (-delta.y * 0.002).exp(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Ok((mut camera, _, mut transform)) = cameras.single_mut() {
        camera.is_active = active.0;
        if let Some(open) = &state.0 {
            transform.set_if_neq(open.orbit.transform());
        }
    }
}

/// Skips the coarse GLB LODs: without their `VisibilityRange` they would draw over LOD0 (POLA-158).
fn descendants(root: Entity, sources: &Query<(&GlobalTransform, Option<&Children>)>, lods: &Query<&GlbLod>) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if lods.get(e).is_ok_and(|lod| !lod.primary) {
            continue;
        }
        out.push(e);
        if let Ok((_, Some(children))) = sources.get(e) {
            stack.extend(children.iter());
        }
    }
    out
}

fn corners(aabb: &Aabb) -> [Vec3; 8] {
    let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
    std::array::from_fn(|i| c + h * Vec3::new([-1.0, 1.0][i & 1], [-1.0, 1.0][(i >> 1) & 1], [-1.0, 1.0][(i >> 2) & 1]))
}
