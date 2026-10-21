//! Runs the Bevy app on its own thread and drives it on demand.
//!
//! Adapted from Slint's `examples/bevy/slint-hosts-bevy/slint_bevy_adapter.rs` (MIT). Changes:
//! the loop blocks until there is work instead of rendering continuously, pipelined rendering
//! is disabled so one `update()` produces one finished frame, and pipelines compile
//! synchronously so that single frame is complete.

use std::time::{Duration, Instant};

use bevy::{
    camera::{ManualTextureViewHandle, RenderTarget},
    prelude::*,
    render::{
        RenderApp, RenderPlugin,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::TextureFormat,
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderGraph, RenderGraphSystems, RenderInstance,
            RenderQueue,
        },
        settings::RenderCreation,
        texture::{ManualTextureView, ManualTextureViews},
    },
    window::{ExitCondition, WindowPlugin},
};
use lab_core::Document;

use crate::{FrameRequest, Quality, ViewportEvent, ViewportInput, ViewportRequest};

/// The UI side of a running viewport.
pub struct ViewportHandle {
    /// Finished frames. Display the newest and send older ones back with
    /// [`ViewportRequest::ReleaseTexture`].
    pub textures: async_channel::Receiver<wgpu::Texture>,
    pub requests: async_channel::Sender<ViewportRequest>,
    pub events: async_channel::Receiver<ViewportEvent>,
}

/// Requests queued for the next Bevy update. Cleared by the host after every update.
#[derive(Resource, Default)]
pub(crate) struct PendingRequests(pub Vec<FrameRequest>);

/// Systems set `keep_alive` when they need another frame right away (e.g. an animation that
/// has not settled). Camera motion also sets `moving`, which renders the next frame at reduced
/// resolution with a sharp frame once motion stops; other frames (settle frames, fades) render
/// at full resolution, so nothing follows them. The host resets both after every update.
#[derive(Resource, Default)]
pub(crate) struct RedrawState {
    pub keep_alive: bool,
    pub moving: bool,
}

/// Extra frames still to render after a spawn, so assets created this frame (meshes, materials,
/// mask images) are prepared and shown before the viewport goes idle. Counted down in `Last`.
#[derive(Resource, Default)]
pub(crate) struct SettleFrames(pub u8);

fn count_down_settle_frames(mut settle: ResMut<SettleFrames>, mut redraw: ResMut<RedrawState>) {
    if settle.0 > 0 {
        settle.0 -= 1;
        redraw.keep_alive = true;
    }
}

/// Sends events to the UI and wakes the UI thread so it handles them right away.
#[derive(Resource, Clone)]
pub(crate) struct EventSender {
    events: async_channel::Sender<ViewportEvent>,
    wake_ui: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl EventSender {
    pub fn send(&self, event: ViewportEvent) {
        // A closed channel means the UI is shutting down; nothing to report to.
        if self.events.try_send(event).is_ok() {
            (self.wake_ui)();
        }
    }
}

/// Number of textures cycled between the viewport and the UI: one on screen, one being
/// rendered. More would let the viewport render ahead, and on a GPU-bound machine every
/// frame rendered ahead is a frame the UI drops (measured: half the GPU time wasted).
const BUFFER_COUNT: usize = 2;
const WARMUP_FRAMES: u64 = 3;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn create_texture(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lab-viewport frame"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Bevy can only render to sRGB textures: https://github.com/bevyengine/bevy/issues/15201
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    // Skia imports the texture through the native graphics API, bypassing wgpu's lazy
    // zero-initialisation, so clear it before the UI can see it.
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &texture.create_view(&Default::default()),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    queue.submit([encoder.finish()]);
    texture
}

/// Starts the viewport on a new thread, rendering with the UI's wgpu `instance`, `device` and
/// `queue`, and showing `document`.
///
/// `wake_ui` is called from the viewport thread after every event it sends; it must wake the
/// UI event loop so the event (and its texture) is handled promptly. Waking through an async
/// channel's waker alone was measured to stall for ~600 ms on Windows.
pub fn start(
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    document: Document,
    quality: Option<Quality>,
    wake_ui: impl Fn() + Send + Sync + 'static,
) -> ViewportHandle {
    let (request_tx, request_rx) = async_channel::unbounded::<ViewportRequest>();
    let (texture_tx, texture_rx) = async_channel::bounded::<wgpu::Texture>(BUFFER_COUNT);
    let (event_tx, event_rx) = async_channel::unbounded::<ViewportEvent>();

    let adapter = device_adapter(&instance, &device);
    let info = adapter.get_info();
    let quality = quality.unwrap_or(match info.device_type {
        // Measured on Intel UHD (DX12): directional shadows cost ~5x the rest of the frame.
        wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::Cpu => Quality::Balanced,
        _ => Quality::High,
    });
    let msaa_samples = supported_msaa(&adapter, quality.msaa_samples());
    let events = EventSender { events: event_tx, wake_ui: std::sync::Arc::new(wake_ui) };
    events.send(ViewportEvent::Ready {
        adapter: info.name.clone(),
        backend: format!("{:?}", info.backend),
        quality,
        msaa_samples,
    });

    let render_device: RenderDevice = device.clone().into();
    let render_queue = RenderQueue::new(queue.clone());
    let render_adapter_info = RenderAdapterInfo::new(info);
    let render_adapter = RenderAdapter::new(adapter);
    let render_instance = RenderInstance::new(instance);

    std::thread::Builder::new()
        .name("lab-viewport".into())
        .spawn(move || {
            let host_events = events.clone();
            let runner = move |mut app: App| {
                app.finish();
                app.cleanup();
                run_on_demand(&mut app, &device, &queue, &request_rx, &host_events);
                AppExit::Success
            };

            let mut app = App::new();
            app.set_runner(runner);
            app.insert_resource(BackBuffer(None))
                .insert_resource(CurrentTarget(ManualTextureViewHandle(0)))
                .init_resource::<PendingRequests>()
                .init_resource::<RedrawState>()
                .init_resource::<SettleFrames>()
                .insert_resource(events)
                .insert_resource(quality)
                .insert_resource(MsaaSamples(msaa_samples))
                .insert_resource(ViewportSize(Vec2::new(640.0, 480.0)))
                .init_resource::<TargetSize>()
                .add_systems(First, apply_render_target)
                .add_systems(Last, count_down_settle_frames);
            crate::glb::register_sources(&mut app);
            app.add_plugins(
                DefaultPlugins
                    .build()
                    .disable::<PipelinedRenderingPlugin>()
                    .set(RenderPlugin {
                        render_creation: RenderCreation::manual(
                            render_device,
                            render_queue,
                            render_adapter_info,
                            render_adapter,
                            render_instance,
                        ),
                        synchronous_pipeline_compilation: true,
                        ..default()
                    })
                    .set(WindowPlugin {
                        primary_window: None,
                        exit_condition: ExitCondition::DontExit,
                        close_when_requested: false,
                        ..default()
                    }),
            );
            app.add_plugins((SendFrameToUiPlugin(texture_tx), ExtractResourcePlugin::<BackBuffer>::default()));
            app.add_plugins((crate::hole_material::HoleGridPlugin, crate::room::RoomPlugin, crate::glb::GlbPlugin, crate::thumbnail::ThumbnailPlugin, crate::export::ExportPlugin));
            app.add_plugins((crate::scene::ScenePlugin { document }, crate::camera::CameraPlugin, crate::picking::PickingPlugin));
            app.add_plugins((crate::place::PlacePlugin, crate::gizmo::GizmoPlugin, crate::blueprint::BlueprintPlugin, crate::isolate::IsolatePlugin, crate::measure::MeasurePlugin, crate::overlay::OverlayPlugin, crate::inspect::InspectPlugin));
            app.run();
        })
        .expect("failed to spawn the viewport thread");

    ViewportHandle { textures: texture_rx, requests: request_tx, events: event_rx }
}

/// The adapter `device` was created on. The UI chooses the GPU (preferring a discrete one), so
/// quality, MSAA support and Bevy's `RenderAdapter` must describe that GPU, not whichever one
/// a fresh adapter request would return.
fn device_adapter(instance: &wgpu::Instance, device: &wgpu::Device) -> wgpu::Adapter {
    let info = device.adapter_info();
    let same_gpu = |other: &wgpu::AdapterInfo| {
        (other.backend, other.vendor, other.device, other.device_type, &other.name, &other.device_pci_bus_id)
            == (info.backend, info.vendor, info.device, info.device_type, &info.name, &info.device_pci_bus_id)
    };
    let adapters = bevy::tasks::block_on(instance.enumerate_adapters(info.backend.into()));
    if let Some(adapter) = adapters.into_iter().find(|adapter| same_gpu(&adapter.get_info())) {
        return adapter;
    }
    warn!("the UI's GPU adapter ({}) was not found again; requesting one", info.name);
    bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("no GPU adapter for the viewport")
}

/// MSAA sample count the camera uses, already checked against the adapter.
#[derive(Resource, Clone, Copy)]
pub(crate) struct MsaaSamples(pub u32);

/// The highest sample count ≤ `wanted` that the adapter supports for the colour and depth
/// formats Bevy renders with. Unsupported counts are a validation error that stops rendering.
fn supported_msaa(adapter: &wgpu::Adapter, wanted: u32) -> u32 {
    let formats = [FORMAT, wgpu::TextureFormat::Rgba16Float, wgpu::TextureFormat::Depth32Float];
    [8, 4, 2, 1]
        .into_iter()
        .filter(|&n| n <= wanted)
        .find(|&n| formats.iter().all(|&f| adapter.get_texture_format_features(f).flags.sample_count_supported(n)))
        .unwrap_or(1)
}

/// The host loop. Blocks while idle; renders one frame per batch of requests and keeps
/// rendering only while some system sets [`RedrawState::keep_alive`].
///
/// Frames that are part of motion (camera animation, drags, zooming) render at a reduced
/// resolution chosen by [`ResolutionScaler`]; once motion stops for [`SETTLE`], one frame at
/// full resolution follows. Measured on Intel UHD: full-resolution frames are GPU-bound at
/// ~19 fps, half-resolution ones reach ~50 fps.
fn run_on_demand(
    app: &mut App,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    requests: &async_channel::Receiver<ViewportRequest>,
    events: &EventSender,
) {
    let mut free: Vec<wgpu::Texture> = (0..BUFFER_COUNT).map(|_| create_texture(device, queue, 640, 480)).collect();
    let mut full_size = (640u32, 480u32);
    let mut dirty = true;
    let mut frames_rendered = 0u64;
    let mut next_view_id = 0u32;
    let mut scaler = ResolutionScaler::default();
    let mut pointer_pressed = false;
    // Motion continues into the next frame while an animation runs.
    let mut animating = false;
    // Set after a reduced-resolution frame: when motion was last seen.
    let mut reduced_since: Option<Instant> = None;
    let mut loading = false;

    loop {
        // Wait for work unless a frame is due and a texture is free to render it into. After a
        // reduced-resolution frame, wait at most until the settle deadline.
        let can_render = dirty && !free.is_empty();
        let first = if can_render {
            requests.try_recv().ok()
        } else if let Some(since) = reduced_since.filter(|_| !free.is_empty()) {
            match recv_until(requests, since + SETTLE) {
                Ok(request) => request,
                Err(()) => return,
            }
        } else if loading {
            // A GLB is loading: tick the main schedule (no rendering) until its events wake a frame.
            match recv_until(requests, Instant::now() + LOAD_POLL) {
                Ok(Some(request)) => Some(request),
                Ok(None) => {
                    let world = app.world_mut();
                    world.run_schedule(Main);
                    world.clear_trackers();
                    loading = world.resource::<crate::glb::GlbLoading>().0;
                    dirty = world.resource::<RedrawState>().keep_alive;
                    None
                }
                Err(()) => return,
            }
        } else {
            match requests.recv_blocking() {
                Ok(request) => Some(request),
                Err(_) => return, // UI closed the channel: shut down.
            }
        };

        let mut pending = Vec::new();
        let mut input_motion = false;
        for request in first.into_iter().chain(std::iter::from_fn(|| requests.try_recv().ok())) {
            match request {
                ViewportRequest::ReleaseTexture(texture) => free.push(texture),
                ViewportRequest::Resize { width, height } => {
                    let new_size = (width.max(1), height.max(1));
                    if new_size != full_size {
                        full_size = new_size;
                        app.world_mut().resource_mut::<ViewportSize>().0 =
                            Vec2::new(new_size.0 as f32, new_size.1 as f32);
                        dirty = true;
                    }
                }
                ViewportRequest::Input(input) => {
                    match input {
                        ViewportInput::PointerDown { .. } => pointer_pressed = true,
                        ViewportInput::PointerUp { .. } => pointer_pressed = false,
                        ViewportInput::PointerMove { .. } if pointer_pressed => input_motion = true,
                        ViewportInput::Scroll { .. } => input_motion = true,
                        _ => {}
                    }
                    pending.push(FrameRequest::Input(input));
                }
                ViewportRequest::SetViewMode(mode) => pending.push(FrameRequest::SetViewMode(mode)),
                ViewportRequest::Frame(target) => pending.push(FrameRequest::Frame(target)),
                ViewportRequest::Select(target) => pending.push(FrameRequest::Select(target)),
                ViewportRequest::LoadDocument(doc) => pending.push(FrameRequest::LoadDocument(doc)),
                ViewportRequest::ApplyChanges { snapshot, changes } => {
                    pending.push(FrameRequest::ApplyChanges(snapshot, changes))
                }
                ViewportRequest::SetWallDisplay(display) => pending.push(FrameRequest::SetWallDisplay(display)),
                ViewportRequest::SetCatalog(catalog) => {
                    app.world_mut().insert_resource(crate::CatalogRes(catalog));
                }
                ViewportRequest::DragPart(drag) => pending.push(FrameRequest::DragPart(drag)),
                ViewportRequest::SetTool(tool) => pending.push(FrameRequest::SetTool(tool)),
                ViewportRequest::SetBlueprint(on) => pending.push(FrameRequest::SetBlueprint(on)),
                ViewportRequest::Isolate(id) => pending.push(FrameRequest::Isolate(id)),
                ViewportRequest::SetReduceMotion(on) => pending.push(FrameRequest::SetReduceMotion(on)),
                ViewportRequest::InspectPart(id) => pending.push(FrameRequest::InspectPart(id)),
                ViewportRequest::InspectMarkers(frames) => pending.push(FrameRequest::InspectMarkers(frames)),
                ViewportRequest::RenderThumbnail { part, path } => {
                    pending.push(FrameRequest::RenderThumbnail(part, path))
                }
                ViewportRequest::ExportImage { path, width, height } => {
                    pending.push(FrameRequest::ExportImage(path, width, height))
                }
            }
        }
        if !pending.is_empty() {
            app.world_mut().resource_mut::<PendingRequests>().0.extend(pending);
            dirty = true;
        }

        let motion = input_motion || animating;
        if motion {
            reduced_since = None;
        } else if let Some(since) = reduced_since
            && since.elapsed() >= SETTLE
        {
            // Motion has stopped: replace the soft reduced-resolution frame with a sharp one.
            dirty = true;
            reduced_since = None;
        }
        if !dirty || free.is_empty() {
            continue;
        }

        let scale = if motion { scaler.next_scale() } else { 1.0 };
        let size = (
            ((full_size.0 as f32 * scale).round() as u32).max(1),
            ((full_size.1 as f32 * scale).round() as u32).max(1),
        );
        let mut target = free.pop().expect("checked above");
        if (target.width(), target.height()) != size {
            target = create_texture(device, queue, size.0, size.1);
        }
        render_into(app, target, ManualTextureViewHandle(next_view_id));
        next_view_id = next_view_id.wrapping_add(1);

        let world = app.world_mut();
        world.resource_mut::<PendingRequests>().0.clear();
        let mut redraw = world.resource_mut::<RedrawState>();
        frames_rendered += 1;
        let keep_alive = std::mem::take(&mut redraw.keep_alive);
        animating = std::mem::take(&mut redraw.moving);
        // A few warm-up frames let GPU resources created at startup settle.
        dirty = keep_alive || animating || frames_rendered < WARMUP_FRAMES;
        loading = world.resource::<crate::glb::GlbLoading>().0;
        if scale < 1.0 {
            reduced_since = Some(Instant::now());
        }
        debug!("viewport frame {frames_rendered} rendered at {}x{} (scale {scale:.2})", size.0, size.1);
        events.send(ViewportEvent::FrameReady { frames_rendered });
    }
}

/// How often the main schedule is ticked while a GLB loads.
const LOAD_POLL: Duration = Duration::from_millis(30);

/// How long motion must stop before a full-resolution frame replaces the reduced one.
const SETTLE: Duration = Duration::from_millis(120);

/// Waits for the next request until `deadline`. `Ok(None)` means the deadline passed;
/// `Err(())` means the UI closed the channel.
fn recv_until(requests: &async_channel::Receiver<ViewportRequest>, deadline: Instant) -> Result<Option<ViewportRequest>, ()> {
    loop {
        match requests.try_recv() {
            Ok(request) => return Ok(Some(request)),
            Err(async_channel::TryRecvError::Closed) => return Err(()),
            Err(async_channel::TryRecvError::Empty) => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(4)));
    }
}

/// Picks the render scale for motion frames so a frame cycle (viewport render + UI composite)
/// stays near 60 Hz. GPU time is roughly proportional to pixel count, so the scale moves with
/// the square root of the time ratio. Scales are quantised to limit texture re-allocation.
struct ResolutionScaler {
    scale: f32,
    last_frame: Option<Instant>,
}

impl Default for ResolutionScaler {
    fn default() -> Self {
        Self { scale: 0.75, last_frame: None }
    }
}

impl ResolutionScaler {
    const TARGET: f32 = 1.0 / 60.0;
    const MIN: f32 = 0.4;
    const STEP: f32 = 0.05;

    fn next_scale(&mut self) -> f32 {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            let cycle = (now - last).as_secs_f32();
            // Longer gaps are pauses between interactions, not frame cycles.
            if cycle < 0.25 {
                self.adjust(cycle);
            }
        }
        self.last_frame = Some(now);
        self.scale
    }

    fn adjust(&mut self, cycle: f32) {
        let ideal = (self.scale * (Self::TARGET / cycle.max(1e-3)).sqrt()).clamp(Self::MIN, 1.0);
        let half_way = self.scale + (ideal - self.scale) * 0.5;
        let mut next = (half_way / Self::STEP).round() * Self::STEP;
        // Rounding can stall one step short of the ideal; always move at least one step when
        // the ideal is a step or more away, so both bounds stay reachable.
        if (ideal - self.scale).abs() >= Self::STEP && (next - self.scale).abs() < Self::STEP / 2.0 {
            next = self.scale + Self::STEP.copysign(ideal - self.scale);
        }
        self.scale = next.clamp(Self::MIN, 1.0);
    }
}

/// Full-resolution viewport size in physical pixels: the coordinate space of pointer input.
#[derive(Resource, Clone, Copy)]
pub(crate) struct ViewportSize(pub Vec2);

fn render_into(app: &mut App, target: wgpu::Texture, handle: ManualTextureViewHandle) {
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("lab-viewport frame view"),
        ..Default::default()
    });
    let world = app.world_mut();
    world.resource_mut::<BackBuffer>().0 = Some(target.clone());
    let mut views = world.resource_mut::<ManualTextureViews>();
    views.clear();
    views.insert(
        handle,
        ManualTextureView {
            texture_view: view.into(),
            size: (target.width(), target.height()).into(),
            view_format: TextureFormat::Rgba8UnormSrgb,
        },
    );
    world.resource_mut::<CurrentTarget>().0 = handle;
    world.resource_mut::<TargetSize>().0 = UVec2::new(target.width(), target.height());
    app.update();
}

/// Physical size of the texture this frame renders into (smaller than [`ViewportSize`] while
/// the adaptive scaler reduces resolution); the split view's pane rects come from it.
#[derive(Resource, Default)]
pub(crate) struct TargetSize(pub UVec2);

/// The texture view this frame renders into. Applied to every camera at the start of each
/// update, which also covers cameras spawned during startup.
#[derive(Resource)]
pub(crate) struct CurrentTarget(pub ManualTextureViewHandle);

fn apply_render_target(current: Res<CurrentTarget>, mut cameras: Query<&mut RenderTarget, (With<Camera3d>, Without<crate::thumbnail::ThumbCamera>, Without<crate::export::ExportCamera>)>) {
    for mut target in &mut cameras {
        *target = RenderTarget::TextureView(current.0);
    }
}

/// The texture the current frame renders into; handed to the UI once rendering finishes.
#[derive(Clone, Resource, ExtractResource, Deref, DerefMut)]
#[extract_app(RenderApp)]
struct BackBuffer(Option<wgpu::Texture>);

#[derive(Resource)]
struct FrameSender(async_channel::Sender<wgpu::Texture>);

struct SendFrameToUiPlugin(async_channel::Sender<wgpu::Texture>);

impl Plugin for SendFrameToUiPlugin {
    fn build(&self, app: &mut App) {
        let render_app = app.sub_app_mut(RenderApp);
        render_app.insert_resource(FrameSender(self.0.clone()));
        // After all cameras have rendered and been submitted.
        render_app.add_systems(RenderGraph, send_frame_to_ui.in_set(RenderGraphSystems::Finish));
    }
}

fn send_frame_to_ui(sender: Res<FrameSender>, back_buffer: Res<BackBuffer>) {
    if let Some(texture) = &back_buffer.0 {
        // A closed channel means the UI is shutting down.
        sender.0.send_blocking(texture.clone()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaler_shrinks_when_slow_and_grows_when_fast() {
        let mut scaler = ResolutionScaler::default();
        for _ in 0..20 {
            scaler.adjust(0.050);
        }
        assert!(scaler.scale <= 0.45, "slow frames shrink the scale, got {}", scaler.scale);
        for _ in 0..40 {
            scaler.adjust(0.005);
        }
        assert_eq!(scaler.scale, 1.0);
    }

    #[test]
    fn scaler_holds_at_target() {
        let mut scaler = ResolutionScaler { scale: 0.7, last_frame: None };
        scaler.adjust(ResolutionScaler::TARGET);
        assert!((scaler.scale - 0.7).abs() < 1e-6);
    }
}
