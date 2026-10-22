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
/// Frames to wait for a GLB scene before the part is skipped (a failed load never gets ready).
const LOAD_TIMEOUT: u32 = 600;

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
    /// The GLB file, to notice a failed load.
    glb: Option<Handle<bevy::gltf::Gltf>>,
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
        app.init_resource::<Thumbs>().add_observer(glb_ready).add_systems(Update, step.after(crate::export::step));
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
    capturing: Res<crate::export::Capturing>,
) {
    for request in &pending.0 {
        if let FrameRequest::RenderThumbnail(part, path) = request {
            thumbs.queue.push_back((part.clone(), path.clone()));
        }
    }
    // One offscreen render at a time: wait while an export renders.
    if thumbs.current.is_none()
        && !capturing.0
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
    if !job.ready {
        job.frames += 1;
        let failed = job.glb.as_ref().is_some_and(|h| spawner.server.load_state(h).is_failed());
        if failed || job.frames >= LOAD_TIMEOUT {
            let job = thumbs.current.take().unwrap();
            warn!("no thumbnail for {}: its GLB did not load", job.part);
            for entity in job.entities {
                commands.entity(entity).try_despawn();
            }
            events.send(ViewportEvent::ThumbnailReady { part: job.part, path: job.path, ok: false });
        }
        return;
    }
    if job.shot {
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
    let mut glb = None;
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
            WorldAssetRoot(s.server.load(GltfAssetLabel::Scene(0).from_asset(path.clone()))),
            // Same conversion as glb.rs: part space (mm, +Z up).
            Transform { rotation: Quat::from_rotation_x(-FRAC_PI_2), scale: Vec3::splat(MM), ..default() },
            layer.clone(),
        ));
        glb = Some(s.server.load(path));
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
        glb,
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
        job.frames = 0;
    }
}

fn captured(shot: On<ScreenshotCaptured>, mut commands: Commands, mut thumbs: ResMut<Thumbs>, events: Res<EventSender>) {
    commands.entity(shot.entity).try_despawn();
    let Some(job) = thumbs.current.take() else { return };
    for entity in job.entities {
        commands.entity(entity).try_despawn();
    }
    let ok = match write_png(&job.path, &shot.image, false) {
        Ok(()) => true,
        Err(e) => {
            warn!("thumbnail {} not written: {e}", job.path.display());
            false
        }
    };
    events.send(ViewportEvent::ThumbnailReady { part: job.part, path: job.path, ok });
}

pub(crate) fn write_png(path: &Path, image: &Image, background: bool) -> Result<(), String> {
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
    if background {
        under_gradient(&mut data, w, h);
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

/// Reads a thumbnail PNG written by [`write_png`] as `(width, height, RGBA8 pixels)`. The UI
/// loads it from bytes because its path-keyed image cache would keep showing a stale render.
pub fn read_thumbnail(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut reader = png::Decoder::new(file).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some((info.width, info.height, buf))
}

/// Composites RGBA8 `data` over the UI's viewport gradient (`Theme.viewport-bg`: a radial
/// gradient, farthest-corner circle, #2A2D33 0 %, #16181C 55 %, #0B0C0E 100 %).
fn under_gradient(data: &mut [u8], w: u32, h: u32) {
    const STOPS: [(f32, [f32; 3]); 3] =
        [(0.0, [42.0, 45.0, 51.0]), (0.55, [22.0, 24.0, 28.0]), (1.0, [11.0, 12.0, 14.0])];
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let radius = (cx * cx + cy * cy).sqrt().max(1.0);
    for (i, px) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
        let t = (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / radius).min(1.0);
        let (a, b) = if t < STOPS[1].0 { (STOPS[0], STOPS[1]) } else { (STOPS[1], STOPS[2]) };
        let f = (t - a.0) / (b.0 - a.0);
        let alpha = px[3] as f32 / 255.0;
        for c in 0..3 {
            let bg = a.1[c] + (b.1[c] - a.1[c]) * f;
            px[c] = (px[c] as f32 * alpha + bg * (1.0 - alpha)).round() as u8;
        }
        px[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gradient_under_transparent_pixels() {
        let mut data = vec![0u8; 4 * 4 * 4];
        data[0..4].copy_from_slice(&[200, 100, 50, 255]);
        super::under_gradient(&mut data, 4, 4);
        assert_eq!(&data[0..4], &[200, 100, 50, 255]);
        // A corner pixel is near the outer stop, a centre pixel near the inner one.
        assert!(data[3 * 4] < 20 && data[3 * 4 + 3] == 255);
        let centre = 4 * (4 + 1);
        assert!(data[centre] > 30, "{:?}", &data[centre..centre + 4]);
    }
}
