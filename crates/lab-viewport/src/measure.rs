//! The measure tool (M3-PLAN POLA-53): two clicks pick two points, snapped in screen space to
//! part origins, then platform holes, then the scene under the pointer, then the floor plane.
//! The distance goes to the app as [`ViewportEvent::Measured`]; the line is drawn with gizmos.

use bevy::{
    math::{DQuat, DVec3},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::{Document, NodeId, PlatformId, SelectionTarget, compute_holes};

use crate::camera::{Clicks, OrbitCamera, PointerState, camera_at, handle_input};
use crate::host::{EventSender, PendingRequests, ViewportSize};
use crate::picking::{PickId, pick};
use crate::scene::{SceneIndex, from_render, to_render};
use crate::{FrameRequest, Measurement, Tool, ViewportEvent, ViewportInput};

/// Snap radius in physical px.
const SNAP_PX: f32 = 12.0;
const ACCENT: Color = Color::srgb(10.0 / 255.0, 132.0 / 255.0, 1.0);

#[derive(Resource, Default)]
struct Measure {
    active: bool,
    a: Option<DVec3>,
    b: Option<DVec3>,
    /// The snapped point under the pointer (the open end while `b` is not picked).
    hover: Option<DVec3>,
}

pub(crate) struct MeasurePlugin;

impl Plugin for MeasurePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Measure>()
            .add_systems(Update, (measure_input.after(handle_input).before(pick), draw_measure.after(measure_input)));
    }
}

/// A node's origin in the room frame (mm).
fn node_room_pos(doc: &Document, id: NodeId) -> Option<DVec3> {
    let (pos, _) = doc.node_pose(id)?;
    Some(match doc.root_platform(id).and_then(|p| doc.platform_pose(p)) {
        Some((origin, angle)) => origin + DQuat::from_rotation_z(angle) * pos,
        None => pos,
    })
}

/// The nearest hole of platform `pid` to the room point `point`, in the room frame (on the top
/// surface, where parts seat).
fn hole_near(doc: &Document, pid: PlatformId, point: DVec3) -> Option<DVec3> {
    let (origin, angle) = doc.platform_pose(pid)?;
    let holes = compute_holes(doc.platform(pid)?).ok()?;
    let local = DQuat::from_rotation_z(-angle) * (point - origin);
    let hit = lab_mates::snap(&holes, local.truncate(), holes.pitch.max_element())?;
    Some(origin + DQuat::from_rotation_z(angle) * hit.position.extend(0.0))
}

#[allow(clippy::too_many_arguments)]
fn measure_input(
    pending: Res<PendingRequests>,
    pointer: Res<PointerState>,
    mut clicks: ResMut<Clicks>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    targets: Query<&PickId>,
    mut ray_cast: MeshRayCast,
    mut measure: ResMut<Measure>,
    (index, events, full): (Res<SceneIndex>, Res<EventSender>, Res<ViewportSize>),
) {
    let before = (measure.a, measure.b);
    let mut moved = false;
    for request in &pending.0 {
        match request {
            FrameRequest::SetTool(tool) => {
                *measure = Measure { active: *tool == Tool::Measure, ..default() };
                events.send(ViewportEvent::Measured(None));
            }
            FrameRequest::LoadDocument(_) => {
                let active = measure.active;
                *measure = Measure { active, ..default() };
            }
            FrameRequest::Input(ViewportInput::PointerMove { .. } | ViewportInput::PointerLeave) => moved = true,
            _ => {}
        }
    }
    if !measure.active {
        return;
    }
    let Some(doc) = &index.snapshot else { return };

    let snapped = |pos: Vec2, ray_cast: &mut MeshRayCast| -> Option<DVec3> {
        let ((), camera, camera_transform, p) = camera_at(cameras.iter().map(|(c, t)| ((), c, t)), &full, pos)?;
        let to_screen = |p: DVec3| {
            let v = camera.world_to_viewport(camera_transform, to_render(p)).ok()?;
            let size = camera.logical_target_size()?;
            Some(if full.0.x > 0.0 && full.0.y > 0.0 { v * full.0 / size } else { v })
        };
        let near = |p: DVec3| to_screen(p).map(|s| s.distance(pos)).filter(|&d| d <= SNAP_PX);
        // Part origins first.
        let origin = doc
            .nodes
            .iter()
            .filter_map(|n| node_room_pos(doc, n.id))
            .filter_map(|p| near(p).map(|d| (d, p)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, p)) = origin {
            return Some(p);
        }
        let ray = camera.viewport_to_world(camera_transform, p).ok()?;
        let filter = |entity: Entity| targets.contains(entity);
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        match ray_cast.cast_ray(ray, &settings).first().map(|(e, h)| (targets.get(*e).ok(), from_render(h.point))) {
            Some((Some(PickId(SelectionTarget::Platform(pid))), point)) => {
                Some(hole_near(doc, *pid, point).filter(|&h| near(h).is_some()).unwrap_or(point))
            }
            Some((_, point)) => Some(point),
            None => ray
                .intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
                .map(|t| from_render(ray.get_point(t))),
        }
    };

    // Clicks pick points instead of selecting.
    for click in std::mem::take(&mut clicks.0) {
        let Some(p) = snapped(click, &mut ray_cast) else { continue };
        match (measure.a, measure.b) {
            (Some(_), None) => measure.b = Some(p),
            _ => (measure.a, measure.b) = (Some(p), None),
        }
    }
    if moved {
        measure.hover = match pointer.position {
            Some(pos) if !pointer.is_dragging() => snapped(pos, &mut ray_cast),
            _ => None,
        };
    }
    if (measure.a, measure.b) != before {
        events.send(ViewportEvent::Measured(measure.a.map(|a| Measurement { a, b: measure.b })));
    }
}

fn draw_measure(measure: Res<Measure>, mut gizmos: Gizmos) {
    if !measure.active {
        return;
    }
    let dot = |gizmos: &mut Gizmos, p: DVec3| {
        gizmos.sphere(Isometry3d::from_translation(to_render(p)), 0.003, ACCENT);
    };
    if let Some(a) = measure.a {
        dot(&mut gizmos, a);
        if let Some(b) = measure.b.or(measure.hover) {
            gizmos.line(to_render(a), to_render(b), ACCENT);
            dot(&mut gizmos, b);
        }
    } else if let Some(h) = measure.hover {
        dot(&mut gizmos, h);
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::DVec2;

    use super::*;

    /// Two holes 4 pitches apart on the sample M6 table are 100 mm apart.
    #[test]
    fn holes_four_pitches_apart() {
        let doc = lab_core::sample::demo_document();
        let pid = doc.platforms[0].id;
        let holes = compute_holes(doc.platform(pid).unwrap()).unwrap();
        let (origin, angle) = doc.platform_pose(pid).unwrap();
        let room = |local: DVec2| origin + DQuat::from_rotation_z(angle) * local.extend(0.0);
        let first = holes.position(holes.nearest(DVec2::ZERO).unwrap());
        let a = hole_near(&doc, pid, room(first + DVec2::new(3.0, 2.0))).unwrap();
        let b = hole_near(&doc, pid, room(first + DVec2::new(4.0 * holes.pitch.x - 2.0, -3.0))).unwrap();
        assert!((a.distance(b) - 4.0 * holes.pitch.x).abs() < 1e-9, "{a} {b}");
        assert!((holes.pitch.x - 25.0).abs() < 1e-9);
    }
}
