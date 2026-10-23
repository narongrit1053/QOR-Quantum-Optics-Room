// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app_state;
mod beams;
mod bom;
mod bridge;
mod clipboard;
mod constrain;
mod controller;
mod dialogs;
mod embed;
mod export_png;
mod file_drop;
mod files;
mod import;
mod library;
mod measure;
mod nudge;
mod overlay;
mod palette;
mod place;
mod transform;
mod stack;
mod thumbs;
mod scene;
mod ui_models;
mod view_cube;
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
    let has_recovery = !find_recoverable(&default_root(), None).is_empty();
    let (session, open_error) = files::initial_session(&args, &settings, has_recovery);
    let autosave = AutosaveSession::start(&default_root(), &session.title());
    if let Err(error) = &autosave {
        eprintln!("autosave unavailable: {error}");
    }
    let own_dir = autosave.as_ref().ok().map(|a| a.dir().to_path_buf());
    let recovery = find_recoverable(&default_root(), own_dir.as_deref());
    let autosave_available = autosave.is_ok();
    let io = IoWorker::start(autosave.ok(), bridge::io_result_sink());

    let ui = AppWindow::new()?;
    ui.global::<lab_ui::Theme>().set_mode(theme_index(settings.theme));
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
    export_png::connect(&ui, &app);
    library::connect(&ui, &app, &models);
    stack::connect(&ui, &app);
    scene::connect(&ui, &app, &models);
    file_drop::connect(&ui, &app, &models);
    controller::refresh_all(&ui, &app.borrow(), &models);
    files::show_recent(&ui, &app.borrow().settings);
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
                style_title_bar(ui.window(), ui.global::<lab_ui::Theme>().get_mode());
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
                a.borrow().send(ViewportRequest::SetBeams(a.borrow().settings.show_beams));
                let catalog = a.borrow().catalog.clone();
                a.borrow().send(ViewportRequest::SetCatalog(catalog));
                crate::thumbs::render_missing(&a.borrow());
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

fn theme_index(mode: lab_core::ThemeMode) -> i32 {
    match mode {
        lab_core::ThemeMode::Dark => 0,
        lab_core::ThemeMode::Light => 1,
        lab_core::ThemeMode::HighContrast => 2,
    }
}

/// Switches the UI chrome and the native title bar to `mode`; the caller saves the setting.
fn apply_theme(ui: &AppWindow, mode: lab_core::ThemeMode) {
    ui.global::<lab_ui::Theme>().set_mode(theme_index(mode));
    #[cfg(windows)]
    style_title_bar(ui.window(), theme_index(mode));
}

/// Makes the native title bar blend into the app: dark mode plus an explicit caption colour
/// equal to the window background. The explicit colour also wins over the "show accent colour
/// on title bars" setting. Windows 10 ignores the colour attributes and keeps dark mode only.
///
/// (A Mica backdrop was tried here but cannot show: the client area is fully covered by the
/// opaque 3D viewport, and an explicit caption colour replaces Mica in the title bar.)
#[cfg(windows)]
fn style_title_bar(window: &slint::Window, mode: i32) {
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
    // Mirrors Theme.window-bg and Theme.text-secondary for each mode (0 dark, 1 light, 2 contrast).
    let (dark, caption, text): (i32, u32, u32) = match mode {
        1 => (0, colorref(0xF5, 0xF5, 0xF7), colorref(0x6E, 0x6E, 0x73)),
        2 => (1, colorref(0, 0, 0), colorref(0xFF, 0xFF, 0xFF)),
        _ => (1, colorref(0x0B, 0x0C, 0x0E), colorref(0xA1, 0xA1, 0xA6)),
    };
    // SAFETY: `hwnd` is the live window handle; each attribute value is a 4-byte integer.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, (&raw const dark).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as _, (&raw const caption).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as _, (&raw const caption).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR as _, (&raw const text).cast(), 4);
    }
}
