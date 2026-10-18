//! CAD-style orbit camera.
//!
//! Left-drag orbits, right/middle-drag or Shift+left-drag pans, the wheel zooms toward the
//! cursor. In the top view every drag pans. Dragging moves the camera 1:1 with the pointer;
//! zoom, framing and view changes ease toward a target and request frames until they settle.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::{
    camera::{ScalingMode, primitives::Aabb},
    light::ShadowFilteringMethod,
    prelude::*,
};

use crate::host::{MsaaSamples, PendingRequests, RedrawState, ViewportSize};
use crate::scene::{Backdrop, SceneEntity};
use crate::{FrameRequest, Modifiers, PointerButton, ViewMode, ViewportInput};

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

#[derive(Clone, Copy)]
struct Press {
    button: PointerButton,
    mods: Modifiers,
    start: Vec2,
    last: Vec2,
    dragging: bool,
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

pub(crate) fn handle_input(
    pending: Res<PendingRequests>,
    full: Res<ViewportSize>,
    mut pointer: ResMut<PointerState>,
    mut clicks: ResMut<Clicks>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, &GlobalTransform)>,
    bounds: Query<(&Aabb, &GlobalTransform), (With<SceneEntity>, Without<Backdrop>)>,
) {
    clicks.0.clear();
    let Ok((mut orbit, camera, camera_transform)) = cameras.single_mut() else { return };
    // Pointer deltas are in full-resolution pixels, whatever the current render scale.
    let viewport_height = full.0.y;

    for request in &pending.0 {
        match request {
            FrameRequest::Input(input) => match *input {
                ViewportInput::PointerDown { button, pos, mods } => {
                    pointer.position = Some(pos);
                    pointer.press = Some(Press { button, mods, start: pos, last: pos, dragging: false });
                }
                ViewportInput::PointerMove { pos, .. } => {
                    pointer.position = Some(pos);
                    let Some(press) = pointer.press.as_mut() else { continue };
                    if !press.dragging && pos.distance(press.start) > DRAG_THRESHOLD_PX {
                        press.dragging = true;
                    }
                    if press.dragging {
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
                    if let Some(press) = pointer.press.take()
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
            FrameRequest::FrameAll => frame_all(&mut orbit, camera, &bounds),
            FrameRequest::Select(_) | FrameRequest::LoadDocument(_) => {}
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

fn frame_all(orbit: &mut OrbitCamera, camera: &Camera, bounds: &Query<(&Aabb, &GlobalTransform), (With<SceneEntity>, Without<Backdrop>)>) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (aabb, transform) in bounds {
        let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
        for corner in [-1.0f32, 1.0].into_iter().flat_map(|x| [-1.0f32, 1.0].into_iter().flat_map(move |y| [-1.0f32, 1.0].map(|z| Vec3::new(x, y, z)))) {
            let p = transform.transform_point(c + h * corner);
            min = min.min(p);
            max = max.max(p);
        }
    }
    if !min.is_finite() {
        return;
    }
    let aspect = camera.logical_viewport_size().map_or(1.0, |s| s.x / s.y.max(1.0));
    let half_fov = (FOV / 2.0).min(((FOV / 2.0).tan() * aspect).atan());
    let radius = (max - min).length() / 2.0;
    orbit.target.focus = (min + max) / 2.0;
    orbit.target.distance = radius / half_fov.sin() * 1.02;
}

fn animate_camera(
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
