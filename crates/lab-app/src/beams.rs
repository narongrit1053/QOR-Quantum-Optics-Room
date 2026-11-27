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
const PARAMS: [Param; 5] = [
    ("beam_wavelength_nm", "Wavelength", "nm", 780.0),
    ("beam_waist_mm", "Waist (1/e²)", "mm", 0.5),
    ("beam_m2", "M²", "", 1.0),
    ("beam_pol_angle_deg", "Polarization angle", "°", 0.0),
    ("beam_pol_ellipticity_deg", "Ellipticity", "°", 0.0),
];
const WAVEPLATE_PARAMS: [Param; 1] = [("wp_angle_deg", "Fast axis", "°", 0.0)];
const ISOLATOR_PARAMS: [Param; 1] = [("iso_angle_deg", "Input polarizer", "°", 0.0)];

type Param = (&'static str, &'static str, &'static str, f64);

/// The editable beam params of node `id` by its port kinds (a laser wins over the rest).
fn params_of(app: &AppState, id: NodeId) -> &'static [Param] {
    let def = app.session.doc.node(id).and_then(|n| app.catalog.get(&n.part));
    let kinds: Vec<_> = def.map(|d| d.optical_ports.iter().map(|p| PortKind::of(&p.kind, d)).collect()).unwrap_or_default();
    if kinds.contains(&PortKind::Source) {
        &PARAMS
    } else if kinds.iter().any(|k| matches!(k, PortKind::Hwp | PortKind::Qwp)) {
        &WAVEPLATE_PARAMS
    } else if kinds.contains(&PortKind::Isolator) {
        &ISOLATOR_PARAMS
    } else {
        &[]
    }
}

pub fn retrace(app: &mut AppState) {
    let catalog = app.catalog.clone();
    app.beams = lab_optics::trace_document(&app.session.doc, &|id| catalog.get(id).cloned());
}

/// The Beam section for node `id`: hidden for parts without optical ports.
pub fn fill(ui: &AppWindow, app: &AppState, models: &UiModels, id: NodeId) {
    let doc = &app.session.doc;
    let ports = doc.node(id).and_then(|n| app.catalog.get(&n.part)).map(|d| &d.optical_ports);
    let laser = is_laser(app, id);
    let node = doc.node(id);
    let fields = params_of(app, id).iter().map(|(key, label, unit, default)| {
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
    let on = is_on(app, id);
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

/// Whether node `id`'s part has a source optical port.
pub fn is_laser(app: &AppState, id: NodeId) -> bool {
    let def = app.session.doc.node(id).and_then(|n| app.catalog.get(&n.part));
    def.is_some_and(|d| d.optical_ports.iter().any(|p| PortKind::from_kind(&p.kind) == PortKind::Source))
}

/// Whether laser node `id` is emitting.
pub fn is_on(app: &AppState, id: NodeId) -> bool {
    let node = app.session.doc.node(id);
    !matches!(node.and_then(|n| n.params.get("beam_on")), Some(ParamValue::Flag(false)))
}

/// "62 % power"; a trace of light below 0.5 % reads "<1 % power", not "0 %".
pub fn power_label(power: f64) -> String {
    let percent = format!("{:.0}", power * 100.0);
    if power > 0.0 && percent == "0" { "<1 % power".into() } else { format!("{percent} % power") }
}

/// One line per beam pass that ends on node `id`.
fn describe(trace: &Trace, id: NodeId, app: &AppState) -> String {
    let doc = &app.session.doc;
    let several = doc.nodes.iter().filter(|n| is_laser(app, n.id)).count() > 1;
    let lines: Vec<String> = trace
        .segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.hit.as_ref().is_some_and(|(n, _)| *n == id))
        .enumerate()
        .map(|(k, (i, s))| {
            let name = if several { doc.node(s.source).map_or(String::new(), |n| format!("{}: ", n.name)) } else { String::new() };
            format!(
                "{name}Pass {} · Beam Ø {:.2} mm · {} from source · {} · {}",
                k + 1,
                2.0 * s.w_end,
                format_length_with_unit(trace.path_length(i), app.units()),
                s.polarization().label(),
                power_label(s.power)
            )
        })
        .collect();
    if lines.is_empty() { "No beam".into() } else { lines.join("\n") }
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
        params_of(app, id).iter().find(|p| p.0 == key).ok_or_else(|| EditError::UnknownField(field.into()))?;
    let angle = key.ends_with("_deg");
    let value: f64 = text
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && (angle || *v > 0.0))
        .ok_or_else(|| EditError::Invalid(format!("{label} must be a {}number.", if angle { "" } else { "positive " })))?;
    let value = match key {
        "beam_pol_ellipticity_deg" => value.clamp(-45.0, 45.0),
        _ if angle => value.rem_euclid(180.0) % 180.0,
        _ => value,
    };
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
    Ok(command(id, params, if on { "Turn Laser Off" } else { "Turn Laser On" }))
}

fn command(id: NodeId, params: lab_core::part::Params, label: &str) -> EditOutcome {
    EditOutcome::Command { command: Command::SetParams { id, params }, label: label.into(), select: None, fixes: Vec::new() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_set_params_knows_every_beam_param() {
        use lab_app::live::resolve::{LASER_PARAMS, OPTIC_PARAMS};
        let all = super::PARAMS.iter().chain(&super::WAVEPLATE_PARAMS).chain(&super::ISOLATOR_PARAMS);
        for (key, ..) in all {
            assert!(LASER_PARAMS.contains(key) || OPTIC_PARAMS.contains(key), "{key}");
        }
    }

    fn new_app(doc: Document) -> crate::app_state::AppState {
        let mut app = crate::app_state::AppState::new(lab_app::Session::new_clean(doc, None), lab_file::Settings::default());
        app.catalog = std::sync::Arc::new(Catalog::load_builtin().0);
        app
    }

    fn app_with(doc: Document) -> crate::app_state::AppState {
        let mut app = new_app(doc);
        super::retrace(&mut app);
        app
    }

    #[test]
    fn demo_inspector_shows_state_and_power() {
        let app = app_with(lab_core::sample::demo_document());
        let id = |name: &str| app.session.doc.nodes.iter().find(|n| n.name.starts_with(name) && !n.name.contains('·')).unwrap().id;
        let pbs = super::describe(&app.beams, id("PBS"), &app);
        assert!(pbs.contains("Linear") && pbs.contains("% power"), "{pbs}");
        let d1 = super::describe(&app.beams, id("D1"), &app);
        assert!(d1.contains("50 % power"), "{d1}");
    }

    #[test]
    fn power_label_shows_a_trace_of_light() {
        assert_eq!(super::power_label(0.004), "<1 % power");
        assert_eq!(super::power_label(0.0), "0 % power");
        assert_eq!(super::power_label(0.5), "50 % power");
        assert_eq!(super::power_label(0.005), "<1 % power");
        assert_eq!(super::power_label(0.006), "1 % power");
    }

    #[test]
    fn double_pass_lists_two_lines() {
        use lab_optics::{Field, Segment};
        let seg = |hit| Segment {
            source: NodeId(1), start: DVec3::ZERO, end: DVec3::X * 100.0, w_start: 0.5, w_end: 0.5, waist: None,
            hit: Some((hit, "p".into())), blocked: None, depth: 0, field: Field { re: DVec3::Y, im: DVec3::ZERO }, power: 1.0, q_start: (0.0, 1.0), lm2: 1e-3,
            frame_x: DVec3::X,
            path_start: 0.0,
        };
        let mut app = new_app(Document::new("x"));
        app.beams.segments = vec![seg(NodeId(7)), seg(NodeId(8)), seg(NodeId(7))];
        let text = super::describe(&app.beams, NodeId(7), &app);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("Pass 1") && lines[1].starts_with("Pass 2"), "{text}");
        assert_eq!(super::describe(&app.beams, NodeId(9), &app), "No beam");
    }

    /// The demo-style table with one laser at (0,-300) firing along +Y.
    fn laser_table(catalog: &Catalog) -> Document {
        let mut doc = Document::new("Pass");
        doc.platforms.push(Platform { select_lock: false, id: PlatformId(1), name: "Table 1".into(), kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless, outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) }, thickness: 100.0,
            top_height: 800.0, position: DVec2::ZERO, rotation_z: 0.0, hole_grid: Some(HoleGrid::metric()),
            support: Support::Legs { size: 120.0, inset: 150.0 } });
        let laser = lab_app::live::resolve::new_part_node(catalog, "generic:laser-head", 100, Parent::Platform(PlatformId(1)),
            Attach::Free, DVec3::new(0.0, -300.0, 0.0), DQuat::IDENTITY).unwrap();
        History::default().execute(&mut doc, Command::PlaceAssembly { nodes: vec![laser] }).unwrap();
        doc
    }

    /// A real double pass: a QWP at 45 in front of a mirror that sends the beam back through it.
    #[test]
    fn real_double_pass_changes_state() {
        let (catalog, _) = Catalog::load_builtin();
        let qwp_part = catalog.all().into_iter().find(|d| d.optical_ports.iter().any(|p| p.kind == "qwp")).unwrap().id.clone();
        let mut doc = laser_table(&catalog);
        let hit_twice = |d: &Document, id: NodeId| {
            let t = lab_optics::trace_document(d, &|p| catalog.get(p).cloned());
            t.segments.iter().filter(|s| s.hit.as_ref().is_some_and(|h| h.0 == id)).count() == 2
        };
        let qwp = aim(&mut doc, &catalog, &qwp_part, "thorlabs:FMP1", DVec2::new(0.0, -100.0), &|d, id| hits(d, &catalog, id));
        let mut params = doc.node(qwp).unwrap().params.clone();
        params.insert("wp_angle_deg".into(), ParamValue::Number(45.0));
        History::default().execute(&mut doc, Command::SetParams { id: qwp, params }).unwrap();
        aim(&mut doc, &catalog, "generic:mirror-1in", "thorlabs:KM100", DVec2::new(0.0, 150.0), &|d, _| hit_twice(d, qwp));
        let app = app_with(doc);
        let text = super::describe(&app.beams, qwp, &app);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains("Linear 0°"), "{text}");
        assert!(lines[1].contains("Circular"), "{text}");
    }

    /// Fields come from the part's port kinds: a laser wins, plates and isolators get their angle.
    #[test]
    fn params_follow_port_kinds() {
        let (builtin, _) = Catalog::load_builtin();
        let mut multi = builtin.get("generic:laser-head").unwrap().clone();
        multi.id = "test:laser-isolator".into();
        let port = builtin.get("generic:isolator").unwrap().optical_ports[0].clone();
        multi.optical_ports.push(port);
        let qwp_part = builtin.all().into_iter().find(|d| d.optical_ports.iter().any(|p| p.kind == "qwp")).unwrap().id.clone();
        // A catalog import with the legacy generic kind: read from its name.
        let mut legacy = builtin.get(&qwp_part).unwrap().clone();
        legacy.id = "test:legacy-hwp".into();
        legacy.name = "Zero-Order Half-Wave Plate, 850 nm".into();
        legacy.optical_ports[0].kind = "waveplate".into();
        let mut defs: Vec<_> = builtin.all().into_iter().cloned().collect();
        defs.push(multi);
        defs.push(legacy);
        let mut doc = laser_table(&builtin);
        let parts = ["generic:isolator", &qwp_part, "generic:pbs-cube-1in", "generic:mirror-1in", "test:laser-isolator", "generic:laser-head", "test:legacy-hwp"];
        let nodes: Vec<_> = parts.iter().enumerate().map(|(i, p)| {
            lab_app::live::resolve::new_part_node(&Catalog::from_defs(defs.clone()), p, 200 + i as u64, Parent::Platform(PlatformId(1)),
                Attach::Free, DVec3::new(100.0 * i as f64, 0.0, 0.0), DQuat::IDENTITY).unwrap()
        }).collect();
        History::default().execute(&mut doc, Command::PlaceAssembly { nodes }).unwrap();
        let mut app = new_app(doc);
        app.catalog = std::sync::Arc::new(Catalog::from_defs(defs));
        let keys = |i: u64| super::params_of(&app, NodeId(200 + i)).iter().map(|p| p.0).collect::<Vec<_>>();
        assert_eq!(keys(0), ["iso_angle_deg"]);
        assert_eq!(keys(1), ["wp_angle_deg"]);
        assert!(keys(2).is_empty() && keys(3).is_empty());
        assert_eq!(keys(4).len(), 5, "laser wins");
        assert_eq!(keys(5).len(), 5);
        assert_eq!(keys(6), ["wp_angle_deg"], "legacy waveplate read from its name");
    }

    #[test]
    fn angle_wrap_stays_below_180() {
        let mut app = app_with(lab_core::sample::demo_document());
        let hwp = app.session.doc.nodes.iter().find(|n| n.name.starts_with("HWP") && !n.name.contains('·')).unwrap().id;
        app.session.selection = Some(lab_core::SelectionTarget::Node(hwp));
        let Ok(lab_app::EditOutcome::Command { command: Command::SetParams { params, .. }, .. }) = super::field_edit(&app, "beam.wp_angle_deg", "-1e-20") else { panic!() };
        let v = params["wp_angle_deg"].as_f64().unwrap();
        assert!((0.0..180.0).contains(&v), "{v}");
    }

    #[test]
    fn angle_fields_wrap() {
        let doc = lab_core::sample::demo_document();
        let hwp = doc.nodes.iter().find(|n| n.name.starts_with("HWP") && !n.name.contains('·')).unwrap().id;
        let mut app = app_with(doc);
        app.session.selection = Some(lab_core::SelectionTarget::Node(hwp));
        let stored = |app: &crate::app_state::AppState, text: &str, field: &str| match super::field_edit(app, field, text) {
            Ok(lab_app::EditOutcome::Command { command: Command::SetParams { params, .. }, .. }) => {
                params.get(field.strip_prefix("beam.").unwrap()).and_then(|v| v.as_f64())
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(stored(&app, "400", "beam.wp_angle_deg"), Some(40.0));
        assert_eq!(stored(&app, "-30", "beam.wp_angle_deg"), Some(150.0));
        assert!(matches!(super::field_edit(&app, "beam.wp_angle_deg", "abc"), Err(lab_app::EditError::Invalid(_))));
        let laser = app.session.doc.nodes.iter().find(|n| super::is_laser(&app, n.id)).unwrap().id;
        app.session.selection = Some(lab_core::SelectionTarget::Node(laser));
        assert_eq!(stored(&app, "60", "beam.beam_pol_ellipticity_deg"), Some(45.0));
    }

    use lab_catalog::Catalog;
    use lab_core::glam::{DQuat, DVec2, DVec3};
    use lab_core::holes::HoleGrid;
    use lab_core::model::{Attach, Document, NodeId, Parent, Platform, PlatformId, PlatformKind, Support, Surface};
    use lab_core::outline::Outline;
    use lab_core::{Command, History};
    use lab_core::part::ParamValue;
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
        let mut nodes = vec![lab_app::live::resolve::new_part_node(catalog, &root, next, platform, Attach::Free, p, r).unwrap()];
        for (part, attach, position, rotation) in lab_viewport::stack_chain(catalog, &spec).unwrap() {
            let below = Parent::Node(nodes.last().unwrap().id);
            let id = next + nodes.len() as u64;
            nodes.push(lab_app::live::resolve::new_part_node(catalog, &part, id, below, attach, position, rotation).unwrap());
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

    /// POLA-101: the demo's M1 (a primitive-shape node) is traced, and a rotation typed in the
    /// inspector turns the beam leaving it.
    #[test]
    fn demo_mirror_rotation_moves_the_beam() {
        let (catalog, _) = Catalog::load_builtin();
        let mut doc = lab_core::sample::demo_document();
        let m1 = doc.nodes.iter().find(|n| n.name == "M1").unwrap().id;
        let trace = |d: &Document| lab_optics::trace_document(d, &|p| catalog.get(p).cloned());
        let after = |d: &Document| {
            let t = trace(d);
            let i = t.segments.iter().position(|s| s.hit.as_ref().is_some_and(|(n, _)| *n == m1)).expect("M1 not hit");
            let s = t.segments.iter().skip(i + 1).find(|s| s.start.distance(t.segments[i].end) < 1e-6).unwrap();
            (s.end - s.start).normalize()
        };
        assert!(trace(&doc).radius_at(m1).is_some());
        let before = after(&doc);
        let cx = lab_app::EditContext { doc: &doc, selection: Some(lab_core::SelectionTarget::Node(m1)), units: lab_core::units::LengthUnit::Millimetre };
        let lab_app::EditOutcome::Command { command, .. } = lab_app::node_field_edit(&cx, &catalog, "node.rotation", "120").unwrap() else {
            panic!("no command")
        };
        History::default().execute(&mut doc, command).unwrap();
        assert!(after(&doc).dot(before) < 0.99, "beam after M1 did not turn");
    }

    /// POLA-103: the demo beam is focused by L1, split by the PBS and ends on both detectors.
    #[test]
    fn demo_beam_ends_on_the_detectors() {
        let (catalog, _) = Catalog::load_builtin();
        let doc = lab_core::sample::demo_document();
        let t = lab_optics::trace_document(&doc, &|p| catalog.get(p).cloned());
        let id = |name: &str| doc.nodes.iter().find(|n| n.name.starts_with(name) && !n.name.contains('·')).unwrap().id;
        for name in ["M1", "M2", "L1", "HWP", "PBS", "D1", "D2"] {
            assert!(t.radius_at(id(name)).is_some(), "{name} not hit");
        }
        assert!(t.segments.iter().any(|s| s.waist.is_some()), "no focus after L1");
        let ends: Vec<_> = t.segments.iter().filter(|s| s.depth > 0).filter_map(|s| s.hit.as_ref().map(|h| h.0)).collect();
        assert!(ends.contains(&id("D1")) && ends.contains(&id("D2")));
    }

    /// Exit-walkthrough scene: laser (its 75 mm axis is at beam height), two mirror sets
    /// folding the beam, a lens set focusing it. `LABSETUP_BEAM_SCENE=<path>` writes it as a project.
    #[test]
    fn beam_through_mirror_and_lens_sets() {
        let (catalog, _) = Catalog::load_builtin();
        let mut doc = Document::new("Beams");
        doc.platforms.push(Platform { select_lock: false,
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
        let laser = lab_app::live::resolve::new_part_node(
            &catalog,
            "generic:laser-head",
            100,
            Parent::Platform(PlatformId(1)),
            Attach::Free,
            DVec3::new(0.0, -300.0, 0.0),
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
