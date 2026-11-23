//! Hover and click selection by ray casting against scene meshes, and selection highlights
//! (M1-SPEC §5.7).

use std::collections::HashMap;

use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_core::{NodeId, SelectionTarget};

use crate::camera::{Clicks, OrbitCamera, RightClicks, PointerState, camera_at, handle_input};
use crate::hole_material::HoleTopMaterial;
use crate::host::{EventSender, PendingRequests, ViewportSize};
use crate::room::WallState;
use crate::scene::SceneIndex;
use crate::{FrameRequest, Modifiers, ViewportEvent, ViewportInput};

pub(crate) const ACCENT: Color = Color::srgb(10.0 / 255.0, 132.0 / 255.0, 1.0);

/// What a click on this entity selects.
#[derive(Component, Clone, Copy)]
pub(crate) struct PickId(pub SelectionTarget);

/// An entity's own material and unhighlighted look, so highlights can be applied and undone.
#[derive(Component, Clone)]
pub(crate) enum HighlightLook {
    Standard { material: Handle<StandardMaterial>, base_color: Color, owner: SelectionTarget },
    HoleTop { material: Handle<HoleTopMaterial>, owner: SelectionTarget },
}

/// The look's material is shared with other entities (one per finish or GLB material), so
/// highlights swap the entity to a tinted copy instead of changing the material.
#[derive(Component)]
pub(crate) struct SharedMaterial;

#[derive(Resource, Default)]
pub(crate) struct Selection {
    pub selected: Option<SelectionTarget>,
    pub extra: Vec<NodeId>,
    pub hovered: Option<SelectionTarget>,
}

impl Selection {
    pub(crate) fn is_selected(&self, target: SelectionTarget) -> bool {
        self.selected == Some(target)
            || matches!(target, SelectionTarget::Node(id) if self.extra.contains(&id))
    }
}

fn resolve_left_click(
    selected: Option<SelectionTarget>,
    extra: &[NodeId],
    hit: Option<SelectionTarget>,
    ctrl: bool,
) -> (Option<SelectionTarget>, Option<ViewportEvent>) {
    if ctrl {
        return (selected, hit.map(ViewportEvent::SelectionToggled));
    }
    if !extra.is_empty() {
        // With extras active, clicking a selected item narrows instead of clearing.
        return (hit, Some(ViewportEvent::SelectionChanged(hit)));
    }
    // Clicking the selected item again clears the selection.
    let next = if hit.is_some() && hit == selected { None } else { hit };
    let event = (next != selected).then_some(ViewportEvent::SelectionChanged(next));
    (next, event)
}

pub(crate) struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::outline::OutlinePlugin)
            .init_resource::<Selection>()
            .add_systems(Update, (pick.after(handle_input), apply_highlight.after(pick)));
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pick(
    pending: Res<PendingRequests>,
    pointer: Res<PointerState>,
    clicks: Res<Clicks>,
    right_clicks: Res<RightClicks>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    targets: Query<(&PickId, Option<&WallState>)>,
    mut ray_cast: MeshRayCast,
    mut selection: ResMut<Selection>,
    (index, events, full): (Res<SceneIndex>, Res<EventSender>, Res<ViewportSize>),
    mut mods: Local<Modifiers>,
) {
    for request in &pending.0 {
        if let FrameRequest::Input(
            ViewportInput::PointerDown { mods: m, .. }
            | ViewportInput::PointerUp { mods: m, .. }
            | ViewportInput::PointerMove { mods: m, .. },
        ) = request
        {
            *mods = *m;
        }
    }
    let select_locked = |t: SelectionTarget| {
        index.snapshot.as_ref().is_some_and(|doc| match t {
            SelectionTarget::Node(id) => doc.node(id).is_some_and(|n| n.select_lock),
            SelectionTarget::Platform(id) => doc.platform(id).is_some_and(|p| p.select_lock),
            SelectionTarget::Room => false,
        })
    };
    // `through_locked`: plain left clicks and hover ray-cast through select-locked items.
    let mut target_at = |pos: Vec2, through_locked: bool| -> Option<SelectionTarget> {
        let ((), camera, camera_transform, p) = camera_at(cameras.iter().map(|(c, t)| ((), c, t)), &full, pos)?;
        let ray = camera.viewport_to_world(camera_transform, p).ok()?;
        // Faded walls let clicks through; hidden entities are skipped by the default visibility.
        let filter = |entity: Entity| {
            targets.get(entity).is_ok_and(|(id, wall)| {
                wall.is_none_or(|w| w.alpha >= 0.5) && !(through_locked && select_locked(id.0))
            })
        };
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        let (entity, _) = ray_cast.cast_ray(ray, &settings).first()?;
        targets.get(*entity).ok().map(|(id, _)| id.0)
    };
    let exists = |t: SelectionTarget| index.snapshot.as_ref().is_some_and(|doc| t.exists_in(doc));

    let mut selected = selection.selected;
    for request in &pending.0 {
        match request {
            // App-driven selection updates the viewport without echoing SelectionChanged back.
            FrameRequest::Select(target) => {
                selected = target.filter(|&t| exists(t));
                if selection.selected != selected {
                    selection.selected = selected;
                }
            }
            FrameRequest::SelectExtra(ids) => {
                let filtered: Vec<_> =
                    ids.iter().copied().filter(|&id| exists(SelectionTarget::Node(id))).collect();
                if filtered != selection.extra {
                    selection.extra = filtered;
                }
            }
            _ => {}
        }
    }
    let mut force_select_event = false;
    let mut toggles = Vec::new();
    for &click in &clicks.0 {
        let hit = target_at(click, !mods.shift);
        let (next, event) = resolve_left_click(selected, &selection.extra, hit, mods.ctrl);
        selected = next;
        match event {
            Some(ViewportEvent::SelectionToggled(t)) => toggles.push(t),
            Some(ViewportEvent::SelectionChanged(_)) => force_select_event = true,
            _ => {}
        }
    }
    let mut menu = None;
    for &click in &right_clicks.0 {
        let hit = target_at(click, false);
        let already_selected = hit.is_some_and(|t| {
            selected == Some(t) || matches!(t, SelectionTarget::Node(id) if selection.extra.contains(&id))
        });
        if !already_selected {
            selected = hit;
        }
        menu = Some((hit, click));
    }

    // Hover follows the pointer (nodes only), but not while dragging the camera.
    let pointer_moved = pending.0.iter().any(|r| {
        matches!(r, FrameRequest::Input(ViewportInput::PointerMove { .. } | ViewportInput::PointerLeave))
    });
    let mut hovered = selection.hovered;
    if pointer_moved || !clicks.0.is_empty() {
        hovered = match pointer.position {
            Some(pos) if !pointer.is_dragging() => target_at(pos, !mods.shift).filter(|t| matches!(t, SelectionTarget::Node(_))),
            _ => None,
        };
    }

    if selected != selection.selected || force_select_event {
        selection.selected = selected;
        events.send(ViewportEvent::SelectionChanged(selected));
    }
    for target in toggles {
        events.send(ViewportEvent::SelectionToggled(target));
    }
    if let Some((target, pos)) = menu {
        events.send(ViewportEvent::ContextMenu { target, pos });
    }
    if hovered != selection.hovered {
        selection.hovered = hovered;
        events.send(ViewportEvent::HoverChanged(hovered));
    }
}

/// Tinted copies of shared materials: (base, colour, emissive, unlit) → copy.
type Variants = HashMap<(AssetId<StandardMaterial>, [u32; 4], [u32; 4], bool), Handle<StandardMaterial>>;

type Looks<'w, 's> = Query<'w, 's, (Entity, &'static HighlightLook, Has<SharedMaterial>, Option<&'static MeshMaterial3d<StandardMaterial>>)>;

/// Applies the current selection and hover to every look. Runs when the selection changed or
/// new looks were spawned (a rebuilt platform keeps its highlight); writes only changed values.
#[allow(clippy::too_many_arguments)]
fn apply_highlight(
    mut commands: Commands,
    selection: Res<Selection>,
    capturing: Res<crate::export::Capturing>,
    looks: Looks,
    added: Query<(), Added<HighlightLook>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut hole_materials: ResMut<Assets<HoleTopMaterial>>,
    mut variants: Local<Variants>,
) {
    if !selection.is_changed() && !capturing.is_changed() && added.is_empty() {
        return;
    }
    let none = Selection::default();
    let selection = if capturing.0 { &none } else { &*selection };
    for (entity, look, shared, shown) in &looks {
        match look {
            HighlightLook::Standard { material, base_color, owner } => {
                let is_selected = selection.is_selected(*owner);
                let color = if is_selected {
                    let t = if matches!(owner, SelectionTarget::Node(_)) { 0.65 } else { 0.25 };
                    mix(*base_color, ACCENT, t)
                } else if selection.hovered == Some(*owner) {
                    mix(*base_color, Color::WHITE, 0.4)
                } else {
                    *base_color
                };
                // Hover also lifts emission, so it shows on dark parts where a white mix barely does.
                let hovered = selection.hovered == Some(*owner) && !is_selected;
                let lift = LinearRgba::rgb(0.12, 0.12, 0.12);
                let Some(current) = materials.get(material) else { continue };
                // The alpha belongs to the material (walls fade it); highlights only change colour.
                let color = color.with_alpha(current.base_color.alpha());
                if shared {
                    let want = if current.base_color == color && !hovered {
                        material.clone()
                    } else {
                        let emissive = if hovered { lift } else { current.emissive };
                        let key = (material.id(), bits(color.to_linear()), bits(emissive), current.unlit);
                        match variants.get(&key) {
                            Some(handle) => handle.clone(),
                            None => {
                                let mut m = current.clone();
                                m.base_color = color;
                                m.emissive = emissive;
                                let handle = materials.add(m);
                                variants.insert(key, handle.clone());
                                handle
                            }
                        }
                    };
                    if shown.is_none_or(|m| m.0 != want) {
                        commands.entity(entity).insert(MeshMaterial3d(want));
                    }
                    continue;
                }
                let emissive = if hovered { lift } else { LinearRgba::BLACK };
                if (current.base_color != color || current.emissive != emissive)
                    && let Some(mut m) = materials.get_mut(material)
                {
                    m.base_color = color;
                    m.emissive = emissive;
                }
            }
            HighlightLook::HoleTop { material, owner } => {
                let on = if selection.is_selected(*owner) { 1.0 } else { 0.0 };
                let Some(current) = hole_materials.get(material) else { continue };
                let params = &current.extension.params;
                if (params.highlight != on || params.show_zones != on)
                    && let Some(mut m) = hole_materials.get_mut(material)
                {
                    m.extension.params.highlight = on;
                    m.extension.params.show_zones = on;
                }
            }
        }
    }
}

fn bits(c: LinearRgba) -> [u32; 4] {
    c.to_f32_array().map(f32::to_bits)
}

/// Linear-space blend that keeps `a`'s alpha.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_linear(), b.to_linear());
    Color::LinearRgba(LinearRgba::new(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::{NodeId, PlatformId};

    #[test]
    fn is_selected_matches_primary_or_extra_node() {
        let sel = Selection {
            selected: Some(SelectionTarget::Node(NodeId(1))),
            extra: vec![NodeId(2), NodeId(3)],
            hovered: None,
        };
        assert!(sel.is_selected(SelectionTarget::Node(NodeId(1))));
        assert!(sel.is_selected(SelectionTarget::Node(NodeId(2))));
        assert!(sel.is_selected(SelectionTarget::Node(NodeId(3))));
        assert!(!sel.is_selected(SelectionTarget::Node(NodeId(4))));
        assert!(!sel.is_selected(SelectionTarget::Platform(PlatformId(1))));
        assert!(!sel.is_selected(SelectionTarget::Room));

        let plat = Selection {
            selected: Some(SelectionTarget::Platform(PlatformId(2))),
            extra: vec![],
            hovered: None,
        };
        assert!(plat.is_selected(SelectionTarget::Platform(PlatformId(2))));
        assert!(!plat.is_selected(SelectionTarget::Node(NodeId(2))));
    }

    #[test]
    fn ctrl_click_emits_toggle_without_changing_local_selection() {
        let primary = Some(SelectionTarget::Node(NodeId(1)));
        let hit = SelectionTarget::Node(NodeId(2));
        assert_eq!(
            resolve_left_click(primary, &[], Some(hit), true),
            (primary, Some(ViewportEvent::SelectionToggled(hit)))
        );
        // Ctrl+click on empty space changes nothing and emits no event.
        assert_eq!(resolve_left_click(primary, &[NodeId(2)], None, true), (primary, None));
    }

    #[test]
    fn plain_click_without_extra_clears_when_reclicking_primary() {
        let primary = Some(SelectionTarget::Node(NodeId(1)));
        assert_eq!(
            resolve_left_click(primary, &[], primary, false),
            (None, Some(ViewportEvent::SelectionChanged(None)))
        );
        assert_eq!(resolve_left_click(None, &[], None, false), (None, None));
        let other = Some(SelectionTarget::Node(NodeId(2)));
        assert_eq!(
            resolve_left_click(primary, &[], other, false),
            (other, Some(ViewportEvent::SelectionChanged(other)))
        );
    }

    #[test]
    fn plain_click_with_extra_reports_hit_without_clearing() {
        let primary = Some(SelectionTarget::Node(NodeId(1)));
        let extra = [NodeId(2), NodeId(3)];
        // Clicking primary while extras are active reports it even though `selected` is unchanged.
        assert_eq!(
            resolve_left_click(primary, &extra, primary, false),
            (primary, Some(ViewportEvent::SelectionChanged(primary)))
        );
        // Clicking an extra node reports it.
        let hit_extra = Some(SelectionTarget::Node(NodeId(2)));
        assert_eq!(
            resolve_left_click(primary, &extra, hit_extra, false),
            (hit_extra, Some(ViewportEvent::SelectionChanged(hit_extra)))
        );
        // Clicking empty space clears.
        assert_eq!(
            resolve_left_click(primary, &extra, None, false),
            (None, Some(ViewportEvent::SelectionChanged(None)))
        );
    }
}
