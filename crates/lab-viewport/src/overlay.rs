//! Top-view labels and dimension lines (M3-PLAN POLA-54). The viewport only projects; Slint draws.
//! After each update the items for the Top camera are recomputed and sent as
//! [`ViewportEvent::Overlay`] when they differ from the last ones sent, so an idle viewport sends
//! nothing and an animation sends at most one per frame.

use bevy::{math::DVec3, prelude::*, transform::TransformSystems};
use lab_core::SelectionTarget;

use crate::camera::OrbitCamera;
use crate::host::{EventSender, ViewportSize};
use crate::isolate::{Isolated, kept_platforms};
use crate::measure::node_room_pos;
use crate::scene::{SceneIndex, to_render};
use crate::{OverlayItem, ViewMode, ViewportEvent};

const MAX_LABELS: usize = 200;
const MIN_LABEL_GAP_PX: f32 = 40.0;
const DIMENSION_OFFSET_PX: f32 = 24.0;

#[derive(Resource, Default)]
struct LastSent(Vec<OverlayItem>);

pub(crate) struct OverlayPlugin;

impl Plugin for OverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastSent>().add_systems(PostUpdate, send_overlay.after(TransformSystems::Propagate));
    }
}

fn send_overlay(
    cameras: Query<(&OrbitCamera, &Camera, &GlobalTransform)>,
    index: Res<SceneIndex>,
    isolated: Res<Isolated>,
    full: Res<ViewportSize>,
    events: Res<EventSender>,
    mut last: ResMut<LastSent>,
) {
    let top = cameras.iter().find(|(o, c, _)| c.is_active && o.mode() == ViewMode::Top);
    let items = match (top, &index.snapshot) {
        (Some((_, camera, transform)), Some(doc)) => {
            let to_full = |v: Vec2| match camera.logical_target_size() {
                Some(size) if full.0.x > 0.0 && full.0.y > 0.0 => v * full.0 / size,
                _ => v,
            };
            let project = |p: DVec3| Some(to_full(camera.world_to_viewport(transform, to_render(p)).ok()?));
            // In Split only the pane's half may be drawn on (POLA-104).
            let pane = camera.logical_viewport_rect().map(|r| Rect::from_corners(to_full(r.min), to_full(r.max)));
            let inside = |p: Vec2| pane.is_none_or(|r| r.contains(p));
            let kept = isolated.0.map(|root| kept_platforms(doc, root));
            let shown = |p: lab_core::PlatformId| kept.as_ref().is_none_or(|k| k.contains(&p));
            let mut items = Vec::new();
            let mut labels: Vec<Vec2> = Vec::new();
            let mut label = |items: &mut Vec<OverlayItem>, target, pos: Vec2| {
                if inside(pos) && labels.len() < MAX_LABELS && labels.iter().all(|l| l.distance(pos) >= MIN_LABEL_GAP_PX) {
                    labels.push(pos);
                    items.push(OverlayItem::Label { target, pos });
                }
            };
            for p in doc.platforms.iter().filter(|p| shown(p.id)) {
                let (lo, hi) = doc.platform_room_aabb(p.id).unwrap_or_else(|| p.room_aabb());
                let z = hi.z;
                let corner = |x: f64, y: f64| project(DVec3::new(x, y, z));
                let (Some(tl), Some(tr), Some(bl), Some(br)) =
                    (corner(lo.x, hi.y), corner(hi.x, hi.y), corner(lo.x, lo.y), corner(hi.x, lo.y))
                else {
                    continue;
                };
                let centre = (tl + tr + bl + br) / 4.0;
                // Shift an edge 24 px away from the box centre, perpendicular to it.
                let offset = |a: Vec2, b: Vec2| {
                    let n = (b - a).perp().normalize_or_zero();
                    let n = if n.dot((a + b) / 2.0 - centre) < 0.0 { -n } else { n };
                    (a + n * DIMENSION_OFFSET_PX, b + n * DIMENSION_OFFSET_PX)
                };
                for ((a, b), length_mm) in [(offset(tl, tr), hi.x - lo.x), (offset(bl, tl), hi.y - lo.y)] {
                    if let Some((a, b)) = pane.map_or(Some((a, b)), |r| clip_segment(a, b, r)) {
                        items.push(OverlayItem::Dimension { a, b, length_mm });
                    }
                }
                label(&mut items, SelectionTarget::Platform(p.id), tl);
            }
            for n in &doc.nodes {
                if !doc.root_platform(n.id).is_some_and(shown) {
                    continue;
                }
                if let Some(s) = node_room_pos(doc, n.id).and_then(project) {
                    label(&mut items, SelectionTarget::Node(n.id), s);
                }
            }
            items
        }
        _ => Vec::new(),
    };
    if items != last.0 {
        events.send(ViewportEvent::Overlay(items.clone()));
        last.0 = items;
    }
}

/// The part of segment `a`–`b` inside `rect` (Liang–Barsky), `None` when it misses the rect.
fn clip_segment(a: Vec2, b: Vec2, rect: Rect) -> Option<(Vec2, Vec2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-d.x, a.x - rect.min.x), (d.x, rect.max.x - a.x), (-d.y, a.y - rect.min.y), (d.y, rect.max.y - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 { t0 = t0.max(t) } else { t1 = t1.min(t) }
        }
    }
    (t0 <= t1).then(|| (a + d * t0, a + d * t1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_clip_to_the_pane() {
        let pane = Rect::new(100.0, 0.0, 200.0, 100.0);
        let inside = (Vec2::new(120.0, 50.0), Vec2::new(180.0, 50.0));
        assert_eq!(clip_segment(inside.0, inside.1, pane), Some(inside));
        assert_eq!(clip_segment(Vec2::new(50.0, 50.0), Vec2::new(150.0, 50.0), pane), Some((Vec2::new(100.0, 50.0), Vec2::new(150.0, 50.0))));
        assert_eq!(clip_segment(Vec2::new(10.0, 50.0), Vec2::new(90.0, 50.0), pane), None);
        assert_eq!(clip_segment(Vec2::new(150.0, -50.0), Vec2::new(150.0, -10.0), pane), None);
    }
}
