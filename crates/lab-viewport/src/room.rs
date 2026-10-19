//! The room: floor, walls, footprints and the wall fade (M1-SPEC §5.6).
//!
//! T4b stub: `spawn_room` spawns only the floor, `wall_occludes` returns false and
//! `RoomPlugin` only registers [`WallFade`]. T4c replaces it with the full implementation; the
//! signatures here are the contract.

#![allow(dead_code)] // The wall types are used by T4c.

use bevy::{math::DVec2, math::DVec3, prelude::*};
use lab_core::{Room, WallDisplay};

use crate::mesh::trimesh_to_bevy;
use crate::scene::{Backdrop, SceneEntity};

/// Full version: init WallFade; Update: update_wall_fade.after(crate::camera::animate_camera).
pub(crate) struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WallFade>();
    }
}

#[derive(Resource, Default)]
pub(crate) struct WallFade {
    pub display: WallDisplay,
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

/// AssetId<StandardMaterial> is Copy, so Clone + Copy derive is valid.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct WallState {
    pub alpha: f32,
    pub target: f32,
    pub material: AssetId<StandardMaterial>,
}

/// Spawns floor, wall bodies and footprints; returns the entities (stored in SceneIndex.room).
/// Stub: the floor only.
pub(crate) fn spawn_room(
    commands: &mut Commands,
    room: &Room,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<Entity> {
    let floor = match lab_geom::flat_polygon(&room.outline, 0.0, true, 0.001) {
        Ok(floor) => floor,
        Err(error) => {
            warn!("room floor could not be meshed: {error}");
            return Vec::new();
        }
    };
    let entity = commands
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
        .id();
    vec![entity]
}

/// Pure, unit-tested (T4c). Stub: never occludes.
pub(crate) fn wall_occludes(_wall: &WallInfo, _eye: DVec3, _targets: &[DVec3], _currently_faded: bool) -> bool {
    false
}
