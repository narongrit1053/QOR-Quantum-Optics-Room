//! Laser beams in the app (M6-PLAN B): the document is re-traced with `lab_optics` on every
//! refresh (the viewport traces its own copy) and the inspector shows the beam at the selection.

use lab_app::{EditError, EditFieldData, EditOutcome};
use lab_core::part::ParamValue;
use lab_core::units::format_length_with_unit;
use lab_core::{Command, NodeId, SelectionTarget};
use lab_optics::{PortKind, Trace};
use lab_ui::AppWindow;

use crate::app_state::AppState;
use crate::ui_models::{self, UiModels};

/// Laser parameters on the source node: (key, label, unit, default).
const PARAMS: [(&str, &str, &str, f64); 3] = [
    ("beam_wavelength_nm", "Wavelength", "nm", 780.0),
    ("beam_waist_mm", "Waist (1/e²)", "mm", 0.5),
    ("beam_m2", "M²", "", 1.0),
];

pub fn retrace(app: &mut AppState) {
    let catalog = app.catalog.clone();
    app.beams = lab_optics::trace_document(&app.session.doc, &|id| catalog.get(id).cloned());
}

/// The Beam section for node `id`: hidden for parts without optical ports.
pub fn fill(ui: &AppWindow, app: &AppState, models: &UiModels, id: NodeId) {
    let doc = &app.session.doc;
    let ports = doc.node(id).and_then(|n| app.catalog.get(&n.part)).map(|d| &d.optical_ports);
    let laser = ports.is_some_and(|p| p.iter().any(|p| PortKind::from_kind(&p.kind) == PortKind::Source));
    let node = doc.node(id);
    let fields = PARAMS.iter().filter(|_| laser).map(|(key, label, unit, default)| {
        let value = node.and_then(|n| n.params.get(*key)).and_then(ParamValue::as_f64).unwrap_or(*default);
        EditFieldData {
            id: format!("beam.{key}"),
            label: (*label).into(),
            value: format!("{value}"),
            unit: (*unit).into(),
            enabled: true,
            invalid: false,
        }
    });
    ui_models::sync(&models.beam_fields, fields.map(ui_models::edit_field));
    let on = !matches!(node.and_then(|n| n.params.get("beam_on")), Some(ParamValue::Flag(false)));
    ui.set_beam_laser(laser);
    ui.set_beam_on(on);
    let text = match ports {
        None => String::new(),
        Some(p) if p.is_empty() => String::new(),
        _ if laser => String::new(),
        _ => describe(&app.beams, id, app),
    };
    ui.set_beam_text(text.into());
}

fn describe(trace: &Trace, id: NodeId, app: &AppState) -> String {
    let Some(i) = trace.segments.iter().position(|s| s.hit.as_ref().is_some_and(|(n, _)| *n == id)) else {
        return "No beam".into();
    };
    let w = trace.segments[i].w_end;
    format!(
        "Beam Ø at this part: 2w = {:.2} mm · {} from the source",
        2.0 * w,
        format_length_with_unit(path_length(trace, i), app.units())
    )
}

/// Optical path from the source to the end of segment `i`: walk back through the segments that
/// end where the current one starts.
fn path_length(trace: &Trace, mut i: usize) -> f64 {
    let mut total = 0.0;
    for _ in 0..trace.segments.len() {
        let s = &trace.segments[i];
        total += s.start.distance(s.end);
        match trace.segments[..i].iter().rposition(|p| p.source == s.source && p.end.distance(s.start) < 1e-6) {
            Some(prev) => i = prev,
            None => break,
        }
    }
    total
}

fn laser(app: &AppState) -> Result<(NodeId, lab_core::part::Params), EditError> {
    let Some(SelectionTarget::Node(id)) = app.session.selection else { return Err(EditError::Stale) };
    let node = app.session.doc.node(id).ok_or(EditError::Stale)?;
    Ok((id, node.params.clone()))
}

/// A committed `beam.<param>` field.
pub fn field_edit(app: &AppState, field: &str, text: &str) -> Result<EditOutcome, EditError> {
    let (id, mut params) = laser(app)?;
    let key = field.strip_prefix("beam.").ok_or_else(|| EditError::UnknownField(field.into()))?;
    let (_, label, _, default) =
        PARAMS.iter().find(|p| p.0 == key).ok_or_else(|| EditError::UnknownField(field.into()))?;
    let value: f64 = text
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .ok_or_else(|| EditError::Invalid(format!("{label} must be a positive number.")))?;
    let current = params.get(key).and_then(ParamValue::as_f64).unwrap_or(*default);
    if (current - value).abs() < 1e-12 {
        return Ok(EditOutcome::Nothing);
    }
    params.insert(key.into(), ParamValue::Number(value));
    Ok(command(id, params, "Edit beam"))
}

/// `beam.toggle`: switches the selected laser on or off.
pub fn toggle(app: &AppState) -> Result<EditOutcome, EditError> {
    let (id, mut params) = laser(app)?;
    let on = !matches!(params.get("beam_on"), Some(ParamValue::Flag(false)));
    params.insert("beam_on".into(), ParamValue::Flag(!on));
    Ok(command(id, params, if on { "Turn beam off" } else { "Turn beam on" }))
}

fn command(id: NodeId, params: lab_core::part::Params, label: &str) -> EditOutcome {
    EditOutcome::Command { command: Command::SetParams { id, params }, label: label.into(), select: None, fixes: Vec::new() }
}

#[cfg(test)]
mod tests {
    use lab_catalog::Catalog;
    use lab_core::glam::{DQuat, DVec2, DVec3};
    use lab_core::holes::HoleGrid;
    use lab_core::model::{Attach, Document, NodeId, Parent, Platform, PlatformId, PlatformKind, Support, Surface};
    use lab_core::outline::Outline;
    use lab_core::{Command, History};
    use lab_viewport::{DragItem, StackSpec};

    /// Places a 75 mm stack of `optic` in `mount` at `at`, turned `yaw`; returns the optic's id.
    fn stack(doc: &mut Document, catalog: &Catalog, optic: &str, mount: &str, at: DVec2, yaw: f64) -> NodeId {
        let def = catalog.get(mount).unwrap();
        let sol = lab_mates::solver::solve_stack(def, 75.0, &catalog.in_category("posts")).unwrap();
        let spec = StackSpec { optic: Some(optic.into()), mount: mount.into(), base: sol.base, holder: sol.holder, post: sol.post, slide: sol.slide };
        let root = DragItem::Stack(spec.clone()).root().to_string();
        let (p, r) = lab_mates::seat_pose(catalog.get(&root).unwrap(), at, yaw);
        let next = doc.nodes.iter().map(|n| n.id.0).max().unwrap_or(0) + 1;
        let platform = Parent::Platform(PlatformId(1));
        let mut nodes = vec![crate::place::new_node(catalog, &root, next, platform, Attach::Free, p, r).unwrap()];
        for (part, attach, position, rotation) in lab_viewport::stack_chain(catalog, &spec).unwrap() {
            let below = Parent::Node(nodes.last().unwrap().id);
            let id = next + nodes.len() as u64;
            nodes.push(crate::place::new_node(catalog, &part, id, below, attach, position, rotation).unwrap());
        }
        let optic = nodes.last().unwrap().id;
        History::default().execute(doc, Command::PlaceAssembly { nodes }).unwrap();
        optic
    }

    fn hits(doc: &Document, catalog: &Catalog, id: NodeId) -> bool {
        let trace = lab_optics::trace_document(doc, &|p| catalog.get(p).cloned());
        trace.radius_at(id).is_some()
    }

    /// Adds a stack, trying the yaws in turn until the beam hits it (and the next check passes).
    fn aim(doc: &mut Document, catalog: &Catalog, optic: &str, mount: &str, at: DVec2, ok: &dyn Fn(&Document, NodeId) -> bool) -> NodeId {
        for deg in (0..360).step_by(15) {
            let mut trial = doc.clone();
            let id = stack(&mut trial, catalog, optic, mount, at, (deg as f64).to_radians());
            if ok(&trial, id) {
                *doc = trial;
                return id;
            }
        }
        panic!("no yaw puts {optic} in the beam at {at}");
    }

    /// Exit-walkthrough scene: laser (raised 35 mm so its 40 mm axis is at 75 mm), two mirror sets
    /// folding the beam, a lens set focusing it. `LABSETUP_BEAM_SCENE=<path>` writes it as a project.
    #[test]
    fn beam_through_mirror_and_lens_sets() {
        let (catalog, _) = Catalog::load_default();
        let mut doc = Document::new("Beams");
        doc.platforms.push(Platform {
            id: PlatformId(1),
            name: "Table 1".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(HoleGrid::metric()),
            support: Support::Legs { size: 120.0, inset: 150.0 },
        });
        let laser = crate::place::new_node(
            &catalog,
            "generic:laser-head",
            100,
            Parent::Platform(PlatformId(1)),
            Attach::Free,
            DVec3::new(0.0, -300.0, 35.0),
            DQuat::IDENTITY,
        )
        .unwrap();
        History::default().execute(&mut doc, Command::PlaceAssembly { nodes: vec![laser] }).unwrap();
        let trace = |d: &Document| lab_optics::trace_document(d, &|p| catalog.get(p).cloned());
        // Each fold must send the beam on along +X, then +Y.
        let along = |d: &Document, dir: DVec3| trace(d).segments.last().is_some_and(|s| (s.end - s.start).normalize().dot(dir) > 0.99);
        let m1 = aim(&mut doc, &catalog, "generic:mirror-1in", "thorlabs:KM100", DVec2::new(0.0, 0.0), &|d, id| {
            hits(d, &catalog, id) && along(d, DVec3::X)
        });
        let m2 = aim(&mut doc, &catalog, "generic:mirror-1in", "thorlabs:KM100", DVec2::new(250.0, 0.0), &|d, id| {
            hits(d, &catalog, id) && along(d, DVec3::Y)
        });
        let lens = aim(&mut doc, &catalog, "generic:lens-1in", "thorlabs:FMP1", DVec2::new(250.0, 200.0), &|d, id| hits(d, &catalog, id));
        let t = trace(&doc);
        assert!(t.radius_at(m1).is_some() && t.radius_at(m2).is_some() && t.radius_at(lens).is_some());
        assert!(t.segments.iter().any(|s| s.waist.is_some()), "no focus after the lens");
        if let Ok(path) = std::env::var("LABSETUP_BEAM_SCENE") {
            std::fs::write(path, lab_file::encode_archive(&doc).unwrap()).unwrap();
        }
    }
}
