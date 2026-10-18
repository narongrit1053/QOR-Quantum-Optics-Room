//! Hover and click selection by ray casting against scene meshes.

use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::NodeId;

use crate::camera::{Clicks, OrbitCamera, PointerState, handle_input, to_camera_space};
use crate::host::{EventSender, PendingRequests, ViewportSize};
use crate::scene::{NodeLook, PickTarget, SceneNode};
use crate::{FrameRequest, ViewportEvent, ViewportInput};

const ACCENT: Color = Color::srgb(10.0 / 255.0, 132.0 / 255.0, 1.0);

#[derive(Resource, Default)]
pub(crate) struct Selection {
    pub selected: Option<NodeId>,
    pub hovered: Option<NodeId>,
}

pub(crate) struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Selection>()
            .add_systems(Update, (pick.after(handle_input), apply_highlight.after(pick)));
    }
}

#[allow(clippy::too_many_arguments)]
fn pick(
    pending: Res<PendingRequests>,
    pointer: Res<PointerState>,
    clicks: Res<Clicks>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    targets: Query<&SceneNode, With<PickTarget>>,
    mut ray_cast: MeshRayCast,
    mut selection: ResMut<Selection>,
    events: Res<EventSender>,
    full: Res<ViewportSize>,
) {
    let Ok((camera, camera_transform)) = cameras.single() else { return };
    let mut node_at = |pos: Vec2| -> Option<NodeId> {
        let ray = camera.viewport_to_world(camera_transform, to_camera_space(camera, &full, pos)).ok()?;
        let filter = |entity: Entity| targets.contains(entity);
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        let (entity, _) = ray_cast.cast_ray(ray, &settings).first()?;
        targets.get(*entity).ok().map(|node| node.0)
    };

    let mut selected = selection.selected;
    for request in &pending.0 {
        if let FrameRequest::Select(id) = request {
            selected = *id;
        }
    }
    for &click in &clicks.0 {
        selected = node_at(click);
    }

    // Hover follows the pointer, but not while dragging the camera.
    let pointer_moved = pending.0.iter().any(|r| {
        matches!(r, FrameRequest::Input(ViewportInput::PointerMove { .. } | ViewportInput::PointerLeave))
    });
    let mut hovered = selection.hovered;
    if pointer_moved || !clicks.0.is_empty() {
        hovered = match pointer.position {
            Some(pos) if !pointer.is_dragging() => node_at(pos),
            _ => None,
        };
    }

    if selected != selection.selected {
        selection.selected = selected;
        events.send(ViewportEvent::SelectionChanged(selected));
    }
    if hovered != selection.hovered {
        selection.hovered = hovered;
        events.send(ViewportEvent::HoverChanged(hovered));
    }
}

fn apply_highlight(
    selection: Res<Selection>,
    nodes: Query<(&SceneNode, &NodeLook)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !selection.is_changed() {
        return;
    }
    for (node, look) in &nodes {
        let color = if selection.selected == Some(node.0) {
            mix(look.base_color, ACCENT, 0.65)
        } else if selection.hovered == Some(node.0) {
            mix(look.base_color, Color::WHITE, 0.22)
        } else {
            look.base_color
        };
        let needs_update = materials.get(&look.material).is_some_and(|m| m.base_color != color);
        if needs_update && let Some(mut material) = materials.get_mut(&look.material) {
            material.base_color = color;
        }
    }
}

/// Linear-space blend that keeps `a`'s alpha (so glass stays transparent).
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_linear(), b.to_linear());
    Color::LinearRgba(LinearRgba::new(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha,
    ))
}
