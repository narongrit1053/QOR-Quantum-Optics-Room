//! "Import STEP…" flow (M4-PLAN POLA-59): file dialog → form → `import_step` on a worker
//! thread → part definition in the user library.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lab_core::Finish;
use lab_core::glam::DVec3;
use lab_core::part::{ConnectorDef, ConnectorKind, Frame, Geometry, PartDef};
use lab_import::{ImportOptions, Progress, SourceUp};
use lab_ui::AppWindow;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::dialogs::{DialogKind, DialogResult, owner_hwnd, show_file_dialog_local};
use crate::library;
use crate::ui_models::UiModels;

type Handles = (Rc<RefCell<AppState>>, Rc<UiModels>);

const VENDORS: [&str; 4] = ["Thorlabs", "MOGLabs", "Toptica", "Other"];
const FINISHES: [&str; 3] = ["anodized_black", "stainless", "painted"];
const PAINT: [u8; 3] = [210, 210, 205];

/// Bumped by every start and cancel; a worker whose generation is stale is ignored.
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The STEP file of the open dialog.
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// App handles for worker results, which arrive on the UI thread without them.
    static HANDLES: RefCell<Option<Handles>> = const { RefCell::new(None) };
}

/// The `Finish` stored in an imported part's `meta["finish"]` (AnodizedBlack if unknown).
pub fn finish_from_meta(value: &str) -> Finish {
    match value {
        "stainless" => Finish::Stainless,
        "painted" => Finish::Painted { rgb: PAINT },
        _ => Finish::AnodizedBlack,
    }
}

fn finish_of(index: i32) -> Finish {
    finish_from_meta(FINISHES.get(index as usize).copied().unwrap_or(FINISHES[0]))
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    HANDLES.with(|h| *h.borrow_mut() = Some((app.clone(), models.clone())));

    let w = ui.as_weak();
    ui.on_import_cancel(move || {
        GENERATION.fetch_add(1, Ordering::SeqCst);
        if let Some(ui) = w.upgrade() {
            ui.set_import_step(0);
        }
    });

    let w = ui.as_weak();
    ui.on_import_start(move || {
        if let Some(ui) = w.upgrade() {
            start(&ui);
        }
    });

    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_import_define_connectors(move |id| {
        if let Some(ui) = w.upgrade() {
            ui.set_import_step(0);
            crate::wizard::open(&ui, &a.borrow(), &id);
        }
    });
}

/// "Import STEP…" and "Choose another file": the native open dialog, then the form.
pub fn choose_file(ui: &AppWindow, _app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>) {
    let w = ui.as_weak();
    show_file_dialog_local(owner_hwnd(ui.window()), DialogKind::OpenStep, None, move |result| {
        let Some(ui) = w.upgrade() else { return };
        match result {
            DialogResult::Chosen(path) => {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                // Thorlabs names its files "<part>-Step.step" with '/' written as '_'.
                let part = stem.trim_end_matches("-Step").trim_end_matches("-step").replace('_', "/");
                ui.set_import_file(path.file_name().unwrap_or_default().to_string_lossy().as_ref().into());
                ui.set_import_part_number(part.as_str().into());
                ui.set_import_name(part.as_str().into());
                if ui.get_import_category().is_empty() {
                    ui.set_import_category("imported".into());
                }
                FILE.with(|f| *f.borrow_mut() = Some(path));
                ui.set_import_step(1);
            }
            DialogResult::Cancelled => {}
            DialogResult::Failed(text) => {
                ui.set_import_error(text.into());
                ui.set_import_step(3);
            }
        }
    });
}

fn start(ui: &AppWindow) {
    let Some(step) = FILE.with(|f| f.borrow().clone()) else { return };
    let Some(library) = lab_catalog::user_dir() else {
        ui.set_import_error("The user library folder (%APPDATA%\\LabSetup\\library) is not available.".into());
        ui.set_import_step(3);
        return;
    };
    let vendor = VENDORS[ui.get_import_vendor().clamp(0, 3) as usize].to_string();
    let part_number = ui.get_import_part_number().trim().to_string();
    let name = ui.get_import_name().trim().to_string();
    let category = ui.get_import_category().trim().trim_matches('/').to_string();
    let finish_index = ui.get_import_finish();
    let opts = ImportOptions {
        source_up: if ui.get_import_up() == 0 { SourceUp::Y } else { SourceUp::Z },
        finish: Some(finish_of(finish_index)),
        ..Default::default()
    };
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    ui.set_import_progress("Converting…".into());
    ui.set_import_step(2);

    let w = ui.as_weak();
    std::thread::spawn(move || {
        let progress = |p: Progress| {
            let text = match p {
                Progress::Converting => "Converting…",
                Progress::Simplifying => "Simplifying…",
                Progress::Storing => "Saving…",
            };
            let w = w.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade().filter(|_| GENERATION.load(Ordering::SeqCst) == generation) {
                    ui.set_import_progress(text.into());
                }
            })
            .ok();
        };
        let result = lab_import::import_step(&step, &library, &opts, &progress).and_then(|imported| {
            let def = PartDef {
                id: format!("{}:{}", vendor.to_lowercase(), part_number),
                version: 1,
                vendor,
                part_number,
                name,
                category: if category.is_empty() { "imported".into() } else { category },
                tags: Vec::new(),
                geometry: Geometry::Mesh { asset: imported.asset.clone() },
                connectors: vec![ConnectorDef {
                    name: "seat".into(),
                    kind: ConnectorKind::HoleGridSeat,
                    frame: Frame { origin: DVec3::ZERO, z: DVec3::NEG_Z, x: DVec3::X },
                }],
                optical_ports: Vec::new(),
                meta: BTreeMap::from([
                    ("finish".to_string(), FINISHES[finish_index.clamp(0, 2) as usize].to_string()),
                    ("source".to_string(), step.file_name().unwrap_or_default().to_string_lossy().into_owned()),
                ]),
            };
            lab_import::write_part(&library, &def)?;
            Ok((def, imported))
        });
        let message = match result {
            Ok((def, imported)) => Ok((
                def.id.clone(),
                def.category.clone(),
                format!(
                    "{} is in the library under {}. {:.1} × {:.1} × {:.1} mm, {} triangles{}.",
                    def.name,
                    def.category,
                    imported.size_mm.x,
                    imported.size_mm.y,
                    imported.size_mm.z,
                    imported.triangles[0],
                    if imported.has_colours { "" } else { ", finish applied" }
                ),
            )),
            Err(error) => Err(error.to_string()),
        };
        slint::invoke_from_event_loop(move || {
            if GENERATION.load(Ordering::SeqCst) != generation {
                return; // cancelled: the result is dropped
            }
            let Some(ui) = w.upgrade() else { return };
            finish(&ui, message);
        })
        .ok();
    });
}

fn finish(ui: &AppWindow, message: Result<(String, String, String), String>) {
    match message {
        Ok((id, category, text)) => {
            HANDLES.with(|h| {
                if let Some((app, models)) = &*h.borrow() {
                    let mut a = app.borrow_mut();
                    // Shows the new part: its group expanded.
                    a.library.collapsed.remove(category.split('/').next().unwrap_or(""));
                    library::reload_catalog(&mut a, models);
                    crate::thumbs::render(&a, &id);
                    controller::show_toast(ui, &a, &format!("Imported {id}"), false);
                }
            });
            ui.set_import_part_id(id.into());
            ui.set_import_done_text(text.into());
            ui.set_import_step(4);
        }
        Err(text) => {
            ui.set_import_error(text.into());
            ui.set_import_step(3);
        }
    }
}
