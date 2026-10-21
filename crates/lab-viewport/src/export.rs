//! PNG export of the current view (M5-PLAN POLA-66): an offscreen camera copies the main
//! camera's transform and projection, renders the scene into an image of the requested size,
//! and the capture is written as a PNG.

use std::collections::VecDeque;
use std::path::PathBuf;

use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
};

use crate::camera::{OrbitCamera, TopPane};
use crate::host::{EventSender, PendingRequests, RedrawState};
use crate::{FrameRequest, ViewportEvent};

/// Frames rendered before the capture.
const SETTLE: u32 = 3;
/// Opaque background: the middle stop of the UI's viewport gradient (`Theme.viewport-bg`).
const BACKGROUND: Color = Color::srgb(0x16 as f32 / 255.0, 0x18 as f32 / 255.0, 0x1C as f32 / 255.0);

/// The export camera; the host leaves its render target alone.
#[derive(Component)]
pub(crate) struct ExportCamera;

type MainCamera = (&'static Transform, &'static Projection, Option<&'static Msaa>);

struct InFlight {
    path: PathBuf,
    camera: Entity,
    image: Handle<Image>,
    frames: u32,
}

/// True while an export (or project thumbnail) renders: selection and hover tints, the outline,
/// the gizmo and measure/inspect markers are hidden so they stay out of the image.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Capturing(pub bool);

#[derive(Resource, Default)]
struct Exports {
    queue: VecDeque<(PathBuf, u32, u32)>,
    current: Option<InFlight>,
}

pub(crate) struct ExportPlugin;

impl Plugin for ExportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Exports>().init_resource::<Capturing>().add_systems(Update, step);
    }
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut exports: ResMut<Exports>,
    pending: Res<PendingRequests>,
    mut images: ResMut<Assets<Image>>,
    mut redraw: ResMut<RedrawState>,
    events: Res<EventSender>,
    mut capturing: ResMut<Capturing>,
    main: Query<MainCamera, (With<OrbitCamera>, Without<TopPane>)>,
) {
    for request in &pending.0 {
        if let FrameRequest::ExportImage(path, w, h) = request {
            exports.queue.push_back((path.clone(), (*w).clamp(16, 8192), (*h).clamp(16, 8192)));
        }
    }
    // shortcut: runs beside thumbnails (separate queue); fine while thumbnails are rare, share one queue if GPU memory becomes a problem.
    if exports.current.is_none()
        && let Some((path, width, height)) = exports.queue.pop_front()
    {
        let Ok((transform, projection, _)) = main.single() else {
            events.send(ViewportEvent::ImageExported { path, ok: false });
            return;
        };
        let image = images.add(Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None));
        // The projection's aspect ratio follows the new target: same vertical FOV / extent.
        let camera = commands
            .spawn((
                Camera3d::default(),
                Camera { order: -20, clear_color: ClearColorConfig::Custom(BACKGROUND), ..default() },
                RenderTarget::Image(image.clone().into()),
                projection.clone(),
                Msaa::Sample4,
                *transform,
                ExportCamera,
            ))
            .id();
        exports.current = Some(InFlight { path, camera, image, frames: 0 });
    }
    redraw.keep_alive |= exports.current.is_some() || !exports.queue.is_empty();
    capturing.set_if_neq(Capturing(exports.current.is_some()));
    let Some(job) = exports.current.as_mut() else { return };
    job.frames += 1;
    if job.frames == SETTLE {
        commands.spawn(Screenshot::image(job.image.clone())).observe(captured);
    }
}

fn captured(
    shot: On<ScreenshotCaptured>,
    mut commands: Commands,
    mut exports: ResMut<Exports>,
    mut capturing: ResMut<Capturing>,
    events: Res<EventSender>,
) {
    commands.entity(shot.entity).try_despawn();
    let Some(job) = exports.current.take() else { return };
    capturing.set_if_neq(Capturing(false));
    commands.entity(job.camera).try_despawn();
    let ok = match crate::thumbnail::write_png(&job.path, &shot.image) {
        Ok(()) => true,
        Err(e) => {
            warn!("image {} not written: {e}", job.path.display());
            false
        }
    };
    events.send(ViewportEvent::ImageExported { path: job.path, ok });
}
