//! CAD-style orbit camera.
//!
//! Left-drag orbits, right/middle-drag or Shift+left-drag pans, the wheel zooms toward the
//! cursor. In the top view every drag pans. Dragging moves the camera 1:1 with the pointer;
//! zoom, framing and view changes ease toward a target and request frames until they settle.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::{
    camera::{ClearColorConfig, ScalingMode, Viewport, primitives::Aabb},
    ecs::system::SystemParam,
    light::ShadowFilteringMethod,
    math::DVec3,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::{Document, SelectionTarget};

use crate::gizmo::GizmoCapture;
use crate::host::{MsaaSamples, PendingRequests, RedrawState, TargetSize, ViewportSize};
use crate::picking::{PickId, Selection};
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
/// Exponential easing rate (1/s) for wheel zoom.
const EASE_RATE: f32 = 12.0;
/// Duration of view-mode changes and framing (DESIGN §7 Motion), cubic ease-out.
const TRANSITION_SECS: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
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

    /// `k` of the way from `self` to `to`; distance in log space so zoom feels even.
    fn lerp(&self, to: &View, k: f32) -> View {
        View {
            focus: self.focus.lerp(to.focus, k),
            yaw: self.yaw + (to.yaw - self.yaw) * k,
            pitch: self.pitch + (to.pitch - self.pitch) * k,
            distance: (self.distance.ln() + (to.distance.ln() - self.distance.ln()) * k).exp(),
        }
    }
}

/// A timed move from `from` to the (possibly still changing) target.
#[derive(Clone, Copy)]
struct Tween {
    from: View,
    elapsed: f32,
}

#[derive(Component)]
pub(crate) struct OrbitCamera {
    current: View,
    target: View,
    mode: ViewMode,
    tween: Option<Tween>,
    reduce_motion: bool,
}

impl OrbitCamera {
    fn new(view: View) -> Self {
        Self { current: view, target: view, mode: ViewMode::Perspective, tween: None, reduce_motion: false }
    }

    /// Starts a timed transition from the current view to `target`.
    fn start_tween(&mut self) {
        self.tween = Some(Tween { from: self.current, elapsed: 0.0 });
    }

    /// POLA-60: the view the camera is heading to, to restore it later with [`Self::ease_to`].
    pub(crate) fn target_view(&self) -> View {
        self.target
    }

    pub(crate) fn ease_to(&mut self, view: View) {
        self.target = view;
        self.start_tween();
    }
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
    /// The camera of the pane the press started in; the whole drag moves it.
    camera: Entity,
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

    /// Where the current press started (full-resolution px).
    pub fn press_start(&self) -> Option<Vec2> {
        self.press.map(|p| p.start)
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
            .add_systems(Update, (layout_panes, handle_input, animate_camera, top_shadows).chain());
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
        OrbitCamera::new(view),
    ));
    // POLA-51: the Top pane of the split view (right half), inactive outside Split. It renders
    // after the main camera into the same texture and keeps the left half (no clear).
    commands.spawn((
        Camera3d::default(),
        Camera { is_active: false, order: 1, clear_color: ClearColorConfig::None, ..default() },
        msaa,
        ShadowFilteringMethod::Hardware2x2,
        Projection::Perspective(PerspectiveProjection { fov: FOV, near: 0.01, far: 100.0, ..default() }),
        view.transform(),
        OrbitCamera::new(view),
        TopPane,
    ));
}

/// Marks the second camera, the Top pane of [`ViewMode::Split`]. Systems that need "the" camera
/// (wall fade, blueprint, outline width) use the main one: `Without<TopPane>`.
#[derive(Component)]
pub(crate) struct TopPane;

/// Gives each camera its half of the render target in Split (recomputed every update, since the
/// adaptive scaler resizes the target), and the whole target otherwise.
fn layout_panes(size: Res<TargetSize>, mut cameras: Query<(&mut Camera, Has<TopPane>), With<OrbitCamera>>) {
    let split = cameras.iter().any(|(c, pane)| pane && c.is_active);
    let left = (size.0.x / 2).max(1);
    for (mut camera, pane) in &mut cameras {
        let rect = split.then(|| {
            let (x, w) = if pane { (left, size.0.x.saturating_sub(left).max(1)) } else { (0, left) };
            (UVec2::new(x, 0), UVec2::new(w, size.0.y.max(1)))
        });
        let current = camera.viewport.as_ref().map(|v| (v.physical_position, v.physical_size));
        if current != rect {
            camera.viewport = rect.map(|(physical_position, physical_size)| Viewport { physical_position, physical_size, ..default() });
        }
    }
}

/// The active camera whose pane contains `pos` (full-resolution physical px), with `pos` mapped
/// into its render target. Outside every pane (the seam, rounding) the first active camera.
pub(crate) fn camera_at<'a, T>(
    cameras: impl IntoIterator<Item = (T, &'a Camera, &'a GlobalTransform)>,
    full: &ViewportSize,
    pos: Vec2,
) -> Option<(T, &'a Camera, &'a GlobalTransform, Vec2)> {
    let mut first = None;
    for (item, camera, transform) in cameras {
        if !camera.is_active {
            continue;
        }
        let p = to_camera_space(camera, full, pos);
        if camera.logical_viewport_rect().is_some_and(|r| r.contains(p)) {
            return Some((item, camera, transform, p));
        }
        first.get_or_insert((item, camera, transform, p));
    }
    first
}

/// Maps a pointer position (full-resolution physical pixels) into the camera's current render
/// target, which is smaller while frames render at reduced resolution. The camera's size is
/// the one of the frame on screen, so rays match what the user sees. `viewport_to_world` takes
/// target space, so this also holds for a pane of the split view.
pub(crate) fn to_camera_space(camera: &Camera, full: &ViewportSize, pos: Vec2) -> Vec2 {
    match camera.logical_target_size() {
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

type CameraItem = (Entity, &'static mut OrbitCamera, &'static mut Camera, &'static GlobalTransform, Has<TopPane>);

#[allow(clippy::too_many_arguments)] // a Bevy system: every input is its own parameter
pub(crate) fn handle_input(
    pending: Res<PendingRequests>,
    full: Res<ViewportSize>,
    mut pointer: ResMut<PointerState>,
    mut clicks: ResMut<Clicks>,
    mut capture: ResMut<GizmoCapture>,
    mut cameras: Query<CameraItem>,
    sources: FrameSources,
    mut ray_cast: MeshRayCast,
    picks: Query<(), With<PickId>>,
) {
    clicks.0.clear();
    let Some(main) = cameras.iter().find(|c| !c.4).map(|c| c.0) else { return };
    let pane_at = |cameras: &Query<CameraItem>, pos: Vec2| {
        camera_at(cameras.iter().map(|(e, _, c, t, _)| (e, c, t)), &full, pos).map_or(main, |hit| hit.0)
    };
    // Pointer deltas are in full-resolution pixels, whatever the current render scale.
    let viewport_height = full.0.y;
    // Selection as of each request (picking applies `Select` after this system).
    let mut selected = sources.selection.selected;

    for request in &pending.0 {
        match request {
            FrameRequest::Input(input) => match *input {
                ViewportInput::PointerDown { button, pos, mods } => {
                    pointer.position = Some(pos);
                    let camera = pane_at(&cameras, pos);
                    pointer.press = Some(Press { button, mods, start: pos, last: pos, dragging: false, gizmo: capture.0, camera });
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
                        if let Ok((_, mut orbit, ..)) = cameras.get_mut(press.camera) {
                            drag(&mut orbit, press, delta, viewport_height);
                        }
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
                    let entity = pane_at(&cameras, pos);
                    // Zooming in aims at what is under the cursor: the scene, else the floor.
                    let hit = (delta.y > 0.0)
                        .then(|| cameras.get(entity).ok())
                        .flatten()
                        .and_then(|(_, _, camera, camera_transform, _)| camera.viewport_to_world(camera_transform, to_camera_space(camera, &full, pos)).ok())
                        .and_then(|ray| {
                            let filter = |e: Entity| picks.contains(e);
                            let settings = MeshRayCastSettings::default().with_filter(&filter);
                            match ray_cast.cast_ray(ray, &settings).first() {
                                Some((_, h)) => Some(h.point),
                                None => ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y)).map(|t| ray.get_point(t)),
                            }
                        });
                    if let Ok((_, mut orbit, camera, camera_transform, _)) = cameras.get_mut(entity) {
                        zoom_toward(&mut orbit, &camera, camera_transform, to_camera_space(&camera, &full, pos), delta.y, hit);
                    }
                }
            },
            FrameRequest::SetViewMode(mode) => set_view_mode(&mut cameras, *mode),
            FrameRequest::Frame(target) => {
                if let Some((min, max)) = frame_bounds(*target, selected, &sources) {
                    for (_, mut orbit, camera, ..) in &mut cameras {
                        frame_box(&mut orbit, &camera, min, max);
                    }
                }
            }
            FrameRequest::Isolate(id) => {
                let target = id.map_or(FrameTarget::All, FrameTarget::Platform);
                if let Some((min, max)) = frame_bounds(target, selected, &sources) {
                    for (_, mut orbit, camera, ..) in &mut cameras {
                        frame_box(&mut orbit, &camera, min, max);
                    }
                }
            }
            FrameRequest::SetReduceMotion(on) => {
                for (_, mut orbit, ..) in &mut cameras {
                    orbit.reduce_motion = *on;
                }
            }
            FrameRequest::Select(target) => selected = *target,
            FrameRequest::LoadDocument(_) => selected = None,
            FrameRequest::ApplyChanges(..) | FrameRequest::SetWallDisplay(_) | FrameRequest::DragPart(_) | FrameRequest::SetTool(_) | FrameRequest::SetBlueprint(_) | FrameRequest::InspectPart(_) | FrameRequest::InspectMarkers(_) | FrameRequest::RenderThumbnail(..) | FrameRequest::ExportImage(..) => {}
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
    orbit.tween = None;
}

fn zoom_toward(orbit: &mut OrbitCamera, camera: &Camera, camera_transform: &GlobalTransform, pos: Vec2, scroll: f32, hit: Option<Vec3>) {
    let factor = (-scroll * ZOOM_PER_PX).exp();
    let top = orbit.mode == ViewMode::Top;
    let target = &mut orbit.target;
    // Move the focal plane onto the surface under the cursor (eye fixed) so the distance can
    // shrink toward it. Orthographic Top uses the distance as scale, so it keeps its plane.
    if let Some(hit) = hit.filter(|_| factor < 1.0 && !top) {
        let eye = target.eye();
        let forward = (target.focus - eye).normalize_or_zero();
        let depth = (hit - eye).dot(forward);
        if depth > 1e-3 {
            target.focus = eye + forward * depth;
            target.distance = depth;
        }
    }
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

/// 3D and Top drive the main camera at full size; Split keeps it in 3D on the left and turns the
/// Top pane on, which starts at the main view and animates to straight down.
fn set_view_mode(cameras: &mut Query<CameraItem>, mode: ViewMode) {
    let mut main_view = None;
    let mut pane_view = None;
    for (_, orbit, camera, _, pane) in cameras.iter() {
        if !pane {
            main_view = Some((orbit.current, orbit.target));
        } else if camera.is_active {
            pane_view = Some((orbit.current, orbit.target));
        }
    }
    let Some((main_current, main_target)) = main_view else { return };
    for (_, mut orbit, mut camera, _, pane) in cameras.iter_mut() {
        match (pane, mode) {
            (true, ViewMode::Split) => {
                if !camera.is_active {
                    camera.is_active = true;
                    orbit.current = main_current;
                    orbit.target = main_target;
                    orbit.mode = ViewMode::Perspective;
                    // The main view was framed for the full width; the pane gets half of it.
                    let aspect = camera.logical_viewport_size().map_or(1.0, |s| s.x / s.y.max(1.0));
                    orbit.target.distance *= half_fov(aspect).sin() / half_fov(aspect / 2.0).sin();
                }
                set_mode(&mut orbit, ViewMode::Top);
            }
            (true, _) => camera.is_active = false,
            (false, ViewMode::Split) => set_mode(&mut orbit, ViewMode::Perspective),
            (false, ViewMode::Top) if pane_view.is_some() => {
                // Leaving Split for Top: the full view continues from the Top pane.
                let (current, target) = pane_view.unwrap_or((main_current, main_target));
                orbit.current = current;
                orbit.target = target;
                orbit.mode = ViewMode::Top;
                orbit.tween = None;
            }
            (false, mode) => set_mode(&mut orbit, mode),
        }
    }
}

fn set_mode(orbit: &mut OrbitCamera, mode: ViewMode) {
    if orbit.mode == mode {
        return;
    }
    orbit.mode = mode;
    orbit.start_tween();
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
        // Split is per camera: `set_view_mode` gives each camera Perspective or Top.
        ViewMode::Split => {}
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
        // Nothing selected: frame everything.
        FrameTarget::Selection if selected.is_none() => frame_bounds(FrameTarget::All, None, sources),
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
/// The smaller of the vertical and horizontal half field of view for `aspect`.
fn half_fov(aspect: f32) -> f32 {
    (FOV / 2.0).min(((FOV / 2.0).tan() * aspect).atan())
}

pub(crate) fn frame_box(orbit: &mut OrbitCamera, camera: &Camera, min: Vec3, max: Vec3) {
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    let aspect = camera.logical_viewport_size().map_or(1.0, |s| s.x / s.y.max(1.0));
    let radius = ((max - min).length() / 2.0).max(0.01);
    orbit.target.focus = (min + max) / 2.0;
    orbit.target.distance = (radius / half_fov(aspect).sin() * 1.02).max(0.05);
    orbit.start_tween();
}

/// No shadow blobs in the Top projection: the key light casts shadows only while the main camera
/// is not in Top.
pub(crate) fn top_shadows(
    quality: Res<crate::Quality>,
    cameras: Query<&OrbitCamera, Without<TopPane>>,
    mut lights: Query<&mut DirectionalLight, With<crate::scene::KeyLight>>,
) {
    let want = quality.shadows() && cameras.iter().all(|c| c.mode() != ViewMode::Top);
    for mut light in &mut lights {
        if light.shadow_maps_enabled != want {
            light.shadow_maps_enabled = want;
        }
    }
}

pub(crate) fn animate_camera(
    time: Res<Time>,
    mut redraw: ResMut<RedrawState>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, &mut Transform, &mut Projection)>,
) {
    for (mut orbit, camera, transform, projection) in &mut cameras {
        if camera.is_active {
            animate_one(&time, &mut redraw, &mut orbit, transform, projection);
        }
    }
}

fn animate_one(time: &Time, redraw: &mut RedrawState, orbit: &mut OrbitCamera, mut transform: Mut<Transform>, mut projection: Mut<Projection>) {

    // After an idle period the frame delta is huge; cap it so animations still play.
    let dt = time.delta_secs().min(1.0 / 30.0);
    if let Some(tween) = orbit.tween.as_mut() {
        tween.elapsed += dt;
        let t = tween.elapsed / TRANSITION_SECS;
        if orbit.reduce_motion || t >= 1.0 {
            orbit.current = orbit.target;
            orbit.tween = None;
        } else {
            orbit.current = tween.from.lerp(&orbit.target, 1.0 - (1.0 - t).powi(3));
        }
        redraw.keep_alive = true;
        redraw.moving = true;
    } else if orbit.current.settled_at(&orbit.target) {
        orbit.current = orbit.target;
    } else {
        let k = 1.0 - (-dt * EASE_RATE).exp();
        orbit.current = orbit.current.lerp(&orbit.target, k);
        redraw.keep_alive = true;
        redraw.moving = true;
    }

    let next = orbit.current.transform();
    if *transform != next {
        *transform = next;
    }

    // The top view switches to orthographic once the camera is (almost) straight down, scaled
    // so the focal plane keeps its size.
    // Also while a tween leaves Top, so the projection switches at the end of the turn, not on its
    // first frame (POLA-52). Orbiting is clamped below this pitch, so 3D stays perspective.
    let orthographic = (orbit.mode == ViewMode::Top || orbit.tween.is_some()) && orbit.current.pitch > FRAC_PI_2 - 0.02;
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
        let mut orbit = OrbitCamera::new(view);
        set_mode(&mut orbit, ViewMode::Top);
        assert_eq!(orbit.target.yaw, TAU);
        assert_eq!(orbit.target.pitch, TOP_PITCH);
        assert!(orbit.tween.is_some());
    }
}
