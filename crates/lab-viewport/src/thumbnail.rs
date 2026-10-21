//! Offscreen library thumbnails (M4-PLAN POLA-61): one catalog part at a time is spawned on its
//! own render layer, framed by a dedicated camera rendering into a 256×256 image, captured with
//! a `Screenshot` and written as a PNG.

use std::collections::VecDeque;
use std::f32::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};

use bevy::{
    camera::{RenderTarget, primitives::Aabb, visibility::RenderLayers},
    gltf::GltfAssetLabel,
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use lab_catalog::Source;
use lab_core::part::Geometry;

use crate::host::{EventSender, PendingRequests, RedrawState};
use crate::scene::{MM, PartMeshes, finish_material, part_mesh};
use crate::{CatalogRes, FrameRequest, ViewportEvent};

const SIZE: u32 = 256;
const LAYER: usize = 7;
const FOV: f32 = 0.5;
/// Frames rendered after the part is ready before it is framed, and again before the capture.
const SETTLE: u32 = 3;

/// The thumbnail camera; the host leaves its render target alone.
#[derive(Component)]
pub(crate) struct ThumbCamera;

/// The GLB scene of the thumbnail in flight.
#[derive(Component)]
struct ThumbGlb;

struct InFlight {
    part: String,
    path: PathBuf,
    /// Root, camera and light: despawned when done.
    entities: Vec<Entity>,
    image: Handle<Image>,
    /// False until a GLB's scene has spawned.
    ready: bool,
    frames: u32,
    shot: bool,
}

#[derive(Resource, Default)]
struct Thumbs {
    queue: VecDeque<(String, PathBuf)>,
    current: Option<InFlight>,
}

pub(crate) struct ThumbnailPlugin;

impl Plugin for ThumbnailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Thumbs>().add_observer(glb_ready).add_systems(Update, step);
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct Spawner<'w> {
    catalog: Res<'w, CatalogRes>,
    server: Res<'w, AssetServer>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    part_meshes: ResMut<'w, PartMeshes>,
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut thumbs: ResMut<Thumbs>,
    pending: Res<PendingRequests>,
    mut spawner: Spawner,
    mut redraw: ResMut<RedrawState>,
    events: Res<EventSender>,
    boxes: Query<(&Aabb, &GlobalTransform, &RenderLayers)>,
    mut cameras: Query<&mut Transform, With<ThumbCamera>>,
) {
    for request in &pending.0 {
        if let FrameRequest::RenderThumbnail(part, path) = request {
            thumbs.queue.push_back((part.clone(), path.clone()));
        }
    }
    if thumbs.current.is_none()
        && let Some((part, path)) = thumbs.queue.pop_front()
    {
        match start(&mut commands, &mut spawner, &part) {
            Ok(job) => thumbs.current = Some(InFlight { part, path, frames: 0, shot: false, ..job }),
            Err(e) => {
                warn!("no thumbnail for {part}: {e}");
                events.send(ViewportEvent::ThumbnailReady { part, path, ok: false });
            }
        }
    }
    // Keep the on-demand loop running while thumbnails are pending.
    redraw.keep_alive |= thumbs.current.is_some() || !thumbs.queue.is_empty();
    let Some(job) = thumbs.current.as_mut() else { return };
    if !job.ready || job.shot {
        return;
    }
    job.frames += 1;
    if job.frames == SETTLE {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (aabb, transform, layers) in &boxes {
            if !layers.intersects(&RenderLayers::layer(LAYER)) {
                continue;
            }
            let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
            for i in 0..8 {
                let s = Vec3::new(
                    if i & 1 == 0 { -1.0 } else { 1.0 },
                    if i & 2 == 0 { -1.0 } else { 1.0 },
                    if i & 4 == 0 { -1.0 } else { 1.0 },
                );
                let p = transform.transform_point(c + h * s);
                min = min.min(p);
                max = max.max(p);
            }
        }
        if min.x > max.x {
            (min, max) = (Vec3::splat(-0.01), Vec3::splat(0.01));
        }
        let center = (min + max) / 2.0;
        let radius = ((max - min).length() / 2.0).max(0.001);
        // 3/4 view from the front right, slightly above.
        let dir = Vec3::new(1.0, 0.8, 1.2).normalize();
        let distance = radius / (FOV / 2.0).sin() * 1.05;
        for mut transform in &mut cameras {
            *transform = Transform::from_translation(center + dir * distance).looking_at(center, Vec3::Y);
        }
    } else if job.frames == 2 * SETTLE {
        job.shot = true;
        commands.spawn(Screenshot::image(job.image.clone())).observe(captured);
    }
}

/// Spawns the part, a camera and a light on [`LAYER`].
fn start(commands: &mut Commands, s: &mut Spawner, part: &str) -> Result<InFlight, String> {
    let def = s.catalog.0.get(part).ok_or_else(|| format!("{part} is not in the library"))?;
    let layer = RenderLayers::layer(LAYER);
    let root = commands.spawn((Transform::default(), Visibility::default(), layer.clone())).id();
    let mut ready = true;
    if let Geometry::Mesh { asset } = &def.geometry {
        let (source, _) = lab_catalog::mesh_path(asset).ok_or_else(|| format!("its mesh file {asset} is missing"))?;
        let src = match source {
            Source::User => "user",
            Source::BuiltIn => "builtin",
        };
        let path = format!("{src}://meshes/{}.glb", &asset["blake3:".len()..]);
        commands.spawn((
            ChildOf(root),
            ThumbGlb,
            WorldAssetRoot(s.server.load(GltfAssetLabel::Scene(0).from_asset(path))),
            // Same conversion as glb.rs: part space (mm, +Z up).
            Transform { rotation: Quat::from_rotation_x(-FRAC_PI_2), scale: Vec3::splat(MM), ..default() },
            layer.clone(),
        ));
        ready = false;
    } else {
        let mesh = part_mesh(&s.catalog.0, part, &Default::default())?;
        for (finish, handle) in s.part_meshes.pieces(&mut s.meshes, &mesh) {
            let material = s.materials.add(finish_material(&finish));
            commands.spawn((ChildOf(root), Mesh3d(handle), MeshMaterial3d(material), Transform::default(), layer.clone()));
        }
    }
    let image = s.images.add(Image::new_target_texture(SIZE, SIZE, TextureFormat::Rgba8UnormSrgb, None));
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera { order: -10, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
            RenderTarget::Image(image.clone().into()),
            Projection::Perspective(PerspectiveProjection { fov: FOV, near: 0.001, far: 100.0, ..default() }),
            Msaa::Sample4,
            Transform::from_xyz(0.5, 0.4, 0.6).looking_at(Vec3::ZERO, Vec3::Y),
            ThumbCamera,
            layer.clone(),
        ))
        .id();
    let light = commands
        .spawn((
            DirectionalLight { illuminance: 9_000.0, ..default() },
            Transform::from_xyz(1.0, 2.0, 1.5).looking_at(Vec3::ZERO, Vec3::Y),
            layer,
        ))
        .id();
    Ok(InFlight {
        part: String::new(),
        path: PathBuf::new(),
        entities: vec![root, camera, light],
        image,
        ready,
        frames: 0,
        shot: false,
    })
}

/// Puts a spawned GLB's meshes on the thumbnail layer and starts the countdown.
fn glb_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    glbs: Query<(), With<ThumbGlb>>,
    children: Query<&Children>,
    mut thumbs: ResMut<Thumbs>,
) {
    if glbs.get(ready.entity).is_err() {
        return;
    }
    for entity in children.iter_descendants(ready.entity) {
        commands.entity(entity).insert(RenderLayers::layer(LAYER));
    }
    if let Some(job) = thumbs.current.as_mut() {
        job.ready = true;
    }
}

fn captured(shot: On<ScreenshotCaptured>, mut commands: Commands, mut thumbs: ResMut<Thumbs>, events: Res<EventSender>) {
    commands.entity(shot.entity).try_despawn();
    let Some(job) = thumbs.current.take() else { return };
    for entity in job.entities {
        commands.entity(entity).try_despawn();
    }
    let ok = match write_png(&job.path, &shot.image) {
        Ok(()) => true,
        Err(e) => {
            warn!("thumbnail {} not written: {e}", job.path.display());
            false
        }
    };
    events.send(ViewportEvent::ThumbnailReady { part: job.part, path: job.path, ok });
}

pub(crate) fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    let (w, h) = (image.width(), image.height());
    let mut data = image.data.clone().ok_or("the capture has no pixels")?;
    if matches!(image.texture_descriptor.format, TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb) {
        for px in data.as_chunks_mut::<4>().0.iter_mut() {
            px.swap(0, 2);
        }
    }
    if data.len() != (w * h * 4) as usize {
        return Err(format!("unexpected capture format {:?}", image.texture_descriptor.format));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&data)).map_err(|e| e.to_string())
}
