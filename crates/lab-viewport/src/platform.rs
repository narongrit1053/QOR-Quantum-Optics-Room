//! Platforms in the scene (M1-SPEC §5.4).
//!
//! Each platform is a root entity ([`PlatformRoot`]) placed at its frame origin on the top
//! surface. Its parts (top, body, legs) and the nodes on it are children of the root, so moving
//! or re-heighting a platform is one transform write, and rebuilding the parts keeps the nodes.

use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};
use lab_core::{Document, HoleRef, Platform, PlatformId, SelectionTarget, Support, Surface, ZoneShape, compute_holes};

use crate::hole_material::{
    GridGeometry, HoleGridExt, HoleTopMaterial, ZoneParam, build_mask, hole_params, mask_image,
};
use crate::mesh::trimesh_to_bevy;
use crate::picking::{HighlightLook, PickId};
use crate::scene::{MM, SceneEntity, rotation_to_render, to_render};

/// The root of a platform's entities. The id is not read by systems yet; it names the
/// entity in the world (debugging, later tools).
#[derive(Component)]
pub(crate) struct PlatformRoot(#[allow(dead_code)] pub PlatformId);

/// The platform frame in the room ([`Document::platform_pose`], which composes `OnPlatform`
/// chains); falls back to the platform's own fields for a broken chain.
pub(crate) fn platform_root_transform(doc: &Document, p: &Platform) -> Transform {
    let (origin, rotation_z) = doc.platform_pose(p.id).unwrap_or((p.position.extend(p.top_height), p.rotation_z));
    Transform {
        translation: to_render(origin),
        rotation: rotation_to_render(DQuat::from_rotation_z(rotation_z)),
        ..default()
    }
}

/// Standoff posts under an `OnPlatform` platform (Ø25 mm, like Thorlabs RS-series pedestals).
const STANDOFF_DIAMETER: f64 = 25.0;
const STANDOFF_INSET: f64 = 40.0;

pub(crate) struct PlatformAssets<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub hole_materials: &'a mut Assets<HoleTopMaterial>,
    pub images: &'a mut Assets<Image>,
}

/// (base colour, metallic, roughness) of the top and the body.
fn surface_looks(surface: Surface) -> ((Color, f32, f32), (Color, f32, f32)) {
    match surface {
        Surface::Stainless => ((Color::srgb_u8(118, 122, 128), 0.6, 0.45), (Color::srgb_u8(34, 36, 40), 0.4, 0.6)),
        Surface::BlackAnodized => ((Color::srgb_u8(40, 42, 46), 0.3, 0.5), (Color::srgb_u8(22, 23, 26), 0.3, 0.55)),
    }
}

/// Shader parameters and presence mask for the platform's holes (computed by lab-core).
fn hole_material_parts(p: &Platform, images: &mut Assets<Image>) -> Result<HoleGridExt, String> {
    let set = compute_holes(p).map_err(|e| e.to_string())?;
    let mask = build_mask(set.cols, set.rows, |col, row| set.contains_label(HoleRef { col, row }));
    let grid = (p.hole_grid.is_some() && set.cols > 0 && set.rows > 0).then(|| {
        let origin = set.position(set.first);
        GridGeometry {
            origin: Vec2::new(origin.x as f32, origin.y as f32),
            pitch: Vec2::new(set.pitch.x as f32, set.pitch.y as f32),
            hole_radius: set.thread_radius as f32,
        }
    });
    let zones: Vec<ZoneParam> = p
        .hole_grid
        .iter()
        .flat_map(|g| &g.exclusions)
        .map(|zone| match zone.shape {
            ZoneShape::Rect { center, size } => ZoneParam::Rect {
                center: Vec2::new(center.x as f32, center.y as f32),
                half: Vec2::new(size.x as f32, size.y as f32) / 2.0,
            },
            ZoneShape::Circle { center, radius } => {
                ZoneParam::Circle { center: Vec2::new(center.x as f32, center.y as f32), radius: radius as f32 }
            }
        })
        .collect();
    let params = hole_params(grid.as_ref(), &mask, &zones);
    Ok(HoleGridExt { params, mask: images.add(mask_image(&mask)) })
}

/// Spawns the top, body and legs of `p` as children of `root`, with platform-local transforms.
/// Everything is computed before anything is spawned, so on `Err` (a sentence for
/// `SceneError`) nothing was added.
pub(crate) fn spawn_platform_parts(
    commands: &mut Commands,
    root: Entity,
    p: &Platform,
    assets: &mut PlatformAssets,
) -> Result<Vec<Entity>, String> {
    let path = p.outline_path().map_err(|e| e.to_string())?;
    let built = lab_geom::extrude(&path, -p.thickness, 0.0).map_err(|e| format!("its outline could not be meshed ({e})"))?;
    let extension = hole_material_parts(p, assets.images)?;
    let owner = SelectionTarget::Platform(p.id);
    let ((top_color, top_metallic, top_roughness), (body_color, body_metallic, body_roughness)) =
        surface_looks(p.surface);
    let mut parts = Vec::new();

    // Top: the outline at z = 0, holes drawn by the hole-grid material.
    let top_material = assets.hole_materials.add(HoleTopMaterial {
        base: StandardMaterial {
            base_color: top_color,
            metallic: top_metallic,
            perceptual_roughness: top_roughness,
            ..default()
        },
        extension,
    });
    parts.push(
        commands
            .spawn((
                SceneEntity,
                ChildOf(root),
                Mesh3d(assets.meshes.add(trimesh_to_bevy(&built.top))),
                MeshMaterial3d(top_material.clone()),
                Transform::default(),
                PickId(owner),
                HighlightLook::HoleTop { material: top_material, owner },
            ))
            .id(),
    );

    // Body: side walls and bottom.
    let body_material = assets.materials.add(StandardMaterial {
        base_color: body_color,
        metallic: body_metallic,
        perceptual_roughness: body_roughness,
        ..default()
    });
    parts.push(
        commands
            .spawn((
                SceneEntity,
                ChildOf(root),
                Mesh3d(assets.meshes.add(trimesh_to_bevy(&built.body))),
                MeshMaterial3d(body_material.clone()),
                Transform::default(),
                PickId(owner),
                HighlightLook::Standard { material: body_material, base_color: body_color, owner },
            ))
            .id(),
    );

    // Legs and isolators down to the floor, standoffs down to the parent's top. Not pickable.
    let floor = p.top_height - p.thickness;
    let (size, inset, height, round) = match p.support {
        Support::Legs { size, inset } => (size, inset, floor, false),
        Support::Isolators { size, inset } => (size, inset, floor, true),
        Support::OnPlatform { standoff, .. } => (STANDOFF_DIAMETER, STANDOFF_INSET, standoff, true),
        Support::Fixed => (0.0, 0.0, 0.0, false),
    };
    // leg_positions only answers for Legs; isolators and standoffs use the same corner rule.
    let positions = Platform { support: Support::Legs { size, inset }, ..p.clone() }.leg_positions();
    if height > 0.0 && !positions.is_empty() {
        let mesh = if round {
            assets.meshes.add(Cylinder::new(size as f32 * MM / 2.0, height as f32 * MM))
        } else {
            assets.meshes.add(Cuboid::new(size as f32 * MM, height as f32 * MM, size as f32 * MM))
        };
        let material = assets.materials.add(StandardMaterial {
            base_color: Color::srgb_u8(28, 29, 33),
            perceptual_roughness: 0.7,
            ..default()
        });
        for leg in positions {
            parts.push(
                commands
                    .spawn((
                        SceneEntity,
                        ChildOf(root),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::from_translation(to_render(DVec3::new(leg.x, leg.y, -p.thickness - height / 2.0))),
                    ))
                    .id(),
            );
        }
    }
    Ok(parts)
}

