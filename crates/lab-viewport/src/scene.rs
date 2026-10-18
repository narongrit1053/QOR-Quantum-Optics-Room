//! Builds Bevy entities from a document snapshot: room floor, platforms (with hole grids),
//! nodes and lighting.
//!
//! Render space is Bevy's: metres, +Y up. Document space is millimetres, +Z up. Convert only
//! through [`to_render`] and [`rotation_to_render`].

use std::f32::consts::FRAC_PI_2;

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, GlobalAmbientLight, NotShadowCaster},
    math::{Affine2, DQuat, DVec3},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use lab_core::{Document, Finish, Node, NodeId, Platform, Shape, Support};

use crate::{FrameRequest, Quality};
use crate::host::{PendingRequests, RedrawState};

/// Render units per millimetre.
const MM: f32 = 0.001;

pub(crate) fn to_render(p: DVec3) -> Vec3 {
    Vec3::new(p.x as f32, p.z as f32, -p.y as f32) * MM
}

/// Document (Z-up) rotation → render (Y-up) rotation.
pub(crate) fn rotation_to_render(q: DQuat) -> Quat {
    let change = Quat::from_rotation_x(-FRAC_PI_2);
    change * q.as_quat() * change.inverse()
}

/// Everything spawned from a document; despawned when another document is loaded.
#[derive(Component)]
pub(crate) struct SceneEntity;

/// Large background geometry (the floor) that "frame all" ignores.
#[derive(Component)]
pub(crate) struct Backdrop;

#[derive(Component, Clone, Copy)]
pub(crate) struct SceneNode(pub NodeId);

/// Marks entities the user can select.
#[derive(Component)]
pub(crate) struct PickTarget;

/// A node's own material and its unhighlighted look, so highlights can be undone.
#[derive(Component)]
pub(crate) struct NodeLook {
    pub material: Handle<StandardMaterial>,
    pub base_color: Color,
}

pub(crate) struct ScenePlugin {
    pub document: Document,
}

#[derive(Resource)]
struct InitialDocument(Document);

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InitialDocument(self.document.clone()))
            // Transparent, so the UI's background gradient shows behind the scene.
            .insert_resource(ClearColor(Color::NONE))
            .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.9, 1.0), brightness: 160.0, ..default() })
            .insert_resource(DirectionalLightShadowMap { size: 1024 })
            .add_systems(Startup, (spawn_lighting, spawn_initial_document))
            .add_systems(PreUpdate, load_requested_document);
    }
}

fn spawn_lighting(mut commands: Commands, quality: Res<Quality>) {
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: quality.shadows(), ..default() },
        Transform::from_xyz(2.5, 6.0, 3.5).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
        // One cascade over the working area keeps shadows affordable on integrated GPUs.
        CascadeShadowConfigBuilder { num_cascades: 1, maximum_distance: 8.0, ..default() }.build(),
    ));
    // Cool fill from the opposite side, no shadows.
    commands.spawn((
        DirectionalLight { illuminance: 2_500.0, color: Color::srgb(0.8, 0.88, 1.0), ..default() },
        Transform::from_xyz(-3.0, 3.0, -2.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
    ));
}

fn spawn_initial_document(
    mut commands: Commands,
    initial: Res<InitialDocument>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    spawn_document(&mut commands, &initial.0, &mut meshes, &mut materials, &mut images);
    commands.remove_resource::<InitialDocument>();
}

fn load_requested_document(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    existing: Query<Entity, With<SceneEntity>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut redraw: ResMut<RedrawState>,
) {
    let Some(doc) = pending.0.iter().rev().find_map(|r| match r {
        FrameRequest::LoadDocument(doc) => Some(doc),
        _ => None,
    }) else {
        return;
    };
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_document(&mut commands, doc, &mut meshes, &mut materials, &mut images);
    // New meshes need a second frame before bounds (and picking) are available.
    redraw.keep_alive = true;
}

fn spawn_document(
    commands: &mut Commands,
    doc: &Document,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    // Floor.
    commands.spawn((
        SceneEntity,
        Backdrop,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(doc.room.size.x as f32 * MM, doc.room.size.y as f32 * MM))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb_u8(22, 23, 26),
            perceptual_roughness: 0.95,
            reflectance: 0.2,
            ..default()
        })),
        Transform::default(),
    ));

    for platform in &doc.platforms {
        spawn_platform(commands, platform, meshes, materials, images);
    }

    for node in &doc.nodes {
        let Some(platform) = doc.platform(node.platform) else { continue };
        spawn_node(commands, platform, node, meshes, materials);
    }
}

fn platform_transform(platform: &Platform) -> (DVec3, DQuat) {
    (platform.position.extend(platform.top_height), DQuat::from_rotation_z(platform.rotation_z))
}

fn spawn_platform(
    commands: &mut Commands,
    platform: &Platform,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    let (origin, rotation) = platform_transform(platform);
    let at = |local: DVec3| Transform {
        translation: to_render(origin + rotation * local),
        rotation: rotation_to_render(rotation),
        ..default()
    };
    let size = platform.size;
    let thickness = platform.thickness;

    let slab_material = materials.add(StandardMaterial {
        base_color: Color::srgb_u8(34, 36, 40),
        metallic: 0.4,
        perceptual_roughness: 0.6,
        ..default()
    });
    commands.spawn((
        SceneEntity,
        Mesh3d(meshes.add(Cuboid::new(size.x as f32 * MM, thickness as f32 * MM, size.y as f32 * MM))),
        MeshMaterial3d(slab_material),
        at(DVec3::new(0.0, 0.0, -thickness / 2.0)),
    ));

    // Top surface: a single quad with a tiled, mipmapped hole texture.
    let mut top = StandardMaterial {
        base_color: Color::srgb_u8(118, 122, 128),
        metallic: 0.6,
        perceptual_roughness: 0.45,
        ..default()
    };
    if let Some(grid) = &platform.hole_grid {
        top.base_color_texture = Some(images.add(hole_tile_image(grid.thread.diameter() / grid.pitch)));
        let tiles = Vec2::new((size.x / grid.pitch) as f32, (size.y / grid.pitch) as f32);
        let offset = (0.5 - grid.margin / grid.pitch) as f32;
        top.uv_transform = Affine2::from_scale_angle_translation(tiles, 0.0, Vec2::splat(offset));
    }
    commands.spawn((
        SceneEntity,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x as f32 * MM, size.y as f32 * MM))),
        MeshMaterial3d(materials.add(top)),
        at(DVec3::new(0.0, 0.0, 0.2)),
    ));

    if let Support::Legs { size: leg, inset } = platform.support {
        let leg_height = platform.top_height - thickness;
        if leg_height > 0.0 {
            let mesh = meshes.add(Cuboid::new(leg as f32 * MM, leg_height as f32 * MM, leg as f32 * MM));
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb_u8(28, 29, 33),
                perceptual_roughness: 0.7,
                ..default()
            });
            let (hx, hy) = (size.x / 2.0 - inset, size.y / 2.0 - inset);
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                commands.spawn((
                    SceneEntity,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    at(DVec3::new(sx * hx, sy * hy, -thickness - leg_height / 2.0)),
                ));
            }
        }
    }
}

fn spawn_node(
    commands: &mut Commands,
    platform: &Platform,
    node: &Node,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let (origin, rotation) = platform_transform(platform);
    let transform = Transform {
        translation: to_render(origin + rotation * node.position),
        rotation: rotation_to_render(rotation * node.rotation),
        ..default()
    };
    let mesh = match &node.shape {
        // Document Y maps to render −Z, so box depth and height swap places.
        Shape::Box { size } => meshes.add(Cuboid::new(size.x as f32 * MM, size.z as f32 * MM, size.y as f32 * MM)),
        // Bevy cylinders run along local Y, which is the document's local Z after conversion.
        Shape::Cylinder { radius, length } => meshes.add(Cylinder::new(*radius as f32 * MM, *length as f32 * MM)),
    };
    let material = finish_material(&node.finish);
    let base_color = material.base_color;
    let handle = materials.add(material);

    let mut entity = commands.spawn((
        SceneEntity,
        SceneNode(node.id),
        Mesh3d(mesh),
        MeshMaterial3d(handle.clone()),
        NodeLook { material: handle, base_color },
        transform,
    ));
    if node.pickable {
        entity.insert(PickTarget);
    }
    if matches!(node.finish, Finish::Glass | Finish::Beam { .. }) {
        entity.insert(NotShadowCaster);
    }
}

fn finish_material(finish: &Finish) -> StandardMaterial {
    match finish {
        Finish::AnodizedBlack => StandardMaterial {
            base_color: Color::srgb_u8(29, 30, 33),
            metallic: 0.3,
            perceptual_roughness: 0.42,
            ..default()
        },
        Finish::Stainless => StandardMaterial {
            base_color: Color::srgb_u8(201, 204, 209),
            metallic: 1.0,
            perceptual_roughness: 0.28,
            ..default()
        },
        Finish::Mirror => StandardMaterial {
            base_color: Color::srgb_u8(240, 242, 245),
            metallic: 1.0,
            perceptual_roughness: 0.05,
            ..default()
        },
        Finish::Glass => StandardMaterial {
            base_color: Color::srgba(0.78, 0.88, 1.0, 0.3),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.06,
            reflectance: 0.6,
            ..default()
        },
        Finish::Painted { rgb } => StandardMaterial {
            base_color: Color::srgb_u8(rgb[0], rgb[1], rgb[2]),
            metallic: 0.1,
            perceptual_roughness: 0.5,
            ..default()
        },
        Finish::Beam { wavelength_nm } => StandardMaterial {
            base_color: beam_color(*wavelength_nm),
            unlit: true,
            ..default()
        },
    }
}

/// False colour for a beam. Visible wavelengths get their approximate hue; IR and UV get
/// distinct saturated colours, because beams must stay visible on screen.
fn beam_color(wavelength_nm: f64) -> Color {
    match wavelength_nm {
        w if w < 380.0 => Color::srgb_u8(191, 90, 242),  // UV: violet
        w if w < 450.0 => Color::srgb_u8(94, 92, 230),   // violet-blue
        w if w < 495.0 => Color::srgb_u8(10, 132, 255),  // blue
        w if w < 570.0 => Color::srgb_u8(48, 209, 88),   // green
        w if w < 590.0 => Color::srgb_u8(255, 214, 10),  // yellow
        w if w < 620.0 => Color::srgb_u8(255, 159, 10),  // orange
        w if w <= 700.0 => Color::srgb_u8(255, 69, 58),  // red
        _ => Color::srgb_u8(255, 55, 95),                // near/far IR: pink-red
    }
}

/// One repeating tile of the hole grid: a dark hole of `hole_fraction` × tile width in the
/// centre, with a full mip chain so distant holes average out instead of shimmering.
fn hole_tile_image(hole_fraction: f64) -> Image {
    const SIZE: usize = 128;
    let radius = (hole_fraction * SIZE as f64 / 2.0) as f32;
    let centre = SIZE as f32 / 2.0;

    // Linear luminance, 1 = bare surface.
    let mut level: Vec<f32> = (0..SIZE * SIZE)
        .map(|i| {
            let (x, y) = ((i % SIZE) as f32 + 0.5, (i / SIZE) as f32 + 0.5);
            let d = Vec2::new(x - centre, y - centre).length();
            let hole = ((radius + 0.75 - d) / 1.5).clamp(0.0, 1.0);
            let chamfer = ((radius * 1.3 - d) / (radius * 0.3)).clamp(0.0, 1.0) * (1.0 - hole);
            1.0 - 0.94 * hole - 0.22 * chamfer
        })
        .collect();

    let mut data = Vec::new();
    let mut side = SIZE;
    let mut mip_levels = 0;
    loop {
        for &l in &level {
            let v = (srgb_encode(l) * 255.0).round() as u8;
            data.extend_from_slice(&[v, v, v, 255]);
        }
        mip_levels += 1;
        if side == 1 {
            break;
        }
        let half = side / 2;
        level = (0..half * half)
            .map(|i| {
                let (x, y) = (2 * (i % half), 2 * (i / half));
                (level[y * side + x] + level[y * side + x + 1] + level[(y + 1) * side + x] + level[(y + 1) * side + x + 1])
                    / 4.0
            })
            .collect();
        side = half;
    }

    let first_level = data[..SIZE * SIZE * 4].to_vec();
    let mut image = Image::new(
        Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        first_level,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = mip_levels;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}

fn srgb_encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_convert_from_z_up_millimetres() {
        assert_eq!(to_render(DVec3::new(1000.0, 0.0, 0.0)), Vec3::X);
        assert_eq!(to_render(DVec3::new(0.0, 0.0, 1000.0)), Vec3::Y);
        assert_eq!(to_render(DVec3::new(0.0, 1000.0, 0.0)), -Vec3::Z);
    }

    #[test]
    fn rotations_convert_consistently_with_positions() {
        let q = DQuat::from_rotation_z(0.7) * DQuat::from_rotation_y(0.3);
        let v = DVec3::new(10.0, -20.0, 30.0);
        let expected = to_render(q * v);
        let actual = rotation_to_render(q) * to_render(v);
        assert!((expected - actual).length() < 1e-6);
    }

    #[test]
    fn hole_tile_has_full_mip_chain() {
        let image = hole_tile_image(6.0 / 25.0);
        assert_eq!(image.texture_descriptor.mip_level_count, 8);
        let texels: usize = (0..8).map(|l| (128usize >> l).pow(2)).sum();
        assert_eq!(image.data.as_ref().unwrap().len(), texels * 4);
    }
}
