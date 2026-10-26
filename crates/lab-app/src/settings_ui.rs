//! Settings window (POLA-155): render quality, GPU, anti-aliasing, shadows. Every choice is
//! saved to the settings file and applied at the next start; the wgpu device is never rebuilt live.

use std::cell::RefCell;
use std::rc::Rc;

use lab_file::{GpuChoice, MsaaChoice, RenderQuality};
use lab_ui::AppWindow;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app_state::AppState;

const QUALITIES: [RenderQuality; 4] = [RenderQuality::Auto, RenderQuality::High, RenderQuality::Balanced, RenderQuality::Low];
const MSAA: [MsaaChoice; 5] = [MsaaChoice::Auto, MsaaChoice::Off, MsaaChoice::X2, MsaaChoice::X4, MsaaChoice::X8];
const SHADOWS: [Option<bool>; 3] = [None, Some(true), Some(false)];

/// GPU rows after Auto / High performance / Power saving: enumerated adapter names, plus a saved
/// name that is not present (so reopening the sheet keeps it).
fn adapter_names(app: &AppState) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (name, _) in &app.adapters {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    if let GpuChoice::Named(saved) = &app.settings.gpu
        && !names.contains(saved)
    {
        names.push(saved.clone());
    }
    names
}

pub fn open(ui: &AppWindow, app: &AppState) {
    let names = adapter_names(app);
    let kind = |name: &String| {
        app.adapters.iter().find(|(n, _)| n == name).map(|(_, t)| format!(" ({t:?})")).unwrap_or_default()
    };
    let rows: Vec<SharedString> = ["Auto (prefer discrete)".to_string(), "High performance".into(), "Power saving".into()]
        .into_iter()
        .chain(names.iter().map(|n| format!("{n}{}", kind(n))))
        .map(Into::into)
        .collect();
    let s = &app.settings;
    let gpu = match &s.gpu {
        GpuChoice::Auto => 0,
        GpuChoice::HighPerformance => 1,
        GpuChoice::PowerSaving => 2,
        GpuChoice::Named(n) => 3 + names.iter().position(|x| x == n).unwrap_or(0) as i32,
    };
    ui.set_settings_gpus(ModelRc::new(VecModel::from(rows)));
    ui.set_settings_quality(QUALITIES.iter().position(|q| *q == s.render_quality).unwrap_or(0) as i32);
    ui.set_settings_msaa(MSAA.iter().position(|m| *m == s.msaa).unwrap_or(0) as i32);
    ui.set_settings_shadows(SHADOWS.iter().position(|v| *v == s.shadows).unwrap_or(0) as i32);
    ui.set_settings_gpu(gpu);
    ui.set_settings_open(true);
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_settings_apply(move |quality, msaa, shadows, gpu| {
        let Some(ui) = w.upgrade() else { return };
        ui.set_settings_open(false);
        let (settings, changed, before_restart) = {
            let mut app = a.borrow_mut();
            let names = adapter_names(&app);
            let s = &mut app.settings;
            let before = (s.render_quality, s.msaa, s.shadows, s.gpu.clone());
            let before_restart = (s.render_quality, s.msaa, s.gpu.clone());
            s.render_quality = QUALITIES.get(quality as usize).copied().unwrap_or_default();
            s.msaa = MSAA.get(msaa as usize).copied().unwrap_or_default();
            s.shadows = SHADOWS.get(shadows as usize).copied().flatten();
            s.gpu = match gpu {
                1 => GpuChoice::HighPerformance,
                2 => GpuChoice::PowerSaving,
                i if i >= 3 => names.get(i as usize - 3).cloned().map(GpuChoice::Named).unwrap_or_default(),
                _ => GpuChoice::Auto,
            };
            let changed = before != (s.render_quality, s.msaa, s.shadows, s.gpu.clone());
            (s.clone(), changed, before_restart)
        };
        if !changed {
            return;
        }
        if let Err(error) = lab_file::save_settings(&settings) {
            eprintln!("settings not saved: {error}");
            crate::controller::show_toast(&ui, &a.borrow(), "Settings could not be saved; see the log.", false);
            return;
        }
        // Shadows apply live; quality, anti-aliasing and GPU are fixed when the viewport starts.
        a.borrow().send(lab_viewport::ViewportRequest::SetShadows(settings.shadows));
        if before_restart != (settings.render_quality, settings.msaa, settings.gpu.clone()) {
            crate::controller::show_toast(&ui, &a.borrow(), "Restart LabSetup to apply graphics changes.", false);
        }
    });
}
