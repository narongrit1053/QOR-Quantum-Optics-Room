//! The room: floor, walls, footprints and the wall fade (M1-SPEC §5.6).
//!
//! Walls are mitred bodies with real openings, built by `lab_geom` through [`Room::walls`].
//! Each wall has its own material so it can fade on its own: in [`WallDisplay::Auto`], walls
//! between the camera and the scene fade out, and fade back in once they are out of the way.
//! Fading costs frames only while a wall is moving; settled walls are opaque (or hidden) and
//! the viewport idles again.

use bevy::{light::NotShadowCaster, math::DVec2, math::DVec3, prelude::*};
use lab_core::{Room, SelectionTarget, WallDisplay};

use crate::camera::{OrbitCamera, animate_camera};
use crate::host::{PendingRequests, RedrawState};
use crate::mesh::trimesh_to_bevy;
use crate::picking::{HighlightLook, PickId};
use crate::scene::{Backdrop, SceneEntity, SceneIndex, from_render};
use crate::{FrameRequest, ViewMode};

/// Fade rate (1/s) of the exponential approach; settles in ~250 ms (ln(1/FADE_SNAP)/FADE_RATE).
const FADE_RATE: f32 = 14.0;
/// Alphas closer than this to the target snap to it, settling within 300 ms.
const FADE_SNAP: f32 = 0.03;
/// Maximum delta time (s) per fade step; prevents a long pause from overshooting
/// while allowing the fade to finish in real time rather than frame count.
const MAX_FADE_DT: f32 = 0.25;
/// Hysteresis (mm) around the outer face for the "eye behind the wall" test.
const HYSTERESIS: f64 = 50.0;

const WALL_COLOR: Color = Color::srgb(58.0 / 255.0, 60.0 / 255.0, 66.0 / 255.0);
const FOOTPRINT_COLOR: Color = Color::srgb(70.0 / 255.0, 73.0 / 255.0, 80.0 / 255.0);

/// Inits [`WallFade`]; runs [`update_wall_fade`] in `Update` after the camera has moved.
pub(crate) struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WallFade>().add_systems(Update, update_wall_fade.after(animate_camera));
    }
}

#[derive(Resource, Default)]
pub(crate) struct WallFade {
    pub display: WallDisplay,
    /// False until the fade has run once; the first run sets every wall without animation.
    initialized: bool,
}

#[derive(Component, Clone, Debug)]
pub(crate) struct WallInfo {
    pub inner_start: DVec2,
    pub inner_end: DVec2,
    pub outer_start: DVec2,
    pub outer_end: DVec2,
    pub outward: DVec2,
    pub thickness: f64,
    pub height: f64,
}

impl WallInfo {
    fn new(wall: &lab_geom::WallGeometry, thickness: f64, height: f64) -> Self {
        Self {
            inner_start: wall.inner_start,
            inner_end: wall.inner_end,
            outer_start: wall.outer_start,
            outer_end: wall.outer_end,
            outward: wall.outward_normal,
            thickness,
            height,
        }
    }

    /// The plan quad, in order: inner start, inner end, outer end, outer start.
    fn quad(&self) -> [DVec2; 4] {
        [self.inner_start, self.inner_end, self.outer_end, self.outer_start]
    }
}

/// AssetId<StandardMaterial> is Copy, so Clone + Copy derive is valid.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct WallState {
    pub alpha: f32,
    pub target: f32,
    pub material: AssetId<StandardMaterial>,
}

/// Spawns floor, wall bodies and footprints; returns the entities (stored in SceneIndex.room).
pub(crate) fn spawn_room(
    commands: &mut Commands,
    room: &Room,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<Entity> {
    let mut entities = Vec::new();
    match lab_geom::flat_polygon(&room.outline, 0.0, true, 0.001) {
        Ok(floor) => entities.push(
            commands
                .spawn((
                    SceneEntity,
                    Backdrop,
                    Mesh3d(meshes.add(trimesh_to_bevy(&floor))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb_u8(22, 23, 26),
                        perceptual_roughness: 0.95,
                        reflectance: 0.2,
                        ..default()
                    })),
                    Transform::default(),
                ))
                .id(),
        ),
        Err(error) => warn!("room floor could not be meshed: {error}"),
    }

    let walls = match room.walls() {
        Ok(walls) => walls,
        Err(error) => {
            warn!("room walls could not be built: {error}");
            return entities;
        }
    };
    // One material for every footprint: they never fade and are never highlighted.
    let footprint_material = materials.add(StandardMaterial {
        base_color: FOOTPRINT_COLOR,
        perceptual_roughness: 0.9,
        ..default()
    });
    for wall in &walls {
        let handle = materials.add(StandardMaterial {
            base_color: WALL_COLOR,
            perceptual_roughness: 0.9,
            metallic: 0.0,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        });
        let body = commands
            .spawn((
                SceneEntity,
                Backdrop,
                Mesh3d(meshes.add(trimesh_to_bevy(&wall.body))),
                MeshMaterial3d(handle.clone()),
                NotShadowCaster,
                PickId(SelectionTarget::Room),
                HighlightLook::Standard { material: handle.clone(), base_color: WALL_COLOR, owner: SelectionTarget::Room },
                WallInfo::new(wall, room.wall_thickness, room.wall_height),
                WallState { alpha: 1.0, target: 1.0, material: handle.id() },
                Transform::default(),
                Visibility::Inherited,
            ))
            .id();
        let footprint = commands
            .spawn((
                SceneEntity,
                Backdrop,
                Mesh3d(meshes.add(trimesh_to_bevy(&wall.footprint))),
                MeshMaterial3d(footprint_material.clone()),
                Transform::default(),
            ))
            .id();
        entities.extend([body, footprint]);
    }
    entities
}

/// Pure, unit-tested. Whether `wall` should fade for a camera at `eye` looking at `targets`
/// (all in document millimetres):
///
/// - the eye is past the wall's outer face (`thickness ± 50 mm` hysteresis, so a wall that is
///   faded stays faded until the eye is clearly back), or
/// - a segment from the eye to a target crosses the wall's plan quad (Cyrus–Beck) at a height
///   no greater than the wall's within the clipped interval.
pub(crate) fn wall_occludes(wall: &WallInfo, eye: DVec3, targets: &[DVec3], currently_faded: bool) -> bool {
    let d = (eye.truncate() - wall.inner_start).dot(wall.outward);
    let threshold = wall.thickness + if currently_faded { -HYSTERESIS } else { HYSTERESIS };
    if d > threshold {
        return true;
    }
    let quad = wall.quad();
    targets.iter().any(|&target| match clip_segment(&quad, eye.truncate(), target.truncate()) {
        Some((t0, t1)) => {
            // Height is linear along the segment, so its minimum is at an end of the interval.
            let z = |t: f64| eye.z + (target.z - eye.z) * t;
            z(t0).min(z(t1)) <= wall.height
        }
        None => false,
    })
}

/// Cyrus–Beck: the parameter interval `[t0, t1] ⊆ [0, 1]` of the segment `a → b` inside the
/// convex polygon `poly` (either orientation), or `None` when it misses.
fn clip_segment(poly: &[DVec2], a: DVec2, b: DVec2) -> Option<(f64, f64)> {
    let n = poly.len();
    let area2: f64 = (0..n).map(|i| poly[i].perp_dot(poly[(i + 1) % n])).sum();
    if area2.abs() < 1e-9 {
        return None;
    }
    let s = area2.signum();
    let dir = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let e = q - p;
        // Outward normal of this edge; inside means normal · (x - p) <= 0.
        let normal = s * DVec2::new(e.y, -e.x);
        let num = normal.dot(a - p);
        let den = normal.dot(dir);
        if den.abs() < 1e-12 {
            if num > 0.0 {
                return None; // Parallel to this edge and outside it.
            }
            continue;
        }
        let t = -num / den;
        if den < 0.0 {
            t0 = t0.max(t); // Entering.
        } else {
            t1 = t1.min(t); // Leaving.
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// The look that belongs to an alpha: material alpha mode and colour alpha, and visibility.
fn look_for(alpha: f32) -> (AlphaMode, f32, Visibility) {
    if alpha >= 1.0 {
        (AlphaMode::Opaque, 1.0, Visibility::Inherited)
    } else if alpha > 0.0 {
        (AlphaMode::Blend, alpha, Visibility::Inherited)
    } else {
        (AlphaMode::Opaque, 1.0, Visibility::Hidden)
    }
}

/// One fade step towards `target`; snaps when close.
fn step_alpha(alpha: f32, target: f32, dt: f32) -> f32 {
    let next = alpha + (target - alpha) * (1.0 - (-dt * FADE_RATE).exp());
    if (target - next).abs() < FADE_SNAP { target } else { next }
}

#[allow(clippy::too_many_arguments)]
fn update_wall_fade(
    time: Res<Time>,
    pending: Res<PendingRequests>,
    mut fade: ResMut<WallFade>,
    mut redraw: ResMut<RedrawState>,
    index: Res<SceneIndex>,
    cameras: Query<&OrbitCamera>,
    mut walls: Query<(Entity, &WallInfo, Mut<WallState>, &mut Visibility)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for request in &pending.0 {
        if let FrameRequest::SetWallDisplay(display) = request {
            fade.display = *display;
        }
    }
    let Ok(orbit) = cameras.single() else { return };
    // OrbitCamera::eye() is this frame's pose. GlobalTransform is not: it is propagated in
    // PostUpdate, so here it still holds the previous frame's (stale after a drag).
    let eye = from_render(orbit.eye());
    let mut targets = vec![from_render(orbit.focus())];
    if let Some(doc) = &index.snapshot {
        targets.extend(doc.platforms.iter().map(|p| p.position.extend(p.top_height)));
    }
    let dt = time.delta_secs().min(MAX_FADE_DT);
    let first_run = !fade.initialized;
    fade.initialized = true;

    let mut unsettled = false;
    for (entity, info, mut state, mut visibility) in &mut walls {
        let target = match fade.display {
            WallDisplay::Always => 1.0,
            WallDisplay::Hidden => 0.0,
            WallDisplay::Auto if orbit.mode() == ViewMode::Top => 1.0,
            WallDisplay::Auto => {
                if wall_occludes(info, eye, &targets, state.target < 1.0) { 0.0 } else { 1.0 }
            }
        };
        // A freshly spawned wall takes its state at once: rebuilding the room must not fade.
        let alpha = if first_run || state.is_added() { target } else { step_alpha(state.alpha, target, dt) };
        if state.target != target {
            debug!("wall {entity}: fading to {target}");
        }
        if state.alpha != alpha || state.target != target {
            state.alpha = alpha;
            state.target = target;
        }
        unsettled |= alpha != target;

        let (mode, color_alpha, want_visibility) = look_for(alpha);
        if *visibility != want_visibility {
            *visibility = want_visibility;
        }
        if want_visibility == Visibility::Hidden {
            continue; // The material is not drawn; leave it as it is until the wall returns.
        }
        let Some(current) = materials.get(state.material) else { continue };
        if (current.alpha_mode != mode || current.base_color.alpha() != color_alpha)
            && let Some(mut material) = materials.get_mut(state.material)
        {
            material.alpha_mode = mode;
            material.base_color = material.base_color.with_alpha(color_alpha);
        }
    }
    if unsettled {
        redraw.keep_alive = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walls_of(outline: &[(f64, f64)]) -> Vec<WallInfo> {
        let room = Room {
            outline: outline.iter().map(|&(x, y)| DVec2::new(x, y)).collect(),
            ..Room::default()
        };
        room.walls().unwrap().iter().map(|w| WallInfo::new(w, room.wall_thickness, room.wall_height)).collect()
    }

    /// The default 6000 × 5000 room; wall 0 is the south wall (y = -2500, outward -Y).
    fn rectangle() -> Vec<WallInfo> {
        walls_of(&[(-3000.0, -2500.0), (3000.0, -2500.0), (3000.0, 2500.0), (-3000.0, 2500.0)])
    }

    /// The judges' L room: 6000 × 5000 with a 2000 × 1667 notch at +X+Y. Wall 2 runs from
    /// (3000, 833) to (1000, 833), wall 3 (the reflex wall) from (1000, 833) to (1000, 2500).
    fn l_room() -> Vec<WallInfo> {
        walls_of(&[
            (-3000.0, -2500.0),
            (3000.0, -2500.0),
            (3000.0, 833.0),
            (1000.0, 833.0),
            (1000.0, 2500.0),
            (-3000.0, 2500.0),
        ])
    }

    fn occluding(walls: &[WallInfo], eye: DVec3, targets: &[DVec3]) -> Vec<usize> {
        walls.iter().enumerate().filter(|(_, w)| wall_occludes(w, eye, targets, false)).map(|(i, _)| i).collect()
    }

    #[test]
    fn eye_outside_fades_only_the_wall_in_front() {
        let walls = rectangle();
        let eye = DVec3::new(0.0, -8000.0, 2000.0);
        assert_eq!(occluding(&walls, eye, &[DVec3::new(0.0, 0.0, 800.0)]), vec![0]);
        // The eye test alone, without targets.
        assert_eq!(occluding(&walls, eye, &[]), vec![0]);
    }

    #[test]
    fn eye_inside_fades_nothing() {
        let walls = rectangle();
        let eye = DVec3::new(-1000.0, -1500.0, 1600.0);
        let targets = [DVec3::new(0.0, 0.0, 800.0), DVec3::new(2000.0, 1500.0, 900.0)];
        assert!(occluding(&walls, eye, &targets).is_empty());
    }

    #[test]
    fn hysteresis_band_keeps_the_current_state() {
        let south = &rectangle()[0];
        // d = 200 mm past the inner face, thickness 150: inside the band 100..200.
        let eye = DVec3::new(0.0, -2700.0, 1500.0);
        assert!(!wall_occludes(south, eye, &[], false));
        assert!(wall_occludes(south, eye, &[], true));
        // Clearly past the band on either side.
        assert!(wall_occludes(south, DVec3::new(0.0, -2710.0, 1500.0), &[], false));
        assert!(!wall_occludes(south, DVec3::new(0.0, -2590.0, 1500.0), &[], true));
    }

    #[test]
    fn l_room_reflex_wall_occludes() {
        let walls = l_room();
        let eye = DVec3::new(8000.0, -2000.0, 3000.0);
        let target = DVec3::new(500.0, 2000.0, 800.0);
        assert!(wall_occludes(&walls[3], eye, &[target], false));
        // The segment test on its own: the eye is not past wall 3's outer face here.
        let reflex = WallInfo { outward: -walls[3].outward, ..walls[3].clone() };
        assert!((eye.truncate() - reflex.inner_start).dot(reflex.outward) < 0.0);
        assert!(wall_occludes(&reflex, eye, &[target], false));
        assert!(!wall_occludes(&reflex, eye, &[], false));
    }

    #[test]
    fn segment_over_the_wall_top_does_not_occlude() {
        let walls = l_room();
        // Eye inside the room, behind wall 2 (outward +Y); the segment leaves through wall 2
        // into the notch.
        let target = DVec3::new(500.0, 2000.0, 800.0);
        let low = DVec3::new(2500.0, 0.0, 3500.0);
        assert!(wall_occludes(&walls[2], low, &[target], false));
        let high = DVec3::new(2500.0, 0.0, 20000.0);
        assert!(!wall_occludes(&walls[2], high, &[target], false));
    }

    #[test]
    fn cyrus_beck_clips_to_the_quad() {
        let square = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0), DVec2::new(0.0, 10.0)];
        let (t0, t1) = clip_segment(&square, DVec2::new(-10.0, 5.0), DVec2::new(30.0, 5.0)).unwrap();
        assert!((t0 - 0.25).abs() < 1e-12 && (t1 - 0.5).abs() < 1e-12);
        // Clockwise order gives the same result.
        let mut cw = square;
        cw.reverse();
        assert_eq!(clip_segment(&cw, DVec2::new(-10.0, 5.0), DVec2::new(30.0, 5.0)), Some((t0, t1)));
        assert_eq!(clip_segment(&square, DVec2::new(-10.0, 15.0), DVec2::new(30.0, 15.0)), None);
        // Parallel to an edge, outside it.
        assert_eq!(clip_segment(&square, DVec2::new(-1.0, -5.0), DVec2::new(5.0, -5.0)), None);
        // Ends before reaching the quad.
        assert_eq!(clip_segment(&square, DVec2::new(-10.0, 5.0), DVec2::new(-1.0, 5.0)), None);
    }

    #[test]
    fn fade_steps_settle_and_snap() {
        let mut alpha = 1.0;
        let mut frames = 0;
        while alpha != 0.0 {
            alpha = step_alpha(alpha, 0.0, 1.0 / 60.0);
            frames += 1;
            assert!(frames < 100);
        }
        // About a quarter of a second at 60 fps (16 frames at rate 14, snap 0.03).
        assert!((15..=25).contains(&frames), "{frames} frames");
        assert_eq!(look_for(1.0), (AlphaMode::Opaque, 1.0, Visibility::Inherited));
        assert_eq!(look_for(0.4), (AlphaMode::Blend, 0.4, Visibility::Inherited));
        assert_eq!(look_for(0.0).2, Visibility::Hidden);
    }

    #[test]
    fn fade_settles_within_spec_at_various_frame_times() {
        for dt_ms in [16.0, 65.0, 130.0] {
            let dt = (dt_ms / 1000.0) as f32;
            for (start, target) in [(1.0, 0.0), (0.0, 1.0)] {
                let mut alpha = start;
                let mut elapsed = 0.0f32;
                let mut steps = 0;
                while alpha != target {
                    let next = step_alpha(alpha, target, dt.min(MAX_FADE_DT));
                    if start > target {
                        assert!(next <= alpha && next >= target, "overshot during fade out: {alpha} -> {next}");
                    } else {
                        assert!(next >= alpha && next <= target, "overshot during fade in: {alpha} -> {next}");
                    }
                    alpha = next;
                    elapsed += dt;
                    steps += 1;
                    assert!(steps < 100, "did not settle: dt {dt_ms} ms, alpha {alpha}");
                }
                assert!(
                    elapsed <= 0.30,
                    "fade from {start} to {target} took {elapsed:.3} s with dt={dt_ms} ms (expected <= 0.30 s)"
                );
            }
        }
    }
}
