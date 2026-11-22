//! The Beam Profiler probe (beam polarization spec, "Beam profiler probe"): with `Tool::Probe` in
//! Top view a click near a drawn beam pins a probe at the nearest point on it. Probes are world
//! points (segment indices change on every retrace), at most [`MAX_PROBES`]; the overlay sends
//! them to the app as [`crate::OverlayItem::Probe`], which reads the trace there and draws the cards.

use bevy::{math::DVec3, prelude::*};

use crate::beams::Beams;
use crate::camera::{Clicks, OrbitCamera, camera_at, handle_input};
use crate::host::{PendingRequests, ViewportSize};
use crate::picking::pick;
use crate::scene::to_render;
use crate::{FrameRequest, Tool, ViewMode};

/// Snap radius in physical px, as the measure tool's.
const SNAP_PX: f32 = 12.0;
const MAX_PROBES: usize = 4;

#[derive(Resource, Default)]
pub(crate) struct Probes {
    active: bool,
    pub(crate) points: Vec<DVec3>,
}

pub(crate) struct ProbePlugin;

impl Plugin for ProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Probes>().add_systems(Update, probe_input.after(handle_input).before(pick));
    }
}

fn probe_input(
    pending: Res<PendingRequests>,
    mut clicks: ResMut<Clicks>,
    cameras: Query<(&OrbitCamera, &Camera, &GlobalTransform)>,
    full: Res<ViewportSize>,
    beams: Res<Beams>,
    mut probes: ResMut<Probes>,
) {
    for request in &pending.0 {
        match request {
            FrameRequest::SetTool(tool) => probes.active = *tool == Tool::Probe,
            FrameRequest::LoadDocument(_) | FrameRequest::ClearProbes(None) => probes.points.clear(),
            FrameRequest::ClearProbes(Some(i)) if *i < probes.points.len() => {
                probes.points.remove(*i);
            }
            _ => {}
        }
    }
    if !probes.active {
        return;
    }
    // Clicks pin probes instead of selecting.
    for click in std::mem::take(&mut clicks.0) {
        let hit = camera_at(cameras.iter().map(|(o, c, t)| (o, c, t)), &full, click);
        let Some((_, camera, transform, _)) = hit.filter(|h| h.0.mode() == ViewMode::Top) else { continue };
        let to_full = |v: Vec2| match camera.logical_target_size() {
            Some(size) if full.0.x > 0.0 && full.0.y > 0.0 => v * full.0 / size,
            _ => v,
        };
        let project = |p: DVec3| camera.world_to_viewport(transform, to_render(p)).ok().map(to_full);
        let segments: Vec<_> = beams
            .trace
            .segments
            .iter()
            .filter_map(|s| Some((project(s.start)?, project(s.end)?, s.start, s.end)))
            .collect();
        if let Some(p) = snap_to_beam(click, &segments, SNAP_PX) {
            push_probe(&mut probes.points, p);
        }
    }
}

/// The nearest point on the projected segments `(a, b, world a, world b)` within `max_px` of
/// `cursor`, as the world point at the same fraction along the segment.
pub(crate) fn snap_to_beam(cursor: Vec2, segments: &[(Vec2, Vec2, DVec3, DVec3)], max_px: f32) -> Option<DVec3> {
    segments
        .iter()
        .filter_map(|&(a, b, wa, wb)| {
            let d = b - a;
            let t = if d.length_squared() > 0.0 { ((cursor - a).dot(d) / d.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            let dist = cursor.distance(a + d * t);
            (dist <= max_px).then(|| (dist, wa.lerp(wb, t as f64)))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, p)| p)
}

/// Adds a probe; a fifth replaces the oldest.
fn push_probe(points: &mut Vec<DVec3>, p: DVec3) {
    if points.len() >= MAX_PROBES {
        points.remove(0);
    }
    points.push(p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_within_12_px() {
        let seg = (Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), DVec3::ZERO, DVec3::new(200.0, 0.0, 75.0));
        let p = snap_to_beam(Vec2::new(25.0, 5.0), &[seg], 12.0).unwrap();
        assert!(p.distance(DVec3::new(50.0, 0.0, 18.75)) < 1e-4, "{p}");
        assert_eq!(snap_to_beam(Vec2::new(25.0, 20.0), &[seg], 12.0), None);
        let near = (Vec2::new(0.0, 8.0), Vec2::new(100.0, 8.0), DVec3::Y, DVec3::new(200.0, 1.0, 0.0));
        let p = snap_to_beam(Vec2::new(25.0, 5.0), &[seg, near], 12.0).unwrap();
        assert!((p.y - 1.0).abs() < 1e-9, "picked the farther segment: {p}");
    }

    #[test]
    fn fifth_probe_replaces_oldest() {
        let mut points = Vec::new();
        for i in 0..5 {
            push_probe(&mut points, DVec3::X * i as f64);
        }
        assert_eq!(points.len(), 4);
        assert_eq!(points[0], DVec3::X);
    }
}
