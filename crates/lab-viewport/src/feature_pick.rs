//! Feature-pick mode of the constrain tool (M8-PLAN D): clicks report the hit point and normal
//! in the hit target's own frame as [`ViewportEvent::FeaturePoint`]; the app resolves the feature
//! and sends it back with `HighlightFeature`, drawn on top with gizmos.

use bevy::{
    math::{DQuat, DVec3},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::part::Feature;
use lab_core::{Document, SelectionTarget};

use crate::camera::{Clicks, OrbitCamera, camera_at, handle_input};
use crate::gizmo::OnTop;
use crate::host::{EventSender, PendingRequests, ViewportSize};
use crate::picking::{ACCENT, PickId, pick};
use crate::scene::{SceneIndex, from_render, to_render};
use crate::{FrameRequest, Tool, ViewportEvent};

#[derive(Resource, Default)]
struct FeaturePick {
    active: bool,
    highlight: Option<(SelectionTarget, Feature)>,
}

pub(crate) struct FeaturePickPlugin;

impl Plugin for FeaturePickPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FeaturePick>()
            .add_systems(Update, (feature_input.after(handle_input).before(pick), draw_highlight.after(feature_input)));
    }
}

/// Room-frame pose of a node's part frame or a platform's frame (origin on its top surface).
fn target_pose(doc: &Document, target: SelectionTarget) -> Option<(DVec3, DQuat)> {
    let platform = |p| doc.platform_pose(p).map(|(o, a)| (o, DQuat::from_rotation_z(a)));
    match target {
        SelectionTarget::Platform(p) => platform(p),
        SelectionTarget::Node(id) => {
            let (lp, lq) = doc.node_pose(id)?;
            let (rp, rq) = doc.root_platform(id).and_then(platform).unwrap_or((DVec3::ZERO, DQuat::IDENTITY));
            Some((rp + rq * lp, rq * lq))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn feature_input(
    pending: Res<PendingRequests>,
    mut clicks: ResMut<Clicks>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    targets: Query<&PickId>,
    mut ray_cast: MeshRayCast,
    mut state: ResMut<FeaturePick>,
    (index, events, full): (Res<SceneIndex>, Res<EventSender>, Res<ViewportSize>),
) {
    for request in &pending.0 {
        match request {
            FrameRequest::SetTool(tool) => {
                state.active = *tool == Tool::Constrain;
                state.highlight = None;
            }
            FrameRequest::HighlightFeature(h) => state.highlight = h.clone(),
            _ => {}
        }
    }
    if !state.active {
        return;
    }
    let Some(doc) = &index.snapshot else { return };
    // Clicks pick features instead of selecting.
    for click in std::mem::take(&mut clicks.0) {
        let Some(((), camera, transform, p)) = camera_at(cameras.iter().map(|(c, t)| ((), c, t)), &full, click) else {
            continue;
        };
        let Ok(ray) = camera.viewport_to_world(transform, p) else { continue };
        let filter = |e: Entity| targets.contains(e);
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        let Some((entity, hit)) = ray_cast.cast_ray(ray, &settings).first() else { continue };
        let Ok(PickId(target)) = targets.get(*entity) else { continue };
        let Some((origin, rotation)) = target_pose(doc, *target) else { continue };
        let inv = rotation.inverse();
        let n = hit.normal;
        events.send(ViewportEvent::FeaturePoint {
            target: *target,
            point: inv * (from_render(hit.point) - origin),
            normal: (inv * DVec3::new(n.x as f64, -n.z as f64, n.y as f64)).normalize_or_zero(),
        });
    }
}

/// Plane: a 20 mm square and its normal; axis: a line ±30 mm along it.
fn draw_highlight(
    state: Res<FeaturePick>,
    index: Res<SceneIndex>,
    mut gizmos: Gizmos<OnTop>,
    capturing: Res<crate::export::Capturing>,
) {
    if capturing.0 || !state.active {
        return;
    }
    let (Some((target, feature)), Some(doc)) = (&state.highlight, &index.snapshot) else { return };
    let Some((o, q)) = target_pose(doc, *target) else { return };
    let w = |p: DVec3| to_render(o + q * p);
    match feature {
        Feature::Plane { origin, normal } => {
            let n = normal.normalize_or_zero();
            let u = n.any_orthonormal_vector() * 10.0;
            let v = n.cross(u);
            let c = [u + v, -u + v, -u - v, u - v].map(|d| w(*origin + d));
            for i in 0..4 {
                gizmos.line(c[i], c[(i + 1) % 4], ACCENT);
            }
            gizmos.arrow(w(*origin), w(*origin + n * 8.0), ACCENT);
        }
        Feature::Axis { origin, dir, .. } => {
            let d = dir.normalize_or_zero() * 30.0;
            gizmos.line(w(*origin - d), w(*origin + d), ACCENT);
        }
    }
}
