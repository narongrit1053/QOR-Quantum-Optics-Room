// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod bridge;
mod controller;
mod dialogs;
mod files;
mod import;
mod library;
mod measure;
mod overlay;
mod place;
mod transform;
mod stack;
mod scene;
mod ui_models;
mod wizard;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use lab_file::{AutosaveSession, IoWorker, default_root, find_recoverable, load_settings};
use lab_ui::AppWindow;
use lab_viewport::{Quality, ViewportRequest, wgpu};
use slint::ComponentHandle;

use crate::app_state::{AppState, SheetKind};
use crate::bridge::trace;
use crate::ui_models::UiModels;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The UI and the viewport share one wgpu device, so frames are composited without copies.
    let mut wgpu_settings = slint::wgpu_30::WGPUSettings::default();
    wgpu_settings.device_required_limits =
        wgpu::Limits::default().using_resolution(wgpu::Limits::downlevel_defaults());
    // Skia's wgpu interop on Windows supports Direct3D 12 only; automatic selection may pick
    // Vulkan and then fail at startup.
    #[cfg(windows)]
    {
        wgpu_settings.backends = wgpu::Backends::DX12;
    }
    // Prefer a discrete GPU (for example the NVIDIA one of a hybrid laptop); wgpu falls back
    // to the integrated GPU, then to a software adapter, when there is none.
    // `WGPU_POWER_PREF=low|none` still overrides this, `WGPU_ADAPTER_NAME` picks one by name.
    wgpu_settings.power_preference =
        wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance);
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(wgpu_settings))
        .select()?;

    // Startup (§7.6): settings, the initial document, this run's autosave directory, crash
    // candidates of other runs, the I/O worker.
    let settings = load_settings();
    let args: Vec<String> = std::env::args().collect();
    let (session, open_error) = files::initial_session(&args, &settings);
    let autosave = AutosaveSession::start(&default_root(), &session.title());
    if let Err(error) = &autosave {
        eprintln!("autosave unavailable: {error}");
    }
    let own_dir = autosave.as_ref().ok().map(|a| a.dir().to_path_buf());
    let recovery = find_recoverable(&default_root(), own_dir.as_deref());
    let autosave_available = autosave.is_ok();
    let io = IoWorker::start(autosave.ok(), bridge::io_result_sink());

    let ui = AppWindow::new()?;
    let models = UiModels::install(&ui);
    let wall_display = settings.wall_display;
    let mut state = AppState::new(session, settings);
    state.io = Some(io);
    state.autosave_available = autosave_available;
    state.recovery = recovery;
    let (catalog, warnings) = lab_catalog::Catalog::load_default();
    for w in &warnings {
        eprintln!("library: {}: {}", w.path.display(), w.message);
    }
    state.catalog = std::sync::Arc::new(catalog);
    let app = Rc::new(RefCell::new(state));

    bridge::listen_to_io(&ui, &app, &models);
    bridge::connect_input(&ui, &app);
    controller::connect(&ui, &app, &models);
    library::connect(&ui, &app, &models);
    stack::connect(&ui, &app);
    scene::connect(&ui, &app, &models);
    controller::refresh_all(&ui, &app.borrow(), &models);
    library::fill(&app.borrow(), &models);
    stack::fill(&ui, &mut app.borrow_mut());
    if !warnings.is_empty() {
        let text = format!("{} library files could not be read; see the log.", warnings.len());
        controller::show_toast(&ui, &app.borrow(), &text, false);
    }
    if !autosave_available {
        ui.set_status("Autosave unavailable".into());
    }

    let ui_weak = ui.as_weak();
    let (a, m) = (app.clone(), models.clone());
    ui.window().on_close_requested(move || match ui_weak.upgrade() {
        Some(ui) => files::on_close_requested(&ui, &a, &m),
        None => slint::CloseRequestResponse::HideWindow,
    });

    let ui_weak = ui.as_weak();
    let (a, m) = (app.clone(), models.clone());
    let first_frame = Cell::new(true);
    let open_error = RefCell::new(open_error);
    ui.window().set_rendering_notifier(move |state, graphics_api| {
        let Some(ui) = ui_weak.upgrade() else { return };
        match state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::WGPU30 { instance, device, queue, .. } = graphics_api else {
                    return;
                };
                #[cfg(windows)]
                style_title_bar(ui.window());
                let snapshot = a.borrow().session.doc.clone();
                // Automatic unless overridden (until there is a preferences UI).
                let quality = match std::env::var("LABSETUP_QUALITY").as_deref() {
                    Ok("low") => Some(Quality::Low),
                    Ok("balanced") => Some(Quality::Balanced),
                    Ok("high") => Some(Quality::High),
                    _ => None,
                };
                let handle = lab_viewport::start(
                    instance.clone(),
                    device.clone(),
                    queue.clone(),
                    snapshot,
                    quality,
                    bridge::wake_ui,
                );
                a.borrow_mut().requests = Some(handle.requests.clone());
                a.borrow().send(ViewportRequest::SetWallDisplay(wall_display));
                let catalog = a.borrow().catalog.clone();
                a.borrow().send(ViewportRequest::SetCatalog(catalog));
                bridge::listen_to_viewport(&ui, &a, &m, handle);
            }
            slint::RenderingState::BeforeRendering => {
                trace("slint: before rendering");
                // Keep the viewport's render size in sync with its on-screen size.
                let scale = ui.window().scale_factor();
                let size = (
                    (ui.get_viewport_width() * scale).round() as u32,
                    (ui.get_viewport_height() * scale).round() as u32,
                );
                let mut app = a.borrow_mut();
                if size.0 > 0 && size.1 > 0 && size != app.viewport_size {
                    app.viewport_size = size;
                    app.send(ViewportRequest::Resize { width: size.0, height: size.1 });
                }
            }
            slint::RenderingState::AfterRendering => {
                trace("slint: after rendering");
                if first_frame.replace(false) {
                    // Sheets appear once the window is on screen; deferred so no Slint call
                    // re-enters the renderer.
                    let ui_weak = ui.as_weak();
                    let (a, m) = (a.clone(), m.clone());
                    let error = open_error.borrow_mut().take();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        let Some(ui) = ui_weak.upgrade() else { return };
                        if let Some(error) = error {
                            controller::show_sheet(
                                &ui,
                                &mut a.borrow_mut(),
                                SheetKind::Message,
                                "Couldn't open the project",
                                &error,
                                &["OK"],
                                0,
                                -1,
                            );
                        }
                        files::startup(&ui, &a, &m);
                    });
                }
            }
            _ => {}
        }
    })?;

    ui.run()?;
    Ok(())
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
