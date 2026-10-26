//! M9 F. Floating part box: a second camera on its own render layer draws a copy of one node's
//! meshes (part frame, identity pose) into the box texture. Spawned on `PartBox(Some)`, despawned
//! on `PartBox(None)`, so a closed box costs nothing. Left-drag orbits, the wheel zooms, a click
//! picks the copy and reports `FeaturePoint` in the node's part frame, like the main view.

use bevy::{
    camera::{ClearColorConfig, ManualTextureViewHandle, RenderTarget, primitives::Aabb, visibility::RenderLayers},
    math::DVec3,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
};
use lab_core::part::Frame;
use lab_core::{NodeId, SelectionTarget};

use crate::glb::GlbLod;
use crate::outline::Outline;
use crate::picking::{ACCENT, HighlightLook};
use crate::host::{EventSender, MsaaSamples, PendingRequests, SettleFrames};
use crate::scene::{SceneIndex, from_render, to_render};
use crate::{FrameRequest, PointerButton, SnapGlyph, ViewportEvent, ViewportInput};

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
    /// Meshes under the node when copied: a GLB that finishes loading later changes it.
    copied: usize,
    /// The hover snap glyph on this node (part frame).
    snap: Option<(Frame, SnapGlyph)>,
}

#[derive(Resource, Default)]
struct PartBox(Option<Open>);

/// Gizmos only the box camera sees, drawn on top like `OnTop`.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct BoxGizmos;

pub(crate) struct PartBoxPlugin;

impl Plugin for PartBoxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartBoxActive>()
            .init_resource::<PartBox>()
            .init_gizmo_group::<BoxGizmos>()
            .add_systems(Startup, |mut store: ResMut<GizmoConfigStore>| {
                let config = store.config_mut::<BoxGizmos>().0;
                config.depth_bias = -1.0;
                config.render_layers = RenderLayers::layer(BOX_LAYER);
            })
            .add_systems(Update, (update, draw_snap.after(update)));
    }
}

type Pieces<'w, 's> = Query<
    'w,
    's,
    (
        &'static Mesh3d,
        &'static MeshMaterial3d<StandardMaterial>,
        &'static GlobalTransform,
        Option<&'static Aabb>,
        Option<&'static HighlightLook>,
    ),
    Without<Outline>,
>;

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    mut state: ResMut<PartBox>,
    active: Res<PartBoxActive>,
    (index, events, msaa, mut settle, lods): (Res<SceneIndex>, Res<EventSender>, Res<MsaaSamples>, ResMut<SettleFrames>, Query<&GlbLod>),
    spawned: Query<Entity, With<BoxEntity>>,
    copies: Query<Entity, With<BoxPiece>>,
    sources: Query<(&GlobalTransform, Option<&Children>)>,
    pieces: Pieces,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pickable: Query<(), With<BoxPiece>>,
    mut cameras: Query<(&mut Camera, &GlobalTransform, &mut Transform), With<PartBoxCamera>>,
    mut ray_cast: MeshRayCast,
) {
    let mut hover = None;
    for request in &pending.0 {
        match request {
            FrameRequest::PartBox(spec) => {
                for e in &spawned {
                    commands.entity(e).despawn();
                }
                state.0 = None;
                let Some(spec) = spec else { continue };
                let Some(&root) = index.nodes.get(&spec.node) else { continue };
                let Some((copied, (min, max))) = copy(&mut commands, root, &sources, &lods, &pieces, &mut materials) else { continue };
                let mut orbit = Orbit { focus: Vec3::ZERO, yaw: std::f32::consts::FRAC_PI_4, pitch: 0.6, distance: 1.0 };
                frame(&mut orbit, min, max);
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
                state.0 = Some(Open { node: spec.node, orbit, press: None, copied, snap: None });
            }
            FrameRequest::HighlightSnap(h) => {
                if let Some(open) = &mut state.0 {
                    open.snap = h.filter(|(t, ..)| *t == SelectionTarget::Node(open.node)).map(|(_, f, g)| (f, g));
                }
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
                        } else {
                            hover = Some(pos);
                        }
                    }
                    ViewportInput::PointerUp { button: PointerButton::Left, pos, .. } => {
                        if !matches!(open.press.take(), Some((_, _, false))) {
                            continue;
                        }
                        let Ok((camera, gt, _)) = cameras.single() else { continue };
                        if let Some((point, normal)) = hit(camera, gt, pos, &pickable, &mut ray_cast) {
                            events.send(ViewportEvent::FeaturePoint { target: SelectionTarget::Node(open.node), point, normal, in_box: true });
                        }
                    }
                    ViewportInput::PointerLeave => open.press = None,
                    ViewportInput::Scroll { delta, .. } => open.orbit.distance *= (-delta.y * 0.002).exp(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    // M9 E hover, once per frame at most, as in the main view.
    if let (Some(pos), Some(open), Ok((camera, gt, _))) = (hover, &state.0, cameras.single())
        && let Some((point, normal)) = hit(camera, gt, pos, &pickable, &mut ray_cast)
    {
        events.send(ViewportEvent::FeatureHover { target: SelectionTarget::Node(open.node), point, normal });
    }
    // A GLB that finished loading after the box opened: copy it again.
    if let Some(open) = &mut state.0
        && let Some(&root) = index.nodes.get(&open.node)
        && count(root, &sources, &lods, &pieces) != open.copied
    {
        for e in &copies {
            commands.entity(e).despawn();
        }
        if let Some((copied, (min, max))) = copy(&mut commands, root, &sources, &lods, &pieces, &mut materials) {
            open.copied = copied;
            frame(&mut open.orbit, min, max);
            settle.0 = settle.0.max(2);
        }
    }
    if let Ok((mut camera, _, mut transform)) = cameras.single_mut() {
        camera.is_active = active.0;
        if let Some(open) = &state.0 {
            transform.set_if_neq(open.orbit.transform());
        }
    }
}

/// Point and normal (part frame, mm) of the box copy under `pos`. Any visibility: last frame's
/// view visibility need not include the copy (POLA-158 clicks that picked nothing).
fn hit(camera: &Camera, gt: &GlobalTransform, pos: Vec2, pickable: &Query<(), With<BoxPiece>>, ray_cast: &mut MeshRayCast) -> Option<(DVec3, DVec3)> {
    let ray = camera.viewport_to_world(gt, pos).ok()?;
    let filter = |e: Entity| pickable.contains(e);
    let settings = MeshRayCastSettings::default().with_filter(&filter).with_visibility(RayCastVisibility::Any);
    let (_, hit) = ray_cast.cast_ray(ray, &settings).first()?;
    let n = hit.normal;
    Some((from_render(hit.point), DVec3::new(n.x as f64, -n.z as f64, n.y as f64).normalize_or_zero()))
}

/// Focus and distance that frame the bounds `min..max`; the view direction is kept.
fn frame(orbit: &mut Orbit, mut min: Vec3, mut max: Vec3) {
    if min.x > max.x {
        (min, max) = (Vec3::splat(-0.02), Vec3::splat(0.02));
    }
    let radius = ((max - min).length() / 2.0).max(0.005);
    orbit.focus = (min + max) / 2.0;
    orbit.distance = radius / (FOV / 2.0).sin() * 1.15;
}

fn count(root: Entity, sources: &Query<(&GlobalTransform, Option<&Children>)>, lods: &Query<&GlbLod>, pieces: &Pieces) -> usize {
    descendants(root, sources, lods).into_iter().filter(|e| pieces.contains(*e)).count()
}

/// Copies the node's meshes (part frame, base materials) onto the box layer: count and bounds.
fn copy(
    commands: &mut Commands,
    root: Entity,
    sources: &Query<(&GlobalTransform, Option<&Children>)>,
    lods: &Query<&GlbLod>,
    pieces: &Pieces,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(usize, (Vec3, Vec3))> {
    let (root_gt, _) = sources.get(root).ok()?;
    let to_part = root_gt.affine().inverse();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut copied = 0;
    for e in descendants(root, sources, lods) {
        let Ok((mesh, material, gt, aabb, look)) = pieces.get(e) else { continue };
        copied += 1;
        // Highlights tint a shared material's copy, or a part's own material in place; the box
        // wants the untinted finish.
        let material = match look {
            Some(HighlightLook::Standard { material, base_color, .. }) => match materials.get(material) {
                Some(m) if m.base_color != base_color.with_alpha(m.base_color.alpha()) => {
                    let mut m = m.clone();
                    m.base_color = base_color.with_alpha(m.base_color.alpha());
                    materials.add(m)
                }
                _ => material.clone(),
            },
            _ => material.0.clone(),
        };
        let rel = Transform::from_matrix(Mat4::from(to_part * gt.affine()));
        if let Some(aabb) = aabb {
            for corner in corners(aabb) {
                let p = rel.transform_point(corner);
                (min, max) = (min.min(p), max.max(p));
            }
        }
        commands.spawn((BoxEntity, BoxPiece, Mesh3d(mesh.0.clone()), MeshMaterial3d(material), rel, RenderLayers::layer(BOX_LAYER)));
    }
    Some((copied, (min, max)))
}

/// The hover snap glyph in the box, as `feature_pick` draws it in the main view.
fn draw_snap(state: Res<PartBox>, mut gizmos: Gizmos<BoxGizmos>) {
    let Some(Open { snap: Some((f, glyph)), .. }) = &state.0 else { return };
    let z = f.z.normalize_or(DVec3::Z);
    let u = (f.x - z * f.x.dot(z)).normalize_or(z.any_orthonormal_vector());
    let v = z.cross(u);
    let ring = |r: f64| {
        (0..=24).map(move |i| {
            let a = i as f64 * std::f64::consts::TAU / 24.0;
            f.origin + (u * a.cos() + v * a.sin()) * r
        })
    };
    let points: Vec<DVec3> = match glyph {
        SnapGlyph::Circle => ring(3.0).collect(),
        SnapGlyph::Dot => ring(0.8).collect(),
        SnapGlyph::Square => [u + v, -u + v, -u - v, u - v, u + v].map(|d| f.origin + d * 2.5).to_vec(),
    };
    gizmos.linestrip(points.into_iter().map(to_render), ACCENT);
    gizmos.line(to_render(f.origin), to_render(f.origin + z * 6.0), ACCENT);
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
