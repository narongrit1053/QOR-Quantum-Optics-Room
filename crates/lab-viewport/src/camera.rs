//! CAD-style orbit camera.
//!
//! Left-drag orbits, right/middle-drag or Shift+left-drag pans, the wheel zooms toward the
//! cursor. In the top view every drag pans. Dragging moves the camera 1:1 with the pointer;
//! zoom, framing and view changes ease toward a target and request frames until they settle.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::{
    camera::{ScalingMode, primitives::Aabb},
    ecs::system::SystemParam,
    light::ShadowFilteringMethod,
    math::DVec3,
    prelude::*,
};
use lab_core::{Document, SelectionTarget};

use crate::gizmo::GizmoCapture;
use crate::host::{MsaaSamples, PendingRequests, RedrawState, ViewportSize};
use crate::picking::Selection;
use crate::scene::{SceneIndex, SceneNode, to_render};
use crate::{FrameRequest, FrameTarget, Modifiers, PointerButton, ViewMode, ViewportInput};

/// Vertical field of view (≈34°): narrow enough to look calm, wide enough for a room.
const FOV: f32 = 0.6;
const TOP_PITCH: f32 = FRAC_PI_2 - 0.001;
const MIN_PITCH: f32 = -0.15;
const MAX_PITCH: f32 = FRAC_PI_2 - 0.01;
const ORBIT_RADIANS_PER_PX: f32 = 0.006;
const DRAG_THRESHOLD_PX: f32 = 4.0;
const ZOOM_PER_PX: f32 = 0.0025;
/// Exponential easing rate (1/s) for animated camera moves.
const EASE_RATE: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl View {
    fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.focus + self.distance * Vec3::new(cp * sy, sp, cp * cy)
    }

    fn transform(&self) -> Transform {
        Transform::from_translation(self.eye()).looking_at(self.focus, Vec3::Y)
    }

    /// World units per physical pixel at the focus distance.
    fn world_per_px(&self, viewport_height_px: f32) -> f32 {
        2.0 * self.distance * (FOV / 2.0).tan() / viewport_height_px.max(1.0)
    }

    fn settled_at(&self, target: &View) -> bool {
        self.focus.distance(target.focus) < 1e-5
            && (self.yaw - target.yaw).abs() < 1e-4
            && (self.pitch - target.pitch).abs() < 1e-4
            && (self.distance / target.distance - 1.0).abs() < 1e-4
    }
}

#[derive(Component)]
pub(crate) struct OrbitCamera {
    current: View,
    target: View,
    mode: ViewMode,
}

// The getters are read by the wall fade (room.rs, T4c).
#[allow(dead_code)]
impl OrbitCamera {
    /// The point the camera orbits, in render space (current, possibly easing, view).
    pub(crate) fn focus(&self) -> Vec3 {
        self.current.focus
    }

    /// The eye of the current view in render space: the translation `animate_camera` writes to
    /// the camera's `Transform` this frame.
    pub(crate) fn eye(&self) -> Vec3 {
        self.current.eye()
    }

    pub(crate) fn mode(&self) -> ViewMode {
        self.mode
    }
}

#[derive(Clone, Copy)]
struct Press {
    button: PointerButton,
    mods: Modifiers,
    start: Vec2,
    last: Vec2,
    dragging: bool,
    /// The press belongs to the gizmo: no orbit, no click.
    gizmo: bool,
}

#[derive(Resource, Default)]
pub(crate) struct PointerState {
    /// Last known pointer position, `None` when the pointer is outside the viewport.
    pub position: Option<Vec2>,
    press: Option<Press>,
}

impl PointerState {
    pub fn is_dragging(&self) -> bool {
        self.press.is_some_and(|p| p.dragging)
    }
}

/// Left clicks (press and release without dragging) this frame, consumed by picking.
#[derive(Resource, Default)]
pub(crate) struct Clicks(pub Vec<Vec2>);

pub(crate) struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PointerState>()
            .init_resource::<Clicks>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (handle_input, animate_camera).chain());
    }
}

fn spawn_camera(mut commands: Commands, samples: Res<MsaaSamples>) {
    let view = View { focus: Vec3::new(0.0, 0.8, 0.0), yaw: 0.65, pitch: 0.55, distance: 2.6 };
    let msaa = match samples.0 {
        8 => Msaa::Sample8,
        4 => Msaa::Sample4,
        2 => Msaa::Sample2,
        _ => Msaa::Off,
    };
    commands.spawn((
        Camera3d::default(),
        msaa,
        ShadowFilteringMethod::Hardware2x2,
        Projection::Perspective(PerspectiveProjection { fov: FOV, near: 0.01, far: 100.0, ..default() }),
        view.transform(),
        OrbitCamera { current: view, target: view, mode: ViewMode::Perspective },
    ));
}

/// Maps a pointer position (full-resolution physical pixels) into the camera's current render
/// target, which is smaller while frames render at reduced resolution. The camera's size is
/// the one of the frame on screen, so rays match what the user sees.
pub(crate) fn to_camera_space(camera: &Camera, full: &ViewportSize, pos: Vec2) -> Vec2 {
    match camera.logical_viewport_size() {
        Some(size) if full.0.x > 0.0 && full.0.y > 0.0 => pos * size / full.0,
        _ => pos,
    }
}

/// What framing reads: the snapshot (data-driven bounds that work in the frame entities spawn),
/// the selection and the bounds of spawned nodes.
#[derive(SystemParam)]
pub(crate) struct FrameSources<'w, 's> {
    index: Res<'w, SceneIndex>,
    selection: Res<'w, Selection>,
    nodes: Query<'w, 's, (&'static SceneNode, &'static Aabb, &'static GlobalTransform)>,
}

pub(crate) fn handle_input(
    pending: Res<PendingRequests>,
    full: Res<ViewportSize>,
    mut pointer: ResMut<PointerState>,
    mut clicks: ResMut<Clicks>,
    mut capture: ResMut<GizmoCapture>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, &GlobalTransform)>,
    sources: FrameSources,
) {
    clicks.0.clear();
    let Ok((mut orbit, camera, camera_transform)) = cameras.single_mut() else { return };
    // Pointer deltas are in full-resolution pixels, whatever the current render scale.
    let viewport_height = full.0.y;
    // Selection as of each request (picking applies `Select` after this system).
    let mut selected = sources.selection.selected;

    for request in &pending.0 {
        match request {
            FrameRequest::Input(input) => match *input {
                ViewportInput::PointerDown { button, pos, mods } => {
                    pointer.position = Some(pos);
                    pointer.press = Some(Press { button, mods, start: pos, last: pos, dragging: false, gizmo: capture.0 });
                }
                ViewportInput::PointerMove { pos, .. } => {
                    pointer.position = Some(pos);
                    let Some(press) = pointer.press.as_mut() else { continue };
                    if !press.dragging && pos.distance(press.start) > DRAG_THRESHOLD_PX {
                        press.dragging = true;
                    }
                    if press.dragging && !press.gizmo {
                        let delta = pos - press.last;
                        let press = *press;
                        drag(&mut orbit, press, delta, viewport_height);
                    }
                    if let Some(press) = pointer.press.as_mut() {
                        press.last = pos;
                    }
                }
                ViewportInput::PointerUp { button, pos, .. } => {
                    pointer.position = Some(pos);
                    let press = pointer.press.take();
                    if press.is_some_and(|p| p.gizmo) {
                        capture.0 = false;
                    } else if let Some(press) = press
                        && press.button == button
                        && button == PointerButton::Left
                        && !press.dragging
                    {
                        clicks.0.push(pos);
                    }
                }
                ViewportInput::PointerLeave => {
                    pointer.position = None;
                }
                ViewportInput::Scroll { delta, pos, .. } => {
                    pointer.position = Some(pos);
                    zoom_toward(&mut orbit, camera, camera_transform, to_camera_space(camera, &full, pos), delta.y);
                }
            },
            FrameRequest::SetViewMode(mode) => set_mode(&mut orbit, *mode),
            FrameRequest::Frame(target) => {
                if let Some((min, max)) = frame_bounds(*target, selected, &sources) {
                    frame_box(&mut orbit, camera, min, max);
                }
            }
            FrameRequest::Select(target) => selected = *target,
            FrameRequest::LoadDocument(_) => selected = None,
            FrameRequest::ApplyChanges(..) | FrameRequest::SetWallDisplay(_) | FrameRequest::DragPart(_) | FrameRequest::SetTool(_) => {}
        }
    }
}

fn drag(orbit: &mut OrbitCamera, press: Press, delta: Vec2, viewport_height: f32) {
    let pan = press.button != PointerButton::Left || press.mods.shift || orbit.mode == ViewMode::Top;
    let view = &mut orbit.current;
    if pan {
        let transform = view.transform();
        let scale = view.world_per_px(viewport_height);
        view.focus += (-transform.right() * delta.x + transform.up() * delta.y) * scale;
    } else {
        view.yaw -= delta.x * ORBIT_RADIANS_PER_PX;
        view.pitch = (view.pitch + delta.y * ORBIT_RADIANS_PER_PX).clamp(MIN_PITCH, MAX_PITCH);
    }
    // Direct manipulation: no easing while the pointer is down.
    orbit.target = orbit.current;
}

fn zoom_toward(orbit: &mut OrbitCamera, camera: &Camera, camera_transform: &GlobalTransform, pos: Vec2, scroll: f32) {
    let factor = (-scroll * ZOOM_PER_PX).exp();
    let target = &mut orbit.target;
    let new_distance = (target.distance * factor).clamp(0.05, 40.0);
    let factor = new_distance / target.distance;
    // Keep the point under the cursor (on the focal plane) fixed while zooming.
    if let Ok(ray) = camera.viewport_to_world(camera_transform, pos) {
        let normal = camera_transform.forward();
        if let Some(t) = ray.intersect_plane(target.focus, InfinitePlane3d::new(*normal)) {
            let point = ray.get_point(t);
            target.focus += (point - target.focus) * (1.0 - factor);
        }
    }
    target.distance = new_distance;
}

fn set_mode(orbit: &mut OrbitCamera, mode: ViewMode) {
    if orbit.mode == mode {
        return;
    }
    orbit.mode = mode;
    match mode {
        ViewMode::Top => {
            // Turn the shortest way to "north up".
            orbit.target.yaw = (orbit.current.yaw / TAU).round() * TAU;
            orbit.target.pitch = TOP_PITCH;
        }
        ViewMode::Perspective => {
            orbit.target.yaw = orbit.current.yaw + 0.65;
            orbit.target.pitch = 0.55;
        }
    }
}

/// Render-space box of a document-space (mm, +Z up) box.
fn render_box(lo: DVec3, hi: DVec3) -> (Vec3, Vec3) {
    let (a, b) = (to_render(lo), to_render(hi));
    (a.min(b), a.max(b))
}

fn platform_box(doc: &Document, id: lab_core::PlatformId) -> Option<(Vec3, Vec3)> {
    let (lo, hi) = doc.platform(id)?.room_aabb();
    Some(render_box(lo, hi))
}

fn room_box(doc: &Document) -> (Vec3, Vec3) {
    let (lo, hi) = doc.room.bounds_outer();
    render_box(lo.extend(0.0), hi.extend(doc.room.wall_height))
}

/// World-space box of a spawned node (needs its `Aabb`, computed the frame after it spawns).
fn node_box(sources: &FrameSources, id: lab_core::NodeId) -> Option<(Vec3, Vec3)> {
    let (_, aabb, transform) = sources.nodes.iter().find(|(node, ..)| node.0 == id)?;
    Some(aabb_box(aabb, transform))
}

fn aabb_box(aabb: &Aabb, transform: &GlobalTransform) -> (Vec3, Vec3) {
    let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for x in [-1.0f32, 1.0] {
        for y in [-1.0f32, 1.0] {
            for z in [-1.0f32, 1.0] {
                let p = transform.transform_point(c + h * Vec3::new(x, y, z));
                min = min.min(p);
                max = max.max(p);
            }
        }
    }
    (min, max)
}

/// Bounds for `target` in render space; `None` when there is nothing to frame (unknown ids,
/// nothing selected, no document).
fn frame_bounds(target: FrameTarget, selected: Option<SelectionTarget>, sources: &FrameSources) -> Option<(Vec3, Vec3)> {
    let doc = sources.index.snapshot.as_ref()?;
    match target {
        FrameTarget::Platform(id) => platform_box(doc, id),
        FrameTarget::Room => Some(room_box(doc)),
        FrameTarget::Selection => match selected? {
            SelectionTarget::Node(id) => node_box(sources, id),
            SelectionTarget::Platform(id) => platform_box(doc, id),
            SelectionTarget::Room => Some(room_box(doc)),
        },
        FrameTarget::All => {
            if doc.platforms.is_empty() {
                return Some(room_box(doc));
            }
            let boxes = doc
                .platforms
                .iter()
                .filter_map(|p| platform_box(doc, p.id))
                .chain(sources.nodes.iter().map(|(_, aabb, transform)| aabb_box(aabb, transform)));
            boxes.reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
        }
    }
}

/// Eases the camera so the box `min..max` (render space) fits the view.
fn frame_box(orbit: &mut OrbitCamera, camera: &Camera, min: Vec3, max: Vec3) {
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    let aspect = camera.logical_viewport_size().map_or(1.0, |s| s.x / s.y.max(1.0));
    let half_fov = (FOV / 2.0).min(((FOV / 2.0).tan() * aspect).atan());
    let radius = ((max - min).length() / 2.0).max(0.01);
    orbit.target.focus = (min + max) / 2.0;
    orbit.target.distance = (radius / half_fov.sin() * 1.02).max(0.05);
}

pub(crate) fn animate_camera(
    time: Res<Time>,
    mut redraw: ResMut<RedrawState>,
    mut cameras: Query<(&mut OrbitCamera, &mut Transform, &mut Projection)>,
) {
    let Ok((mut orbit, mut transform, mut projection)) = cameras.single_mut() else { return };
    let orbit = &mut *orbit;

    if orbit.current.settled_at(&orbit.target) {
        orbit.current = orbit.target;
    } else {
        // After an idle period the frame delta is huge; cap it so animations still play.
        let dt = time.delta_secs().min(1.0 / 30.0);
        let k = 1.0 - (-dt * EASE_RATE).exp();
        let (c, t) = (&mut orbit.current, &orbit.target);
        c.focus = c.focus.lerp(t.focus, k);
        c.yaw += (t.yaw - c.yaw) * k;
        c.pitch += (t.pitch - c.pitch) * k;
        c.distance = (c.distance.ln() + (t.distance.ln() - c.distance.ln()) * k).exp();
        redraw.keep_alive = true;
        redraw.moving = true;
    }

    let next = orbit.current.transform();
    if *transform != next {
        *transform = next;
    }

    // The top view switches to orthographic once the camera is (almost) straight down, scaled
    // so the focal plane keeps its size.
    let orthographic = orbit.mode == ViewMode::Top && orbit.current.pitch > FRAC_PI_2 - 0.02;
    if orthographic {
        let viewport_height = 2.0 * orbit.current.distance * (FOV / 2.0).tan();
        let up_to_date = matches!(
            &*projection,
            Projection::Orthographic(o)
                if matches!(o.scaling_mode, ScalingMode::FixedVertical { viewport_height: h } if h == viewport_height)
        );
        if !up_to_date {
            *projection = Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical { viewport_height },
                near: -1000.0,
                far: 1000.0,
                ..OrthographicProjection::default_3d()
            });
        }
    } else if !matches!(&*projection, Projection::Perspective(_)) {
        *projection = Projection::Perspective(PerspectiveProjection { fov: FOV, near: 0.01, far: 100.0, ..default() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_orbits_around_focus() {
        let view = View { focus: Vec3::new(1.0, 2.0, 3.0), yaw: 0.0, pitch: 0.0, distance: 5.0 };
        assert!((view.eye() - Vec3::new(1.0, 2.0, 8.0)).length() < 1e-5);
        let top = View { pitch: TOP_PITCH, ..view };
        assert!(top.eye().y > 6.99);
    }

    #[test]
    fn top_view_turns_shortest_way_north() {
        let view = View { focus: Vec3::ZERO, yaw: TAU + 0.4, pitch: 0.5, distance: 2.0 };
        let mut orbit = OrbitCamera { current: view, target: view, mode: ViewMode::Perspective };
        set_mode(&mut orbit, ViewMode::Top);
        assert_eq!(orbit.target.yaw, TAU);
        assert_eq!(orbit.target.pitch, TOP_PITCH);
    }
}
