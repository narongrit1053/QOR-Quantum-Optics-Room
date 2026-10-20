//! Top-view "blueprint" look (M3-PLAN POLA-49): while the camera is in Top mode and the setting
//! is on, platforms and parts swap to flat unlit colours and holes are drawn as outlines. The
//! original looks are kept and restored; meshes are not rebuilt. Highlights still apply on top
//! because `HighlightLook::base_color` is what the highlight system tints.

use bevy::prelude::*;
use lab_core::SelectionTarget;

use crate::FrameRequest;
use crate::camera::{OrbitCamera, handle_input};
use crate::hole_material::HoleTopMaterial;
use crate::host::PendingRequests;
use crate::picking::{HighlightLook, Selection, pick};
use crate::ViewMode;

const PLATFORM_FILL: Color = Color::srgb(0.21, 0.25, 0.31);
const PART_FILL: Color = Color::srgb(0.78, 0.79, 0.81);

/// The session setting from `SetBlueprint`, and whether the look is applied right now.
#[derive(Resource, Default)]
struct Blueprint {
    setting: bool,
    active: bool,
}

/// A look's own colour and lighting before the blueprint swap.
#[derive(Component)]
struct Saved {
    base_color: Color,
    unlit: bool,
}

pub(crate) struct BlueprintPlugin;

impl Plugin for BlueprintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Blueprint>().add_systems(Update, apply_blueprint.after(handle_input).before(pick));
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_blueprint(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    cameras: Query<&OrbitCamera>,
    mut state: ResMut<Blueprint>,
    mut looks: Query<(Entity, &mut HighlightLook, Option<&Saved>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut hole_materials: ResMut<Assets<HoleTopMaterial>>,
    mut selection: ResMut<Selection>,
) {
    for request in &pending.0 {
        if let FrameRequest::SetBlueprint(on) = request {
            state.setting = *on;
        }
    }
    let top = cameras.iter().any(|c| c.mode() == ViewMode::Top);
    let active = state.setting && top;
    if active == state.active && (!active || !looks.iter_mut().any(|(_, look, _)| look.is_added())) {
        return;
    }
    state.active = active;

    for (entity, mut look, saved) in &mut looks {
        match &mut *look {
            HighlightLook::Standard { material, base_color, owner } => {
                let fill = match owner {
                    SelectionTarget::Platform(_) => PLATFORM_FILL,
                    SelectionTarget::Node(_) => PART_FILL,
                    SelectionTarget::Room => continue,
                };
                match (active, saved) {
                    (true, None) => {
                        let unlit = materials.get(&*material).is_some_and(|m| m.unlit);
                        commands.entity(entity).insert(Saved { base_color: *base_color, unlit });
                        *base_color = fill;
                        if let Some(mut m) = materials.get_mut(&*material) {
                            m.unlit = true;
                        }
                    }
                    (false, Some(saved)) => {
                        *base_color = saved.base_color;
                        if let Some(mut m) = materials.get_mut(&*material) {
                            m.unlit = saved.unlit;
                        }
                        commands.entity(entity).remove::<Saved>();
                    }
                    _ => {}
                }
            }
            HighlightLook::HoleTop { material, .. } => {
                let flag = u32::from(active);
                if hole_materials.get(&*material).is_some_and(|m| m.extension.params.blueprint != flag)
                    && let Some(mut m) = hole_materials.get_mut(&*material)
                {
                    m.extension.params.blueprint = flag;
                }
            }
        }
    }
    // Re-run the highlight pass so the new base colours reach the materials.
    selection.set_changed();
}
