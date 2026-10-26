//! "Import STEP…" flow (M4-PLAN POLA-59): file dialog → form → `import_step` on a worker
//! thread → part definition in the user library.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lab_core::Finish;
use lab_core::glam::DVec3;
use lab_core::part::{ConnectorDef, ConnectorKind, Feature, Frame, Gender, Geometry, PartDef};
use lab_geom::TriMesh;
use lab_import::{ImportOptions, Progress, SourceUp};
use lab_ui::AppWindow;
use lab_viewport::{DragItem, Modifiers, PartDrag, Vec2, ViewportRequest};
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
    /// Files still to import after the current one (a multi-file "Import STEP…").
    static QUEUE: RefCell<std::collections::VecDeque<PathBuf>> = const { RefCell::new(std::collections::VecDeque::new()) };
    /// The running "Import Folder into Library…" batch (POLA-163).
    static BATCH: RefCell<Option<Batch>> = const { RefCell::new(None) };
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
        QUEUE.with(|q| q.borrow_mut().clear());
        if let Some(ui) = w.upgrade() {
            // A cancelled batch keeps what it imported: a second run resumes.
            end_batch(&ui);
            ui.set_import_step(0);
        }
    });

    let w = ui.as_weak();
    ui.on_import_download(move || {
        if let Some(ui) = w.upgrade() {
            download(&ui);
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
    show_file_dialog_local(owner_hwnd(ui.window()), DialogKind::OpenSteps, None, move |result| {
        let Some(ui) = w.upgrade() else { return };
        match result {
            DialogResult::Chosen(path) => {
                prefill(&ui, path);
                ui.set_import_step(1);
            }
            DialogResult::ChosenMany(mut paths) if paths.len() == 1 => {
                prefill(&ui, paths.remove(0));
                ui.set_import_step(1);
            }
            // Several files: imported one after another with the form's defaults, no form.
            DialogResult::ChosenMany(paths) => {
                let mut paths = std::collections::VecDeque::from(paths);
                let first = paths.pop_front();
                QUEUE.with(|q| *q.borrow_mut() = paths);
                if let Some(first) = first {
                    prefill(&ui, first);
                    start(&ui);
                }
            }
            DialogResult::Cancelled => {}
            DialogResult::Failed(text) => {
                ui.set_import_error(text.into());
                ui.set_import_step(3);
            }
        }
    });
}

/// "Import from URL…" (POLA-170): the URL step of the import dialog.
pub fn choose_url(ui: &AppWindow) {
    ui.set_import_step(5);
}

/// Downloads the dialog's URL on a worker thread, then opens the normal form on the file.
fn download(ui: &AppWindow) {
    let url = ui.get_import_url().to_string();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    ui.set_import_progress("Downloading…".into());
    ui.set_import_step(2);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let result = crate::import_url::download(&url);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            match result {
                Ok(path) => {
                    prefill(&ui, path);
                    ui.set_import_step(1);
                }
                Err(text) => {
                    ui.set_import_error(text.into());
                    ui.set_import_step(3);
                }
            }
        });
    });
}

/// Fills the form from a STEP file name and makes it the file to import.
fn prefill(ui: &AppWindow, path: PathBuf) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // Thorlabs names its files "<part>-Step.step" with '/' written as '_'.
    let part = stem.trim_end_matches("-Step").trim_end_matches("-step").replace('_', "/");
    ui.set_import_file(path.file_name().unwrap_or_default().to_string_lossy().as_ref().into());
    ui.set_import_part_number(part.as_str().into());
    ui.set_import_name(part.as_str().into());
    if ui.get_import_category().is_empty() {
        ui.set_import_category(category_of(&part).into());
    }
    FILE.with(|f| *f.borrow_mut() = Some(path));
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
        let result = convert(&step, &library, Spec { vendor, part_number, name, category, finish_index, opts }, &progress);
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

/// A STEP file name mapped to a library entry (POLA-163).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub vendor: String,
    pub part_number: String,
    pub name: String,
    pub category: &'static str,
}

/// Part-number prefixes per category; the longest matching prefix wins.
const CATEGORIES: &[(&str, &[&str])] = &[
    ("posts/posts", &["TR", "RS", "MS"]),
    ("posts/holders", &["PH"]),
    ("posts/clamps", &["RA", "SWC"]),
    ("mounts/kinematic", &["KM", "KS", "KC", "K5X", "K6X", "POLARIS"]),
    ("mounts/fixed", &["VM", "LM", "LRM", "CL", "HMM", "SCP", "SPT", "ST1XY", "CXY", "CT", "SM05", "SM16", "SSP"]),
    (
        "stages",
        &["DT", "DTS", "DDR", "XR", "XRN", "XYR", "XYT", "LX", "MT", "MVS", "PT", "T12", "GM", "AMA", "MMP", "OCT", "RB", "MK"],
    ),
    ("fiber", &["ADAFC", "CAPF", "CAPN", "PM20", "SM1FC", "SM1SMA", "SFL", "FC-"]),
    ("hardware", &["F19", "F6M", "LN6"]),
    ("optics/lenses", &["LA", "LC", "LE", "AC", "BE"]),
];

/// The category of a part number by prefix; a "-Stellring…" set ring takes its base part's.
/// A leading "ThorLabs - " and a trailing "-Step" are ignored.
pub fn category_of(part_number: &str) -> &'static str {
    let upper = part_number.trim().to_uppercase();
    let upper = upper.strip_prefix("THORLABS - ").unwrap_or(&upper);
    let upper = upper.strip_suffix("-STEP").unwrap_or(upper);
    let base = upper.split("-STELLRING").next().unwrap_or(upper);
    CATEGORIES
        .iter()
        .flat_map(|(category, prefixes)| prefixes.iter().map(move |p| (*category, *p)))
        .filter(|(_, p)| base.starts_with(p))
        .max_by_key(|(_, p)| p.len())
        .map_or("imported", |(category, _)| category)
}

/// Gives every part under `dir/parts` with category "imported" the category of its part
/// number, where one is known; rewrites only those files. Returns how many changed.
pub fn recategorize_imported(dir: &Path) -> usize {
    let mut changed = 0;
    let mut stack = vec![dir.join("parts")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let mut def: PartDef = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| toml::from_str(&t).map_err(|e| e.to_string())) {
                Ok(def) => def,
                Err(error) => {
                    eprintln!("library: {}: skipped: {error}", path.display());
                    continue;
                }
            };
            let category = category_of(&def.part_number);
            if def.category != "imported" || category == "imported" {
                continue;
            }
            def.category = category.into();
            let tmp = path.with_extension("toml.tmp");
            let written = toml::to_string(&def).map_err(std::io::Error::other).and_then(|t| std::fs::write(&tmp, t)).and_then(|_| std::fs::rename(&tmp, &path));
            match written {
                Ok(()) => changed += 1,
                Err(error) => eprintln!("library: {}: not recategorized: {error}", path.display()),
            }
        }
    }
    changed
}

/// "ThorLabs - F6MSS12 (M6 x 0.25 Fine Hex Adjusters)" → Thorlabs, F6MSS12, the parenthesis
/// text as the name. Part numbers that collide get their parenthesis text appended.
pub fn names_from_stems(stems: &[String]) -> Vec<Named> {
    const PREFIX: &str = "thorlabs - ";
    let mut named: Vec<(Named, Option<String>)> = stems
        .iter()
        .map(|stem| {
            let stem = stem.trim();
            let thorlabs = stem.get(..PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(PREFIX));
            let (vendor, rest) = if thorlabs { ("Thorlabs", stem[PREFIX.len()..].trim()) } else { ("Other", stem) };
            let (part, paren) = match (rest.find('('), rest.rfind(')')) {
                (Some(open), Some(close)) if close > open => {
                    (rest[..open].trim().to_string(), Some(rest[open + 1..close].trim().to_string()).filter(|p| !p.is_empty()))
                }
                _ => (rest.to_string(), None),
            };
            let name = paren.clone().unwrap_or_else(|| part.clone());
            let category = category_of(&part);
            (Named { vendor: vendor.into(), part_number: part, name, category }, paren)
        })
        .collect();
    let clashes: Vec<bool> = named
        .iter()
        .map(|(n, _)| named.iter().filter(|(m, _)| m.vendor == n.vendor && m.part_number == n.part_number).count() > 1)
        .collect();
    for ((n, paren), clash) in named.iter_mut().zip(clashes) {
        if let (true, Some(p)) = (clash, paren) {
            n.part_number = format!("{} ({p})", n.part_number);
        }
    }
    named.into_iter().map(|(n, _)| n).collect()
}

fn named_id(n: &Named) -> String {
    format!("{}:{}", n.vendor.to_lowercase(), n.part_number)
}

struct Batch {
    jobs: VecDeque<(PathBuf, Named)>,
    total: usize,
    imported: usize,
    skipped: usize,
    failed: Vec<String>,
    /// (vendor, id) of every imported or skipped part, for the vendor libraries.
    ids: Vec<(String, String)>,
    library: PathBuf,
    finish_index: i32,
    up: i32,
    started: std::time::Instant,
}

/// "Import Folder into Library…": the folder dialog, then [`start_batch`].
pub fn choose_folder(ui: &AppWindow) {
    let w = ui.as_weak();
    show_file_dialog_local(owner_hwnd(ui.window()), DialogKind::OpenFolder, None, move |result| {
        let Some(ui) = w.upgrade() else { return };
        match result {
            DialogResult::Chosen(dir) => start_batch(&ui, &dir),
            DialogResult::ChosenMany(_) | DialogResult::Cancelled => {}
            DialogResult::Failed(text) => {
                ui.set_import_error(text.into());
                ui.set_import_step(3);
            }
        }
    });
}

/// Imports every `.step`/`.stp` file directly in `dir` (no recursion), one at a time on a
/// worker thread, with the form's finish and up axis.
fn start_batch(ui: &AppWindow, dir: &Path) {
    let Some(library) = lab_catalog::user_dir() else {
        ui.set_import_error("The user library folder (%APPDATA%\\LabSetup\\library) is not available.".into());
        ui.set_import_step(3);
        return;
    };
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    files.retain(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp")));
    files.sort();
    if files.is_empty() {
        ui.set_import_error(format!("{} has no .step or .stp files.", dir.display()).into());
        ui.set_import_step(3);
        return;
    }
    let stems: Vec<String> = files.iter().map(|p| p.file_stem().unwrap_or_default().to_string_lossy().into_owned()).collect();
    let jobs: VecDeque<_> = files.into_iter().zip(names_from_stems(&stems)).collect();
    let batch = Batch {
        total: jobs.len(),
        jobs,
        imported: 0,
        skipped: 0,
        failed: Vec::new(),
        ids: Vec::new(),
        library,
        finish_index: ui.get_import_finish(),
        up: ui.get_import_up(),
        started: std::time::Instant::now(),
    };
    BATCH.with(|b| *b.borrow_mut() = Some(batch));
    ui.set_import_step(2);
    next_job(ui);
}

/// Skips files already in the user library, starts the next conversion, or ends the batch.
fn next_job(ui: &AppWindow) {
    let user_ids: BTreeSet<String> = HANDLES.with(|h| {
        h.borrow().as_ref().map_or_else(BTreeSet::new, |(app, _)| {
            let c = &app.borrow().catalog;
            c.all().iter().filter(|d| c.source(&d.id) == Some(lab_catalog::Source::User)).map(|d| d.id.clone()).collect()
        })
    });
    let job = BATCH.with(|b| {
        let mut b = b.borrow_mut();
        let batch = b.as_mut()?;
        while let Some((path, named)) = batch.jobs.pop_front() {
            let id = named_id(&named);
            if user_ids.contains(&id) {
                batch.skipped += 1;
                batch.ids.push((named.vendor.clone(), id));
                continue;
            }
            let text = format!("Importing {} of {}: {}", batch.total - batch.jobs.len(), batch.total, named.part_number);
            let spec = Spec {
                vendor: named.vendor.clone(),
                part_number: named.part_number.clone(),
                name: named.name.clone(),
                category: named.category.to_string(),
                finish_index: batch.finish_index,
                opts: ImportOptions {
                    source_up: if batch.up == 0 { SourceUp::Y } else { SourceUp::Z },
                    finish: Some(finish_of(batch.finish_index)),
                    ..Default::default()
                },
            };
            return Some((path, named, spec, batch.library.clone(), text));
        }
        None
    });
    let Some((path, named, spec, library, text)) = job else { return end_batch(ui) };
    ui.set_import_progress(text.into());
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let w = ui.as_weak();
    std::thread::spawn(move || {
        // A panic must count as a failure, or the batch waits forever for this file.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| convert(&path, &library, spec, &|_| {})))
            .map_err(|_| "the importer panicked".to_string())
            .and_then(|r| r.map(|_| ()).map_err(|e| e.to_string()));
        slint::invoke_from_event_loop(move || {
            if GENERATION.load(Ordering::SeqCst) != generation {
                return; // cancelled
            }
            let Some(ui) = w.upgrade() else { return };
            BATCH.with(|b| {
                if let Some(batch) = b.borrow_mut().as_mut() {
                    match result {
                        Ok(()) => {
                            batch.imported += 1;
                            batch.ids.push((named.vendor.clone(), named_id(&named)));
                        }
                        Err(error) => {
                            eprintln!("import folder: {} failed: {error}", path.display());
                            batch.failed.push(named.part_number.clone());
                        }
                    }
                }
            });
            next_job(&ui);
        })
        .ok();
    });
}

/// Ends the batch (if one runs): reloads the catalog once, adds the parts to the library named
/// after their vendor and shows the summary.
fn end_batch(ui: &AppWindow) {
    let Some(batch) = BATCH.with(|b| b.borrow_mut().take()) else { return };
    let summary = format!(
        "{} imported, {} skipped, {} failed ({:.0} s)",
        batch.imported,
        batch.skipped,
        batch.failed.len(),
        batch.started.elapsed().as_secs_f64()
    );
    eprintln!("import folder: {summary}");
    if !batch.failed.is_empty() {
        eprintln!("import folder: failed: {}", batch.failed.join(", "));
    }
    HANDLES.with(|h| {
        let Some((app, models)) = &*h.borrow() else { return };
        let mut a = app.borrow_mut();
        for (vendor, id) in &batch.ids {
            let libs = &mut a.settings.libraries;
            let i = libs.iter().position(|l| &l.name == vendor).unwrap_or_else(|| {
                libs.push(lab_file::UserLibrary { name: vendor.clone(), parts: Vec::new() });
                libs.len() - 1
            });
            if !libs[i].parts.contains(id) {
                libs[i].parts.push(id.clone());
            }
        }
        if let Err(error) = lab_file::save_settings(&a.settings) {
            eprintln!("settings not saved: {error}");
        }
        library::reload_catalog(&mut a, models);
        library::sync_ui(ui, &a);
        crate::thumbs::render_missing(&a);
        controller::show_toast(ui, &a, &summary, false);
    });
    ui.set_import_step(0);
}

struct Spec {
    vendor: String,
    part_number: String,
    name: String,
    category: String,
    finish_index: i32,
    opts: ImportOptions,
}

/// Blocking: converts `step`, detects its features and connectors and writes the part to
/// `library` (with `meta["step_hash"]`, so a later drop of the same file reuses it).
fn convert(
    step: &Path,
    library: &Path,
    spec: Spec,
    progress: &dyn Fn(Progress),
) -> Result<(PartDef, lab_import::Imported), lab_import::ImportError> {
    let step_hash = blake3::hash(&std::fs::read(step)?).to_hex().to_string();
    let imported = lab_import::import_step(step, library, &spec.opts, progress)?;
    let mut mesh = TriMesh::default();
    for (_, m) in lab_import::read_glb(&imported.path, 0)? {
        mesh.append(&m);
    }
    let features = lab_import::features::detect(&mesh);
    let mut connectors =
        vec![ConnectorDef { name: "seat".into(), kind: ConnectorKind::HoleGridSeat, frame: seat_frame(&features) }];
    connectors.extend(hole_connectors(&features));
    let def = PartDef {
        id: format!("{}:{}", spec.vendor.to_lowercase(), spec.part_number),
        version: 1,
        vendor: spec.vendor,
        part_number: spec.part_number,
        name: spec.name,
        category: if spec.category.is_empty() { "imported".into() } else { spec.category },
        tags: Vec::new(),
        geometry: Geometry::Mesh { asset: imported.asset.clone() },
        connectors,
        optical_ports: Vec::new(),
        meta: BTreeMap::from([
            ("finish".to_string(), FINISHES[spec.finish_index.clamp(0, 2) as usize].to_string()),
            ("source".to_string(), step.file_name().unwrap_or_default().to_string_lossy().into_owned()),
            ("step_hash".to_string(), step_hash),
        ]),
        features,
    };
    lab_import::write_part(library, &def)?;
    Ok((def, imported))
}

/// (vendor, part number) from a STEP file name: Thorlabs names its files "<part>-Step.step"
/// with '/' written as '_'.
fn names_from_file(step: &Path) -> (&'static str, String) {
    let stem = step.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let part = stem.trim_end_matches("-Step").trim_end_matches("-step");
    let vendor = if part.len() < stem.len() { "Thorlabs" } else { "Imported" };
    (vendor, part.replace('_', "/"))
}

/// "Insert STEP…": the open dialog, then [`quick_insert`] at the viewport centre.
pub fn insert_dialog(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    show_file_dialog_local(owner_hwnd(ui.window()), DialogKind::OpenSteps, None, move |result| {
        let Some(ui) = w.upgrade() else { return };
        match result {
            DialogResult::Chosen(path) => quick_insert(&ui, &a, &m, path, None),
            DialogResult::ChosenMany(paths) => {
                // shortcut: spaced 120 px apart on screen from the viewport centre, not 100 mm
                // in the scene; switch to world spacing if parts of very different sizes overlap.
                let centre = centre(&ui);
                let scale = ui.window().scale_factor();
                for (i, path) in paths.into_iter().enumerate() {
                    let pos = Vec2::new(centre.x + i as f32 * 120.0 * scale, centre.y);
                    quick_insert(&ui, &a, &m, path, Some(pos));
                }
            }
            DialogResult::Cancelled => {}
            DialogResult::Failed(text) => controller::show_toast(&ui, &a.borrow(), &text, false),
        }
    });
}

/// Converts `step` in the background (or reuses the library part with the same STEP hash),
/// writes the library entry silently, reloads the catalog, then places it at `pos`
/// (viewport px) with DragPart Move+Drop so it seats on a hole. Toast on error.
pub fn quick_insert(ui: &AppWindow, app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>, step: PathBuf, pos: Option<Vec2>) {
    let toast = |text: &str| controller::show_toast(ui, &app.borrow(), text, false);
    let hash = match std::fs::read(&step) {
        Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
        Err(error) => return toast(&format!("{} could not be read: {error}", step.display())),
    };
    let known = app.borrow().catalog.all().iter().find(|d| d.meta.get("step_hash") == Some(&hash)).map(|d| d.id.clone());
    if let Some(id) = known {
        return place(ui, app, &id, pos);
    }
    let Some(library) = lab_catalog::user_dir() else {
        return toast("The user library folder (%APPDATA%\\LabSetup\\library) is not available.");
    };
    let (vendor, mut part) = names_from_file(&step);
    // An older import with this id (no or another STEP hash) keeps its own file and connectors.
    if app.borrow().catalog.get(&format!("{}:{part}", vendor.to_lowercase())).is_some() {
        part = format!("{part} ({})", &hash[..6]);
    }
    toast(&format!("Converting {part}…"));
    let category = category_of(&part);
    let spec = Spec {
        vendor: vendor.into(),
        part_number: part.clone(),
        name: part,
        category: category.into(),
        finish_index: 0,
        opts: ImportOptions { source_up: SourceUp::Y, finish: Some(finish_of(0)), ..Default::default() },
    };
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let result = convert(&step, &library, spec, &|_| {}).map(|(def, _)| def.id).map_err(|e| e.to_string());
        slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            HANDLES.with(|h| {
                let Some((app, models)) = &*h.borrow() else { return };
                match result {
                    Ok(id) => {
                        {
                            let mut a = app.borrow_mut();
                            library::reload_catalog(&mut a, models);
                            crate::thumbs::render(&a, &id);
                        }
                        // shortcut: the catalog swap respawns the scene, so the drop ray misses
                        // the table for a few frames; a fixed delay, until the viewport reports readiness.
                        let (w, app) = (ui.as_weak(), app.clone());
                        slint::Timer::single_shot(std::time::Duration::from_millis(500), move || {
                            if let Some(ui) = w.upgrade() {
                                place(&ui, &app, &id, pos);
                            }
                        });
                    }
                    Err(text) => controller::show_toast(&ui, &app.borrow(), &text, false),
                }
            });
        })
        .ok();
    });
}

/// The centre of the visible viewport (right of the sidebar), in viewport px.
fn centre(ui: &AppWindow) -> Vec2 {
    let scale = ui.window().scale_factor();
    let left = (ui.get_sidebar_right() - ui.get_viewport_x()).max(0.0);
    Vec2::new((left + ui.get_viewport_width()) * 0.5 * scale, ui.get_viewport_height() * 0.5 * scale)
}

/// Ghosts catalog part `id` at `pos` (viewport px; `None` = the viewport centre) and drops it.
fn place(ui: &AppWindow, app: &Rc<RefCell<AppState>>, id: &str, pos: Option<Vec2>) {
    let pos = pos.unwrap_or_else(|| centre(ui));
    let mods = Modifiers::default();
    let a = app.borrow();
    a.send(ViewportRequest::DragPart(PartDrag::Move { item: DragItem::Part(id.into()), pos, mods }));
    a.send(ViewportRequest::DragPart(PartDrag::Drop { pos, mods }));
}

/// The seat (M8-PLAN A): a bottom hole (axis down within 2°, M4–M6 / 8-32–1/4-20 size, opening
/// within 1 mm of z = 0 and of the lowest such opening) nearest the bbox centre in XY, at z = 0;
/// none: the origin.
fn seat_frame(features: &[Feature]) -> Frame {
    let bottom: Vec<DVec3> = features
        .iter()
        .filter_map(|f| match f {
            Feature::Axis { origin, dir, radius, hole: true }
                if dir.z < -0.999_390_827 && origin.z.abs() <= 1.0 && (3.2..=6.2).contains(&(2.0 * radius)) =>
            {
                Some(*origin)
            }
            _ => None,
        })
        .collect();
    let lowest = bottom.iter().map(|o| o.z).fold(f64::INFINITY, f64::min);
    let origin = bottom
        .iter()
        .filter(|o| o.z - lowest <= 1.0)
        .map(|o| DVec3::new(o.x, o.y, 0.0))
        .min_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap_or(DVec3::ZERO);
    Frame { origin, z: DVec3::NEG_Z, x: DVec3::X }
}

/// Detected holes as connectors `hole_1`, …, typed like the wizard's Hole button does.
fn hole_connectors(features: &[Feature]) -> Vec<ConnectorDef> {
    features
        .iter()
        .filter_map(|f| match f {
            Feature::Axis { origin, dir, radius, hole: true } => Some((*origin, *dir, (radius * 200.0).round() / 100.0)),
            _ => None,
        })
        .enumerate()
        .map(|(i, (origin, z, diameter))| {
            let kind = match lab_import::fit::suggest(diameter) {
                ("thread", spec) => ConnectorKind::Thread { spec: spec.into(), gender: Gender::Female },
                ("bore", _) => ConnectorKind::Bore { diameter, gender: Gender::Female, range: None },
                _ => ConnectorKind::OpticBore { diameter, gender: Gender::Female },
            };
            let helper = if z.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
            let frame = Frame { origin, z, x: z.cross(helper).cross(z).normalize() };
            ConnectorDef { name: format!("hole_{}", i + 1), kind, frame }
        })
        .collect()
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
            if let Some(next) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
                prefill(ui, next);
                start(ui);
            }
        }
        Err(text) => {
            QUEUE.with(|q| q.borrow_mut().clear());
            ui.set_import_error(text.into());
            ui.set_import_step(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seat_accepts_a_bottom_hole_just_above_zero() {
        let hole = Feature::Axis { origin: DVec3::new(5.0, -3.0, 0.62), dir: DVec3::NEG_Z, radius: 3.0, hole: true };
        assert_eq!(seat_frame(&[hole]).origin, DVec3::new(5.0, -3.0, 0.0));
    }

    #[test]
    fn folder_names_map_to_specs() {
        let stems: Vec<String> = [
            "ThorLabs - KM100",
            "thorlabs - TR50-M",
            "ThorLabs - F6MSS12 (M6 x 0.25 Fine Hex Adjusters)",
            "ThorLabs - F19SS050 (A)",
            "ThorLabs - F19SS050 (B)",
            "ThorLabs - SPT2-M-Stellring1",
            "ThorLabs - DTS25",
            "Widget 7",
        ]
        .map(String::from)
        .to_vec();
        let n = names_from_stems(&stems);
        let km = Named { vendor: "Thorlabs".into(), part_number: "KM100".into(), name: "KM100".into(), category: "mounts/kinematic" };
        assert_eq!(n[0], km);
        assert_eq!((n[1].part_number.as_str(), n[1].category), ("TR50-M", "posts/posts"));
        assert_eq!((n[2].part_number.as_str(), n[2].name.as_str(), n[2].category), ("F6MSS12", "M6 x 0.25 Fine Hex Adjusters", "hardware"));
        assert_eq!((n[3].part_number.as_str(), n[4].part_number.as_str()), ("F19SS050 (A)", "F19SS050 (B)"));
        assert_eq!(n[5].category, "mounts/fixed");
        assert_eq!(n[6].category, "stages");
        assert_eq!((n[7].vendor.as_str(), n[7].part_number.as_str(), n[7].category), ("Other", "Widget 7", "imported"));
    }

    #[test]
    fn import_category_of_strips_vendor_and_step() {
        assert_eq!(category_of("ThorLabs - KM100"), "mounts/kinematic");
        assert_eq!(category_of("ThorLabs - TR50-M"), "posts/posts");
        assert_eq!(category_of("ThorLabs - CT101"), "mounts/fixed");
        assert_eq!(category_of("ThorLabs - PH50-M"), "posts/holders");
        assert_eq!(category_of("LA1289-A-ML"), "optics/lenses");
        assert_eq!(category_of("BE1-Step"), "optics/lenses");
        assert_eq!(category_of("SM05NT1"), "mounts/fixed");
        assert_eq!(category_of("SM16RR"), "mounts/fixed");
        assert_eq!(category_of("SSP05"), "mounts/fixed");
        assert_eq!(category_of("LB1"), "imported");
        assert_eq!(category_of("Widget 7"), "imported");
    }

    #[test]
    fn import_recategorize_imported_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("labsetup-recat-{}", std::process::id()));
        let parts = dir.join("parts").join("imported");
        std::fs::create_dir_all(&parts).unwrap();
        for (file, number) in [("km.toml", "KM100"), ("w.toml", "Widget")] {
            let def = PartDef {
                id: format!("other:{number}"),
                version: 1,
                vendor: "Other".into(),
                part_number: number.into(),
                name: number.into(),
                category: "imported".into(),
                tags: Vec::new(),
                geometry: Geometry::Mesh { asset: "x.glb".into() },
                connectors: Vec::new(),
                optical_ports: Vec::new(),
                meta: BTreeMap::new(),
                features: Vec::new(),
            };
            std::fs::write(parts.join(file), toml::to_string(&def).unwrap()).unwrap();
        }
        assert_eq!(recategorize_imported(&dir), 1);
        assert_eq!(recategorize_imported(&dir), 0);
        let read = |f: &str| toml::from_str::<PartDef>(&std::fs::read_to_string(parts.join(f)).unwrap()).unwrap().category;
        assert_eq!((read("km.toml"), read("w.toml")), ("mounts/kinematic".to_string(), "imported".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
