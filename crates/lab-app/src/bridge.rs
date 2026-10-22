//! Viewport and I/O plumbing between worker threads and the UI thread (moved from M0 main.rs).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lab_file::IoResult;
use lab_ui::AppWindow;
use lab_viewport::{
    Modifiers, PointerButton, Vec2, ViewportEvent, ViewportHandle, ViewportInput, ViewportRequest, wgpu,
};
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::files;
use crate::ui_models::UiModels;

/// Timing log for diagnosing frame pacing; enabled with `LABSETUP_TRACE=1`.
pub fn trace(what: &str) {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("LABSETUP_TRACE").is_some()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let day = t.as_secs_f64() % 86_400.0;
        let (h, m, sec) = ((day / 3600.0) as u32, ((day % 3600.0) / 60.0) as u32, day % 60.0);
        eprintln!("{h:02}:{m:02}:{sec:09.6} {what}");
    }
}

/// The UI-thread handler for I/O worker results.
type IoHandler = Rc<dyn Fn(IoResult)>;

thread_local! {
    /// Drains and handles pending viewport events. Lives on the UI thread; run via [`wake_ui`].
    static VIEWPORT_EVENTS: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    /// Handles one I/O worker result. Lives on the UI thread; run via [`io_result_sink`].
    static IO_RESULTS: RefCell<Option<IoHandler>> = RefCell::new(None);
}

/// Called by the viewport thread after each event: runs the handler on the UI thread.
pub fn wake_ui() {
    slint::invoke_from_event_loop(|| {
        VIEWPORT_EVENTS.with(|handler| {
            if let Some(handler) = handler.borrow_mut().as_mut() {
                handler();
            }
        });
    })
    .ok();
}

/// Installs the handler for everything coming back from the viewport thread.
pub fn listen_to_viewport(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, handle: ViewportHandle) {
    let ui_weak = ui.as_weak();
    let app = app.clone();
    let models = models.clone();
    let handler = move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        while let Ok(event) = handle.events.try_recv() {
            match event {
                ViewportEvent::Ready { adapter, backend, quality, msaa_samples } => {
                    app.borrow_mut().viewport_ready = true;
                    ui.set_gpu(format!("{adapter} · {backend} · {quality:?}, MSAA {msaa_samples}×").into());
                }
                ViewportEvent::FrameReady { frames_rendered } => {
                    trace(&format!("ui: frame {frames_rendered} ready"));
                    show_newest_frame(&ui, &app, &handle.textures);
                    // Host's WARMUP_FRAMES: pipelines are compiled once the scene has rendered this many.
                    if frames_rendered >= 3 && ui.get_viewport_preparing() {
                        ui.set_viewport_preparing(false);
                    }
                    // Constant text while rendering: a changing label would cost a UI redraw per frame.
                    ui.set_rendering(true);
                    ui.set_status("Rendering".into());
                    let ui_weak = ui.as_weak();
                    app.borrow().timers.status.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(400),
                        move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_rendering(false);
                                ui.set_status(format!("Idle · {frames_rendered} frames").into());
                            }
                        },
                    );
                }
                ViewportEvent::SelectionChanged(target) => {
                    controller::on_viewport_selection(&ui, &app, &models, target);
                }
                ViewportEvent::HoverChanged(_) => {}
                ViewportEvent::IsolationChanged(id) => {
                    app.borrow_mut().isolated = id;
                    controller::refresh_isolated(&ui, &app.borrow());
                }
                ViewportEvent::Measured(m) => crate::measure::on_measured(&ui, &app, m),
                ViewportEvent::Overlay(items) => crate::overlay::on_overlay(&ui, &app, items),
                ViewportEvent::PlaceRequested(request) => {
                    crate::place::on_place_requested(&ui, &app, &models, request);
                }
                ViewportEvent::TransformRequested { id, parent, attach, position, rotation, done } => {
                    crate::transform::on_transform_requested(&ui, &app, &models, id, parent, attach, position, rotation, done);
                }
                ViewportEvent::PartPoint { point, normal } => crate::wizard::on_part_point(&ui, point, normal),
                ViewportEvent::ThumbnailReady { ok, .. } => {
                    if ok {
                        crate::library::fill(&app.borrow(), &models);
                    }
                }
                ViewportEvent::CameraOrient { yaw, pitch } => crate::view_cube::on_orient(&ui, yaw, pitch),
                ViewportEvent::ImageExported { path, ok } => crate::export_png::finished(&app, &path, ok),
                ViewportEvent::SceneError(message) => {
                    eprintln!("viewport: {message}");
                    controller::show_toast(&ui, &app.borrow(), &message, false);
                }
            }
        }
    };
    VIEWPORT_EVENTS.with(|slot| *slot.borrow_mut() = Some(Box::new(handler)));
    // Events sent before the handler existed (e.g. `Ready`) are handled now.
    wake_ui();
}

/// Shows the newest rendered texture and hands every older one back to the viewport.
pub fn show_newest_frame(ui: &AppWindow, app: &Rc<RefCell<AppState>>, textures: &async_channel::Receiver<wgpu::Texture>) {
    let requests = app.borrow().requests.clone();
    let release = |texture| {
        if let Some(requests) = &requests {
            requests.try_send(ViewportRequest::ReleaseTexture(texture)).ok();
        }
    };
    let mut newest = None;
    while let Ok(texture) = textures.try_recv() {
        if let Some(older) = newest.replace(texture) {
            release(older);
        }
    }
    let Some(texture) = newest else { return };
    let previous = ui.get_viewport_texture().to_wgpu_30_texture();
    match slint::Image::try_from(texture.clone()) {
        Ok(image) => {
            ui.set_viewport_texture(image);
            if let Some(previous) = previous {
                release(previous);
            }
        }
        Err(error) => {
            eprintln!("viewport frame could not be shown: {error:?}");
            release(texture);
        }
    }
}

/// Forwards pointer, scroll and leave events from the viewport area to the viewport thread.
pub fn connect_input(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_viewport_pointer(move |kind, button, x, y, shift, ctrl, alt| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let scale = ui.window().scale_factor();
        let pos = Vec2::new(x * scale, y * scale);
        let mods = Modifiers { shift, ctrl, alt };
        let button = match button {
            0 => PointerButton::Left,
            1 => PointerButton::Right,
            2 => PointerButton::Middle,
            _ => PointerButton::Other,
        };
        let input = match kind {
            0 => ViewportInput::PointerDown { button, pos, mods },
            1 => ViewportInput::PointerUp { button, pos, mods },
            2 => ViewportInput::PointerMove { pos, mods },
            _ => ViewportInput::PointerLeave,
        };
        a.borrow().send(ViewportRequest::Input(input));
    });

    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_viewport_scroll(move |dx, dy, x, y| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let scale = ui.window().scale_factor();
        a.borrow().send(ViewportRequest::Input(ViewportInput::Scroll {
            delta: Vec2::new(dx * scale, dy * scale),
            pos: Vec2::new(x * scale, y * scale),
            mods: Modifiers::default(),
        }));
    });

    let a = app.clone();
    ui.on_viewport_left(move || a.borrow().send(ViewportRequest::Input(ViewportInput::PointerLeave)));
}

/// Installs the UI-thread handler for I/O worker results (`files::handle_io_result`).
pub fn listen_to_io(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let ui_weak = ui.as_weak();
    let app = app.clone();
    let models = models.clone();
    let handler: IoHandler = Rc::new(move |result| {
        if let Some(ui) = ui_weak.upgrade() {
            files::handle_io_result(&ui, &app, &models, result);
        }
    });
    IO_RESULTS.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// The `on_result` callback for `IoWorker::start`: forwards each result to the UI thread.
/// Results arriving before [`listen_to_io`] ran are dropped (none can: jobs are submitted
/// only from the UI thread after it is installed).
pub fn io_result_sink() -> impl Fn(IoResult) + Send + 'static {
    |result| {
        slint::invoke_from_event_loop(move || {
            // Clone the handler out so no thread-local borrow is held while it runs.
            let handler = IO_RESULTS.with(|slot| slot.borrow().clone());
            if let Some(handler) = handler {
                handler(result);
            }
        })
        .ok();
    }
}
