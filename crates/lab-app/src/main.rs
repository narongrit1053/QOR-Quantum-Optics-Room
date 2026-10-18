// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lab_core::units::format_mm;
use lab_core::{Document, Finish, NodeId, Shape, sample::demo_document};
use lab_ui::{AppWindow, InspectorField, LibraryRow, SceneRow};
use lab_viewport::{
    Modifiers, PointerButton, Quality, Vec2, ViewMode, ViewportEvent, ViewportHandle, ViewportInput, ViewportRequest, wgpu,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// UI-thread state. The document lives here; the viewport only ever gets snapshots.
struct App {
    doc: Document,
    selection: Option<NodeId>,
    requests: Option<async_channel::Sender<ViewportRequest>>,
    viewport_size: (u32, u32),
}

impl App {
    fn send(&self, request: ViewportRequest) {
        if let Some(requests) = &self.requests {
            // Unbounded: never blocks the UI thread. Fails only during shutdown.
            requests.try_send(request).ok();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The UI and the viewport share one wgpu device, so frames are composited without copies.
    let mut settings = slint::wgpu_30::WGPUSettings::default();
    settings.device_required_limits =
        wgpu::Limits::default().using_resolution(wgpu::Limits::downlevel_defaults());
    // Skia's wgpu interop on Windows supports Direct3D 12 only; automatic selection may pick
    // Vulkan and then fail at startup.
    #[cfg(windows)]
    {
        settings.backends = wgpu::Backends::DX12;
    }
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(settings))
        .select()?;

    let ui = AppWindow::new()?;
    let app = Rc::new(RefCell::new(App {
        doc: demo_document(),
        selection: None,
        requests: None,
        viewport_size: (0, 0),
    }));

    populate_static(&ui, &app.borrow().doc);
    refresh_selection(&ui, &app.borrow());
    connect_callbacks(&ui, &app);

    let idle_timer = Rc::new(slint::Timer::default());
    let ui_weak = ui.as_weak();
    let app_for_setup = app.clone();
    ui.window().set_rendering_notifier(move |state, graphics_api| {
        let Some(ui) = ui_weak.upgrade() else { return };
        match state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::WGPU30 { instance, device, queue, .. } = graphics_api else {
                    return;
                };
                #[cfg(windows)]
                style_title_bar(ui.window());
                let snapshot = app_for_setup.borrow().doc.clone();
                // Automatic unless overridden (until there is a preferences UI).
                let quality = match std::env::var("LABSETUP_QUALITY").as_deref() {
                    Ok("low") => Some(Quality::Low),
                    Ok("balanced") => Some(Quality::Balanced),
                    Ok("high") => Some(Quality::High),
                    _ => None,
                };
                let handle =
                    lab_viewport::start(instance.clone(), device.clone(), queue.clone(), snapshot, quality, wake_ui);
                app_for_setup.borrow_mut().requests = Some(handle.requests.clone());
                listen_to_viewport(&ui, &app_for_setup, handle, idle_timer.clone());
            }
            slint::RenderingState::BeforeRendering => {
                trace("slint: before rendering");
                // Keep the viewport's render size in sync with its on-screen size.
                let scale = ui.window().scale_factor();
                let size = (
                    (ui.get_viewport_width() * scale).round() as u32,
                    (ui.get_viewport_height() * scale).round() as u32,
                );
                let mut app = app_for_setup.borrow_mut();
                if size.0 > 0 && size.1 > 0 && size != app.viewport_size {
                    app.viewport_size = size;
                    app.send(ViewportRequest::Resize { width: size.0, height: size.1 });
                }
            }
            slint::RenderingState::AfterRendering => trace("slint: after rendering"),
            _ => {}
        }
    })?;

    ui.run()?;
    Ok(())
}

/// Timing log for diagnosing frame pacing; enabled with `LABSETUP_TRACE=1`.
fn trace(what: &str) {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("LABSETUP_TRACE").is_some()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let day = t.as_secs_f64() % 86_400.0;
        let (h, m, sec) = ((day / 3600.0) as u32, ((day % 3600.0) / 60.0) as u32, day % 60.0);
        eprintln!("{h:02}:{m:02}:{sec:09.6} {what}");
    }
}

thread_local! {
    /// Drains and handles pending viewport events. Lives on the UI thread; run via [`wake_ui`].
    static VIEWPORT_EVENTS: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
}

/// Called by the viewport thread after each event: runs the handler on the UI thread.
fn wake_ui() {
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
fn listen_to_viewport(ui: &AppWindow, app: &Rc<RefCell<App>>, handle: ViewportHandle, idle_timer: Rc<slint::Timer>) {
    let ui_weak = ui.as_weak();
    let app = app.clone();
    let handler = move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        while let Ok(event) = handle.events.try_recv() {
            match event {
                ViewportEvent::Ready { adapter, backend, quality, msaa_samples } => {
                    ui.set_gpu(format!("{adapter} · {backend} · {quality:?}, MSAA {msaa_samples}×").into());
                }
                ViewportEvent::FrameReady { frames_rendered } => {
                    trace(&format!("ui: frame {frames_rendered} ready"));
                    show_newest_frame(&ui, &app.borrow(), &handle.textures);
                    // Constant text while rendering: a changing label would cost a UI redraw per frame.
                    ui.set_rendering(true);
                    ui.set_status("Rendering".into());
                    let ui_weak = ui.as_weak();
                    idle_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(400), move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_rendering(false);
                            ui.set_status(format!("Idle · {frames_rendered} frames").into());
                        }
                    });
                }
                ViewportEvent::SelectionChanged(id) => {
                    app.borrow_mut().selection = id;
                    refresh_selection(&ui, &app.borrow());
                }
                ViewportEvent::HoverChanged(_) => {}
            }
        }
    };
    VIEWPORT_EVENTS.with(|slot| *slot.borrow_mut() = Some(Box::new(handler)));
    // Events sent before the handler existed (e.g. `Ready`) are handled now.
    wake_ui();
}

/// Shows the newest rendered texture and hands every older one back to the viewport.
fn show_newest_frame(ui: &AppWindow, app: &App, textures: &async_channel::Receiver<wgpu::Texture>) {
    let mut newest = None;
    while let Ok(texture) = textures.try_recv() {
        if let Some(older) = newest.replace(texture) {
            app.send(ViewportRequest::ReleaseTexture(older));
        }
    }
    let Some(texture) = newest else { return };
    let previous = ui.get_viewport_texture().to_wgpu_30_texture();
    match slint::Image::try_from(texture.clone()) {
        Ok(image) => {
            ui.set_viewport_texture(image);
            if let Some(previous) = previous {
                app.send(ViewportRequest::ReleaseTexture(previous));
            }
        }
        Err(error) => {
            eprintln!("viewport frame could not be shown: {error:?}");
            app.send(ViewportRequest::ReleaseTexture(texture));
        }
    }
}

fn connect_callbacks(ui: &AppWindow, app: &Rc<RefCell<App>>) {
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

    let a = app.clone();
    ui.on_view_mode_requested(move |mode| {
        let mode = if mode == 1 { ViewMode::Top } else { ViewMode::Perspective };
        a.borrow().send(ViewportRequest::SetViewMode(mode));
    });

    let a = app.clone();
    ui.on_frame_all(move || a.borrow().send(ViewportRequest::FrameAll));

    let a = app.clone();
    ui.on_scene_row_clicked(move |id| a.borrow().send(ViewportRequest::Select(Some(NodeId(id as u64)))));

    let a = app.clone();
    ui.on_clear_selection(move || a.borrow().send(ViewportRequest::Select(None)));
}

fn populate_static(ui: &AppWindow, doc: &Document) {
    ui.set_project_name(doc.name.clone().into());
    let tables = doc.platforms.len();
    let parts = doc.nodes.iter().filter(|n| n.pickable).count();
    ui.set_project_detail(format!("{tables} table · {parts} parts · offline").into());

    let accent = |r, g, b| slint::Color::from_rgb_u8(r, g, b);
    let library = [
        ("Tables & breadboards", "T", accent(100, 210, 255)),
        ("Posts & holders", "P", accent(191, 90, 242)),
        ("Mounts", "M", accent(10, 132, 255)),
        ("Optics", "O", accent(48, 209, 88)),
        ("Lasers", "L", accent(255, 55, 95)),
        ("Detectors", "D", accent(255, 159, 10)),
    ]
    .into_iter()
    .map(|(label, letter, tint)| LibraryRow { label: label.into(), letter: letter.into(), tint })
    .collect::<Vec<_>>();
    ui.set_library(ModelRc::new(VecModel::from(library)));
}

/// Groups nodes named "M1 · post", "M1 · mount", "M1" under one "M1" row.
fn scene_rows(doc: &Document, selection: Option<NodeId>) -> Vec<SceneRow> {
    let mut groups: Vec<(&str, Vec<&lab_core::Node>)> = Vec::new();
    for node in &doc.nodes {
        let key = node.name.split(" · ").next().unwrap_or(&node.name);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(node),
            None => groups.push((key, vec![node])),
        }
    }

    let mut rows = Vec::new();
    for (key, members) in groups {
        let head = members.iter().find(|n| n.name == key).copied();
        let is_beam = members.iter().all(|n| matches!(n.finish, Finish::Beam { .. }));
        let head_id = head.or(members.first().copied()).map(|n| n.id);
        rows.push(SceneRow {
            id: head_id.map_or(-1, |id| id.0 as i32),
            label: key.into(),
            detail: if members.len() > 1 { SharedString::from(format!("{}", members.len())) } else { "".into() },
            depth: 0,
            selected: head.is_some() && head_id == selection,
            is_beam,
        });
        for node in members.iter().filter(|n| n.name != key) {
            let label = node.name.split(" · ").nth(1).unwrap_or(&node.name);
            rows.push(SceneRow {
                id: node.id.0 as i32,
                label: label.into(),
                detail: "".into(),
                depth: 1,
                selected: Some(node.id) == selection,
                is_beam: matches!(node.finish, Finish::Beam { .. }),
            });
        }
    }
    rows
}

fn refresh_selection(ui: &AppWindow, app: &App) {
    let rows = scene_rows(&app.doc, app.selection);
    let model = ui.get_scene();
    // Update in place when the shape is unchanged so the list keeps its scroll position.
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        ui.set_scene(ModelRc::new(VecModel::from(rows)));
    }

    let node = app.selection.and_then(|id| app.doc.node(id));
    ui.set_has_selection(node.is_some());
    let Some(node) = node else { return };
    ui.set_selection_title(node.name.clone().into());
    ui.set_selection_detail(node.part.clone().into());

    let platform = app.doc.platform(node.platform);
    let hole = platform.and_then(|p| {
        let grid = p.hole_grid.as_ref()?;
        let hole = grid.nearest_hole(p.size, node.position.truncate())?;
        let centre = grid.hole_position(p.size, hole)?;
        (centre.distance(node.position.truncate()) < 0.5).then(|| hole.to_string())
    });
    let (yaw, _, _) = node.rotation.to_euler(lab_core::glam::EulerRot::ZYX);
    let size = match &node.shape {
        Shape::Box { size } => format!("{:.1} × {:.1} × {:.1} mm", size.x, size.y, size.z),
        Shape::Cylinder { radius, length } => format!("Ø{:.1} × {:.1} mm", radius * 2.0, length),
    };
    let finish = match &node.finish {
        Finish::AnodizedBlack => "Black anodized".to_string(),
        Finish::Stainless => "Stainless steel".to_string(),
        Finish::Mirror => "Mirror coating".to_string(),
        Finish::Glass => "Glass".to_string(),
        Finish::Painted { .. } => "Painted".to_string(),
        Finish::Beam { wavelength_nm } => format!("Beam, {wavelength_nm:.0} nm"),
    };
    let fields = [
        ("Platform", platform.map_or("—".to_string(), |p| p.name.clone())),
        ("Hole", hole.unwrap_or_else(|| "—".to_string())),
        ("X", format_mm(node.position.x)),
        ("Y", format_mm(node.position.y)),
        ("Height", format_mm(node.position.z)),
        ("Rotation", format!("{:.1}°", yaw.to_degrees())),
        ("Size", size),
        ("Finish", finish),
    ]
    .into_iter()
    .map(|(label, value)| InspectorField { label: label.into(), value: value.into() })
    .collect::<Vec<_>>();
    ui.set_inspector(ModelRc::new(VecModel::from(fields)));
}

/// Makes the native title bar blend into the app: dark mode plus an explicit caption colour
/// equal to the window background. The explicit colour also wins over the "show accent colour
/// on title bars" setting. Windows 10 ignores the colour attributes and keeps dark mode only.
///
/// (A Mica backdrop was tried here but cannot show: the client area is fully covered by the
/// opaque 3D viewport, and an explicit caption colour replaces Mica in the title bar.)
#[cfg(windows)]
fn style_title_bar(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmSetWindowAttribute,
    };

    // COLORREF is 0x00BBGGRR.
    const fn colorref(r: u8, g: u8, b: u8) -> u32 {
        (b as u32) << 16 | (g as u32) << 8 | r as u32
    }

    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let dark: i32 = 1;
    let caption = colorref(0x0B, 0x0C, 0x0E); // Theme.window-bg
    let text = colorref(0xA1, 0xA1, 0xA6); // Theme.text-secondary
    // SAFETY: `hwnd` is the live window handle; each attribute value is a 4-byte integer.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, (&raw const dark).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as _, (&raw const caption).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as _, (&raw const caption).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR as _, (&raw const text).cast(), 4);
    }
}
