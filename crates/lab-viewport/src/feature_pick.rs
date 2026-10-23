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

/// M9 E2. DOF glyph on the selected jointed node: an arrow per slide and an arc per rotation at
/// the joint origin. Immediate-mode gizmos, so nothing is drawn (or redrawn) while idle.
pub(crate) struct DofGlyphPlugin;

impl Plugin for DofGlyphPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_dof_glyph.after(crate::picking::pick));
    }
}

fn draw_dof_glyph(
    selection: Res<crate::picking::Selection>,
    index: Res<SceneIndex>,
    capturing: Res<crate::export::Capturing>,
    mut gizmos: Gizmos<OnTop>,
) {
    if capturing.0 {
        return;
    }
    let (Some(SelectionTarget::Node(id)), Some(doc)) = (selection.selected, &index.snapshot) else { return };
    let Some(node) = doc.node(id) else { return };
    let lab_core::Attach::Joint(j) = &node.attach else { return };
    let Some((o, r)) = crate::gizmo::joint_frame(doc, node, j) else { return };
    for d in lab_mates::joints::dofs(j.kind) {
        let axis = crate::gizmo::dof_axis(*d, r);
        if crate::gizmo::is_rot(*d) {
            // Three-quarter arc of radius 15 mm about the axis, with an arrow at its end.
            let u = axis.any_orthonormal_vector() * 15.0;
            let v = axis.cross(u);
            let at = |a: f64| o + u * a.cos() + v * a.sin();
            let n = 24;
            let end = 1.5 * std::f64::consts::PI;
            for k in 0..n {
                let (a0, a1) = (end * k as f64 / n as f64, end * (k + 1) as f64 / n as f64);
                gizmos.line(to_render(at(a0)), to_render(at(a1)), ACCENT);
            }
            gizmos.arrow(to_render(at(end - 0.3)), to_render(at(end)), ACCENT);
        } else {
            gizmos.arrow(to_render(o - axis * 20.0), to_render(o + axis * 20.0), ACCENT);
        }
    }
}
