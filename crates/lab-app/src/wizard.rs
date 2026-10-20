//! The connector wizard (M4-PLAN POLA-60): click a hole or optical face of a user-library part
//! in inspect mode, confirm what it is, save the definition back to the user library.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use lab_core::glam::DVec3;
use lab_core::part::{ConnectorDef, ConnectorKind, Frame, Gender, Geometry, OpticalPort, PartDef};
use lab_geom::TriMesh;
use lab_import::fit::{HoleFit, fit_hole, suggest};
use lab_ui::AppWindow;
use lab_viewport::ViewportRequest;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app_state::AppState;
use crate::controller::show_toast;
use crate::ui_models::UiModels;

const OPTIC_KINDS: [&str; 8] = ["mirror", "lens", "splitter", "waveplate", "source", "detector", "dump", "fiber"];
const THREAD_KINDS: [&str; 3] = ["thread", "bore", "optic_bore"];

struct Wizard {
    def: PartDef,
    mesh: TriMesh,
    step: i32,
    point: DVec3,
    normal: DVec3,
    fit: Option<HoleFit>,
    hole_kind: i32,
    spec: String,
    gender: i32,
    diameter: f64,
    optic_kind: i32,
    aperture: Option<f64>,
    message: String,
}

thread_local! {
    static WIZARD: RefCell<Option<Wizard>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Wizard) -> R) -> Option<R> {
    WIZARD.with(|w| w.borrow_mut().as_mut().map(f))
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_library_edit(move |id| {
        if let Some(ui) = w.upgrade() {
            open(&ui, &a.borrow(), &id);
        }
    });
    let w = ui.as_weak();
    ui.on_wizard_choose(move |i| {
        with(|wz| choose(wz, i));
        refresh(&w);
    });
    let w = ui.as_weak();
    ui.on_wizard_pick(move |id, i| {
        with(|wz| match id.as_str() {
            "kind" => wz.hole_kind = i,
            "gender" => wz.gender = i,
            _ => wz.optic_kind = i,
        });
        refresh(&w);
    });
    let w = ui.as_weak();
    ui.on_wizard_field(move |id, text| {
        let ok = with(|wz| field(wz, &id, text.trim())).unwrap_or(false);
        refresh(&w);
        ok
    });
    let w = ui.as_weak();
    ui.on_wizard_back(move || {
        with(|wz| wz.step = 0);
        refresh(&w);
    });
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_wizard_confirm(move || {
        with(confirm);
        send_markers(&a.borrow());
        refresh(&w);
    });
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_wizard_remove(move |i| {
        with(|wz| {
            let i = i as usize;
            if i < wz.def.connectors.len() {
                wz.def.connectors.remove(i);
            } else if i - wz.def.connectors.len() < wz.def.optical_ports.len() {
                wz.def.optical_ports.remove(i - wz.def.connectors.len());
            }
        });
        send_markers(&a.borrow());
        refresh(&w);
    });
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_wizard_cancel(move || {
        if let Some(ui) = w.upgrade() {
            close(&ui, &a.borrow());
        }
    });
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_wizard_save(move || {
        if let Some(ui) = w.upgrade() {
            save(&ui, &a, &m);
        }
    });
}

/// Opens the connector wizard on a user-library part.
pub fn open(ui: &AppWindow, app: &AppState, part_id: &str) {
    if app.catalog.source(part_id) != Some(lab_catalog::Source::User) {
        show_toast(ui, app, "Only parts in your own library can get new connectors.", false);
        return;
    }
    let Some(def) = app.catalog.get(part_id).cloned() else { return };
    let mesh = match part_mesh(&def) {
        Ok(mesh) => mesh,
        Err(error) => {
            show_toast(ui, app, &format!("The part's mesh could not be read: {error}"), false);
            return;
        }
    };
    WIZARD.with(|w| {
        *w.borrow_mut() = Some(Wizard {
            def,
            mesh,
            step: 0,
            point: DVec3::ZERO,
            normal: DVec3::Z,
            fit: None,
            hole_kind: 0,
            spec: String::new(),
            gender: 1,
            diameter: 0.0,
            optic_kind: 0,
            aperture: None,
            message: String::new(),
        })
    });
    app.send(ViewportRequest::InspectPart(Some(part_id.to_string())));
    send_markers(app);
    ui.set_wizard_optic_kinds(ModelRc::new(VecModel::from(OPTIC_KINDS.map(SharedString::from).to_vec())));
    ui.set_wizard_open(true);
    refresh(&ui.as_weak());
}

/// A click on the inspected part (viewport `PartPoint`).
pub fn on_part_point(ui: &AppWindow, point: DVec3, normal: DVec3) {
    with(|wz| {
        wz.point = point;
        wz.normal = normal;
        wz.step = 1;
        wz.message.clear();
    });
    refresh(&ui.as_weak());
}

/// LOD0 of a stored GLB, or the generated mesh of a parametric part, merged into one mesh.
fn part_mesh(def: &PartDef) -> Result<TriMesh, String> {
    let mut mesh = TriMesh::default();
    match &def.geometry {
        Geometry::Mesh { asset } => {
            let (_, path) = lab_catalog::mesh_path(asset).ok_or("its mesh file is missing")?;
            for (_, m) in lab_import::read_glb(&path, 0).map_err(|e| e.to_string())? {
                mesh.append(&m);
            }
        }
        Geometry::Parametric { generator, params } => {
            for (_, m) in &lab_parts::generate(generator, params).map_err(|e| e.to_string())?.pieces {
                mesh.append(m);
            }
        }
    }
    Ok(mesh)
}

fn choose(wz: &mut Wizard, i: i32) {
    if i == 1 {
        wz.step = 3;
        wz.optic_kind = 0;
        wz.aperture = None;
        return;
    }
    match fit_hole(&wz.mesh, wz.point, wz.normal) {
        Some(fit) => {
            let (kind, spec) = suggest(fit.diameter);
            wz.fit = Some(fit);
            wz.hole_kind = THREAD_KINDS.iter().position(|k| *k == kind).unwrap_or(0) as i32;
            wz.spec = if spec.is_empty() { "M4".into() } else { spec.into() };
            wz.diameter = (fit.diameter * 100.0).round() / 100.0;
            wz.gender = 1;
            wz.step = 2;
        }
        None => {
            wz.message = "No round hole found here.".into();
            wz.step = 0;
        }
    }
}

fn field(wz: &mut Wizard, id: &str, text: &str) -> bool {
    match id {
        "spec" if !text.is_empty() => wz.spec = text.to_string(),
        "diameter" => match text.parse::<f64>() {
            Ok(d) if d > 0.0 => wz.diameter = d,
            _ => return false,
        },
        "aperture" if text.is_empty() => wz.aperture = None,
        "aperture" => match text.parse::<f64>() {
            Ok(d) if d > 0.0 => wz.aperture = Some(d),
            _ => return false,
        },
        _ => return false,
    }
    true
}

/// A name not used yet: `<base>_1`, `<base>_2`, …
fn unique(def: &PartDef, base: &str) -> String {
    (1..)
        .map(|n| format!("{base}_{n}"))
        .find(|n| !def.connectors.iter().any(|c| &c.name == n) && !def.optical_ports.iter().any(|p| &p.name == n))
        .unwrap_or_default()
}

fn confirm(wz: &mut Wizard) {
    match wz.step {
        2 => {
            let Some(fit) = wz.fit else { return };
            let gender = if wz.gender == 0 { Gender::Male } else { Gender::Female };
            let kind = match wz.hole_kind {
                0 => ConnectorKind::Thread { spec: wz.spec.clone(), gender },
                1 => ConnectorKind::Bore { diameter: wz.diameter, gender, range: None },
                _ => ConnectorKind::OpticBore { diameter: wz.diameter, gender },
            };
            let name = unique(&wz.def, THREAD_KINDS[wz.hole_kind.clamp(0, 2) as usize]);
            wz.def.connectors.push(ConnectorDef { name, kind, frame: fit.axis });
        }
        3 => {
            let kind = OPTIC_KINDS[wz.optic_kind.clamp(0, OPTIC_KINDS.len() as i32 - 1) as usize];
            let z = wz.normal.normalize_or(DVec3::Z);
            let helper = if z.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
            let frame = Frame { origin: wz.point, z, x: z.cross(helper).cross(z).normalize() };
            let name = unique(&wz.def, kind);
            wz.def.optical_ports.push(OpticalPort { name, kind: kind.into(), clear_aperture: wz.aperture, frame });
        }
        _ => return,
    }
    wz.step = 0;
    wz.fit = None;
}

fn row(c: &ConnectorDef) -> String {
    let g = |g: &Gender| if *g == Gender::Male { "male" } else { "female" };
    let what = match &c.kind {
        ConnectorKind::Thread { spec, gender } => format!("thread {spec} {}", g(gender)),
        ConnectorKind::Bore { diameter, gender, .. } => format!("bore Ø{diameter:.2} mm {}", g(gender)),
        ConnectorKind::OpticBore { diameter, gender } => format!("optic bore Ø{diameter:.2} mm {}", g(gender)),
        ConnectorKind::HoleGridSeat => "seat on a table hole".into(),
        ConnectorKind::CageRod { spacing, gender } => format!("cage {spacing} mm {}", g(gender)),
        ConnectorKind::LensTube { thread, gender } => format!("lens tube {thread} {}", g(gender)),
    };
    format!("{} · {what}", c.name)
}

fn send_markers(app: &AppState) {
    let frames = with(|wz| {
        wz.def.connectors.iter().map(|c| c.frame).chain(wz.def.optical_ports.iter().map(|p| p.frame)).collect()
    });
    if let Some(frames) = frames {
        app.send(ViewportRequest::InspectMarkers(frames));
    }
}

fn refresh(ui: &slint::Weak<AppWindow>) {
    let Some(ui) = ui.upgrade() else { return };
    with(|wz| {
        let rows: Vec<SharedString> = wz
            .def
            .connectors
            .iter()
            .map(row)
            .chain(wz.def.optical_ports.iter().map(|p| format!("{} · {} port", p.name, p.kind)))
            .map(SharedString::from)
            .collect();
        ui.set_wizard_title(wz.def.name.as_str().into());
        ui.set_wizard_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_wizard_step(wz.step);
        ui.set_wizard_message(wz.message.as_str().into());
        ui.set_wizard_hole_kind(wz.hole_kind);
        ui.set_wizard_spec(wz.spec.as_str().into());
        ui.set_wizard_gender(wz.gender);
        ui.set_wizard_diameter(format!("{:.2}", wz.diameter).into());
        ui.set_wizard_optic_kind(wz.optic_kind);
        ui.set_wizard_aperture(wz.aperture.map(|a| format!("{a:.2}")).unwrap_or_default().into());
    });
}

fn close(ui: &AppWindow, app: &AppState) {
    WIZARD.with(|w| w.borrow_mut().take());
    app.send(ViewportRequest::InspectPart(None));
    ui.set_wizard_open(false);
}

fn save(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let Some(def) = with(|wz| wz.def.clone()) else { return };
    let written = lab_catalog::user_dir()
        .ok_or_else(|| "there is no user library folder".to_string())
        .and_then(|dir| lab_import::write_part(&dir, &def).map_err(|e| e.to_string()));
    if let Err(error) = written {
        with(|wz| wz.message = format!("Could not save: {error}."));
        refresh(&ui.as_weak());
        return;
    }
    // shortcut: reloads here; switch to `library::reload_catalog` (POLA-59) once merged.
    {
        let mut a = app.borrow_mut();
        let (catalog, warnings) = lab_catalog::Catalog::load_default();
        for w in &warnings {
            eprintln!("library: {}: {}", w.path.display(), w.message);
        }
        a.catalog = Arc::new(catalog);
        a.send(ViewportRequest::SetCatalog(a.catalog.clone()));
        crate::library::fill(&a, models);
    }
    let a = app.borrow();
    close(ui, &a);
    show_toast(ui, &a, &format!("Saved the connectors of {}.", def.name), false);
}
