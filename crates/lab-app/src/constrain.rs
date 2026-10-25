//! The constrain tool (M8-PLAN D) as the Constrain Components panel (POLA-147): pick a face or
//! axis per component; the constraint applies live as one gesture, OK keeps it, Cancel undoes it. The inspector's Constraints rows edit, re-solve and delete.

use std::cell::RefCell;
use std::rc::Rc;

use lab_app::{EditError, EditOutcome};
use lab_catalog::Catalog;
use lab_core::glam::DVec3;
use lab_core::part::{Feature, Geometry, PartDef};
use lab_core::units::{LengthUnit, format_angle_deg, format_length, parse_angle_deg, parse_length};
use lab_core::{Attach, Command, Constraint, ConstraintKind, Document, Node, Parent, SelectionTarget, compute_holes};
use lab_geom::TriMesh;
use lab_mates::constraints::{constrain, default_kind, solve};
use lab_ui::{AppWindow, ConstraintRow, EditField};
use lab_viewport::ViewportRequest;

use crate::app_state::AppState;
use crate::controller::{self, show_toast};
use crate::ui_models::UiModels;

const KINDS: [ConstraintKind; 4] = [ConstraintKind::Mate, ConstraintKind::Flush, ConstraintKind::Concentric, ConstraintKind::Angle];
const GESTURE: &str = "constrain";
/// A detected feature wins over the clicked triangles within 2 mm / 3°.
const SNAP_MM: f64 = 2.0;
const SNAP_DEG: f64 = 3.0;

/// The Constrain Components panel (POLA-147). Slot 0 = Component 1 (always a node), slot 1 =
/// Component 2 (node or platform); each with its picked feature.
#[derive(Default)]
struct Panel {
    open: bool,
    slots: [(Option<SelectionTarget>, Option<Feature>); 2],
    next: usize,
    kind: Option<ConstraintKind>,
    /// Constraints of Component 1 before the new one (re-applies start from these).
    base: Option<Vec<Constraint>>,
    /// The gesture has an undo entry.
    applied: bool,
    solve_all: bool,
    row: i32,
    models: Option<Rc<UiModels>>,
}

thread_local! {
    static PANEL: RefCell<Panel> = RefCell::new(Panel::default());
}

/// The tool was switched: Constrain opens the panel (Component 1 = the selected node), any other
/// tool closes it keeping what was applied (as OK).
pub fn reset(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, active: bool) {
    let open = PANEL.with(|p| p.borrow().open);
    if !active {
        if open {
            finish(ui, app, false);
        }
        return;
    }
    if open {
        return;
    }
    let selected = match app.borrow().session.selection {
        Some(SelectionTarget::Node(n)) => Some(n),
        _ => None,
    };
    PANEL.with(|p| {
        *p.borrow_mut() = Panel {
            open: true,
            slots: [(selected.map(SelectionTarget::Node), None), (None, None)],
            solve_all: ui.get_constrain_solve_all(),
            row: -1,
            models: Some(models.clone()),
            ..Panel::default()
        }
    });
    if let Some(n) = selected {
        crate::part_box::open(ui, app, n);
    }
    ui.set_constrain_open(true);
    show(ui, app);
}

/// The part box was closed (its ×): Cancel.
pub fn part_box_closed(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    if PANEL.with(|p| p.borrow().open) {
        finish(ui, app, true);
    }
}

/// OK (`undo` false) or Cancel: ends the gesture (Cancel undoes it), closes panel and part box.
fn finish(ui: &AppWindow, app: &Rc<RefCell<AppState>>, undo: bool) {
    let (applied, models) = PANEL.with(|p| {
        let mut p = p.borrow_mut();
        p.open = false;
        (std::mem::take(&mut p.applied), p.models.take())
    });
    app.borrow_mut().session.history.end_gesture();
    ui.set_constrain_open(false);
    crate::part_box::close(ui, app);
    controller::set_hint(ui, "");
    let Some(models) = models else { return };
    if undo && applied {
        controller::run_action(ui, app, &models, "edit.undo");
    } else {
        if applied {
            show_toast(ui, &app.borrow(), "Constrained", false);
        }
        controller::refresh(ui, app, &models);
    }
    if ui.get_tool() == 4 {
        controller::run_action(ui, app, &models, "tool.select");
    }
}

/// Pushes the panel state into the UI properties.
fn show(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let a = app.borrow();
    let doc = &a.session.doc;
    let name = |t: Option<SelectionTarget>| -> String {
        match t {
            Some(SelectionTarget::Node(n)) => doc.node(n).map(|n| n.name.clone()).unwrap_or_default(),
            Some(SelectionTarget::Platform(p)) => doc.platform(p).map(|p| p.name.clone()).unwrap_or_default(),
            _ => String::new(),
        }
    };
    let geometry = |f: &Option<Feature>| match f {
        Some(Feature::Plane { .. }) => "Face",
        Some(Feature::Axis { .. }) => "Axis",
        None => "",
    };
    PANEL.with(|p| {
        let p = p.borrow();
        ui.set_constrain_name1(name(p.slots[0].0).into());
        ui.set_constrain_geometry1(geometry(&p.slots[0].1).into());
        ui.set_constrain_name2(name(p.slots[1].0).into());
        ui.set_constrain_geometry2(geometry(&p.slots[1].1).into());
        ui.set_constrain_next(p.next as i32);
        ui.set_constrain_kind(p.kind.and_then(|k| KINDS.iter().position(|x| *x == k)).map_or(-1, |i| i as i32));
        ui.set_constrain_solve_all(p.solve_all);
        ui.set_constrain_row(p.row);
        let hint = match p.slots[p.next].0 {
            None => format!("Click Component {} in the view", p.next + 1),
            Some(_) => format!("Pick a face or axis for Component {} in the box", p.next + 1),
        };
        controller::set_hint(ui, &hint);
    });
}

/// A click with the constrain tool (POLA-159): in the main view it chooses a component (a
/// platform also its hole/plane, the box cannot show one); in the part box it picks the feature.
pub fn on_feature_point(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    target: SelectionTarget,
    point: DVec3,
    normal: DVec3,
    in_box: bool,
) {
    if !PANEL.with(|p| p.borrow().open) {
        return;
    }
    let resolved = || {
        let a = app.borrow();
        resolve(&a.session.doc, &a.catalog, target, point, normal)
    };
    if in_box {
        let Some(feature) = resolved() else {
            show_toast(ui, &app.borrow(), "No face or axis there", false);
            return;
        };
        let other = PANEL.with(|p| {
            let mut p = p.borrow_mut();
            let next = p.next;
            let slot = if p.slots[next].0 == Some(target) { Some(next) } else { p.slots.iter().position(|s| s.0 == Some(target)) };
            let slot = slot?;
            p.slots[slot].1 = Some(feature);
            let other = 1 - slot;
            p.slots[other].1.is_none().then(|| {
                p.next = other;
                p.slots[other].0
            })
        });
        if let Some(Some(SelectionTarget::Node(n))) = other {
            crate::part_box::open(ui, app, n);
        }
    } else {
        match target {
            SelectionTarget::Node(id) => {
                let c1 = PANEL.with(|p| p.borrow().slots[0].0);
                if c1 == Some(target) {
                    PANEL.with(|p| p.borrow_mut().next = 0);
                } else if c1.is_none() {
                    if app.borrow().session.doc.node(id).is_some_and(|n| n.locked) {
                        show_toast(ui, &app.borrow(), "Part is locked", false);
                        return;
                    }
                    PANEL.with(|p| {
                        let mut p = p.borrow_mut();
                        // A new Component 1 starts a new constraint.
                        p.base = None;
                        p.row = -1;
                        p.slots[0] = (Some(target), None);
                        p.next = 0;
                    });
                    {
                        let mut a = app.borrow_mut();
                        a.session.history.end_gesture();
                        a.session.selection = Some(target);
                        a.send(ViewportRequest::Select(Some(target)));
                    }
                    controller::refresh(ui, app, models);
                } else {
                    PANEL.with(|p| {
                        let mut p = p.borrow_mut();
                        p.slots[1] = (Some(target), None);
                        p.next = 1;
                    });
                }
                crate::part_box::open(ui, app, id);
            }
            SelectionTarget::Platform(_) => {
                let Some(feature) = resolved() else {
                    show_toast(ui, &app.borrow(), "No face or axis there", false);
                    return;
                };
                PANEL.with(|p| {
                    let mut p = p.borrow_mut();
                    p.slots[1] = (Some(target), Some(feature));
                    p.next = if p.slots[0].1.is_none() { 0 } else { 1 };
                });
            }
            _ => return,
        }
    }
    let both = PANEL.with(|p| p.borrow().slots.iter().all(|s| s.1.is_some()));
    if both {
        apply_new(ui, app, models, None);
    }
    show(ui, app);
}

/// Applies (or re-applies) the new constraint from the two picks: `kind` or `default_kind`.
fn apply_new(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, kind: Option<ConstraintKind>) {
    let (Some(SelectionTarget::Node(child)), Some(child_feature), Some(target), Some(feature), base) = PANEL.with(|p| {
        let p = p.borrow();
        (p.slots[0].0, p.slots[0].1.clone(), p.slots[1].0, p.slots[1].1.clone(), p.base.clone())
    }) else {
        return;
    };
    let parent = match target {
        SelectionTarget::Node(n) => Parent::Node(n),
        SelectionTarget::Platform(p) => Parent::Platform(p),
        _ => return,
    };
    let Some(default) = default_kind(&child_feature, &feature) else {
        show_toast(ui, &app.borrow(), "Pick two faces or two axes", false);
        return;
    };
    let kind = kind.unwrap_or(default);
    let (command, base) = {
        let a = app.borrow();
        let doc = &a.session.doc;
        if let Parent::Node(n) = parent
            && (n == child || doc.descendants(child).contains(&n))
        {
            drop(a);
            show_toast(ui, &app.borrow(), "Pick a part this one does not carry", false);
            return;
        }
        let Some(node) = doc.node(child) else { return };
        // Least motion: keep whichever end of the child axis points along the parent axis now
        // (fitted axis signs are not "out of material").
        let flip = match (&child_feature, &feature) {
            (Feature::Axis { dir: cd, .. }, Feature::Axis { dir: pd, .. }) => {
                let world = |at| lab_mates::constraints::world_pose(doc, at).map(|(_, q)| q);
                match (world(Parent::Node(child)), world(parent)) {
                    (Some(cq), Some(pq)) => (cq * *cd).dot(pq * *pd) < 0.0,
                    _ => false,
                }
            }
            _ => false,
        };
        let new = Constraint { kind, child: child_feature, parent: feature, offset: 0.0, angle: 0.0, flip, lock_rotation: false };
        let base = base.unwrap_or_else(|| match &node.attach {
            Attach::Constrained { constraints } if node.parent == parent => constraints.clone(),
            _ => Vec::new(),
        });
        let mut constraints = base.clone();
        constraints.push(new);
        (constrain(doc, child, parent, constraints), base)
    };
    let Some(command) = command else { return };
    PANEL.with(|p| {
        let mut p = p.borrow_mut();
        p.kind = Some(kind);
        p.row = base.len() as i32;
        p.base = Some(base);
    });
    live(ui, app, models, command, "Constrain".into());
}

/// Runs `command` inside the open constrain gesture (one undo step until OK / `+`).
fn live(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, command: Command, label: String) -> bool {
    let result = {
        let mut a = app.borrow_mut();
        let session = &mut a.session;
        let result = session.history.execute_coalescing(&mut session.doc, command, label, GESTURE);
        result.map(|changes| (!changes.is_empty()).then(|| (Box::new(session.doc.clone()), changes)))
    };
    match result {
        Ok(Some((snapshot, changes))) => {
            PANEL.with(|p| p.borrow_mut().applied = true);
            app.borrow().send(ViewportRequest::ApplyChanges { snapshot, changes });
            controller::refresh(ui, app, models);
            crate::files::arm_autosave(app);
            true
        }
        Ok(None) => true,
        Err(error) => {
            show_toast(ui, &app.borrow(), &error.to_string(), false);
            false
        }
    }
}

/// Runs an inspector-style edit outcome inside the gesture.
fn live_outcome(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, outcome: Result<EditOutcome, EditError>) -> bool {
    match outcome {
        Ok(EditOutcome::Command { command, label, .. }) => live(ui, app, models, command, label),
        Ok(_) => true,
        Err(error) => {
            show_toast(ui, &app.borrow(), &error.to_string(), false);
            false
        }
    }
}

/// `cpanel.constraint.<i>.offset` committed in the panel table.
pub fn panel_field(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str, text: &str) -> bool {
    let id = id.trim_start_matches("cpanel.");
    let outcome = field_edit(&app.borrow(), id, text);
    live_outcome(ui, app, models, outcome)
}

/// `cpanel.*` clicked.
pub fn panel_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str) {
    let id = id.trim_start_matches("cpanel.");
    match id {
        "ok" => return finish(ui, app, false),
        "cancel" => return finish(ui, app, true),
        "select.0" | "select.1" => {
            let slot = if id == "select.0" { 0 } else { 1 };
            let target = PANEL.with(|p| {
                let mut p = p.borrow_mut();
                p.next = slot;
                p.slots[slot].0
            });
            if let Some(SelectionTarget::Node(n)) = target {
                crate::part_box::open(ui, app, n);
            }
        }
        "solve_all" => PANEL.with(|p| {
            let mut p = p.borrow_mut();
            p.solve_all = !p.solve_all;
        }),
        "add" => {
            // Keep what was applied as its own step; the next pair starts a new constraint.
            app.borrow_mut().session.history.end_gesture();
            PANEL.with(|p| {
                let mut p = p.borrow_mut();
                p.slots[1] = (None, None);
                p.base = None;
                p.kind = None;
                p.next = 1;
                p.row = -1;
            });
        }
        "delete" => {
            let row = PANEL.with(|p| p.borrow().row);
            if row >= 0 {
                let outcome = action_edit(&app.borrow(), &format!("constraint.{row}.delete"));
                if live_outcome(ui, app, models, outcome) {
                    PANEL.with(|p| {
                        let mut p = p.borrow_mut();
                        p.row = -1;
                        // The table changed under the new constraint: picks start over.
                        p.base = None;
                        p.slots[1] = (None, None);
                        p.next = 1;
                    });
                }
            }
        }
        _ if id.starts_with("row.") => {
            let row = id[4..].parse().unwrap_or(-1);
            PANEL.with(|p| p.borrow_mut().row = row);
        }
        _ if id.starts_with("kind.") => {
            let Some(&kind) = id[5..].parse::<usize>().ok().and_then(|i| KINDS.get(i)) else { return };
            if PANEL.with(|p| p.borrow().base.is_some()) {
                apply_new(ui, app, models, Some(kind));
            } else {
                PANEL.with(|p| p.borrow_mut().kind = Some(kind));
            }
        }
        _ if id.starts_with("constraint.") => {
            let outcome = action_edit(&app.borrow(), id);
            live_outcome(ui, app, models, outcome);
        }
        _ => {}
    }
    show(ui, app);
}

/// The feature under a click, in the target's frame: a detected feature of the part when one is
/// close, else the one fitted to the clicked triangles; a platform's nearest hole or top plane.
fn resolve(doc: &Document, catalog: &Catalog, target: SelectionTarget, point: DVec3, normal: DVec3) -> Option<Feature> {
    match target {
        SelectionTarget::Node(id) => {
            let node = doc.node(id)?;
            let def = catalog.get(&node.part)?;
            snap(&def.features, point, normal)
                .or_else(|| lab_import::features::pick_feature(&part_mesh(def, node)?, point, normal))
        }
        SelectionTarget::Platform(pid) => {
            let holes = compute_holes(doc.platform(pid)?).ok()?;
            let radius = holes.thread_radius;
            Some(match lab_mates::snap(&holes, point.truncate(), radius) {
                Some(hit) => Feature::Axis { origin: hit.position.extend(0.0), dir: DVec3::Z, radius, hole: true },
                None => Feature::Plane { origin: point.truncate().extend(0.0), normal: DVec3::Z },
            })
        }
        _ => None,
    }
}

fn snap(features: &[Feature], point: DVec3, normal: DVec3) -> Option<Feature> {
    let (cos, sin) = (SNAP_DEG.to_radians().cos(), SNAP_DEG.to_radians().sin());
    features
        .iter()
        .filter_map(|f| {
            let d = match f {
                Feature::Plane { origin, normal: n } => {
                    let n = n.normalize_or_zero();
                    if n.dot(normal) < cos {
                        return None;
                    }
                    (point - *origin).dot(n).abs()
                }
                Feature::Axis { origin, dir, radius, .. } => {
                    let d = dir.normalize_or_zero();
                    if d.dot(normal).abs() > sin {
                        return None;
                    }
                    let r = point - *origin;
                    ((r - d * r.dot(d)).length() - radius).abs()
                }
            };
            (d <= SNAP_MM).then_some((d, f))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f.clone())
}

/// The node's CPU mesh in its part frame (generated with its parameter overrides, or LOD0).
pub(crate) fn part_mesh(def: &PartDef, node: &Node) -> Option<TriMesh> {
    let Geometry::Parametric { generator, params } = &def.geometry else {
        return crate::wizard::part_mesh(def).ok();
    };
    let mut all = params.clone();
    all.extend(node.params.clone());
    let mut mesh = TriMesh::default();
    for (_, m) in &lab_parts::generate(generator, &all).ok()?.pieces {
        mesh.append(m);
    }
    Some(mesh)
}

// ── Inspector ─────────────────────────────────────────────────────────────────────────────

fn kind_label(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Mate => "Mate",
        ConstraintKind::Flush => "Flush",
        ConstraintKind::Concentric => "Concentric",
        ConstraintKind::Angle => "Angle",
    }
}

/// The kinds a pair can take, in the order the kind button cycles through.
fn valid_kinds(c: &Constraint) -> &'static [ConstraintKind] {
    match c.child {
        Feature::Plane { .. } => &[ConstraintKind::Mate, ConstraintKind::Flush, ConstraintKind::Angle],
        Feature::Axis { .. } => &[ConstraintKind::Concentric, ConstraintKind::Angle],
    }
}

/// The Constraints rows of a node (empty unless it is constrained).
pub fn rows(node: &Node, units: LengthUnit) -> Vec<ConstraintRow> {
    let Attach::Constrained { constraints } = &node.attach else { return Vec::new() };
    let field = |id: String, label: &str, value: String, unit: &str| EditField {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        unit: unit.into(),
        enabled: true,
        invalid: false,
    };
    constraints
        .iter()
        .enumerate()
        .map(|(i, c)| ConstraintRow {
            index: i as i32,
            kind: kind_label(c.kind).into(),
            offset: field(format!("constraint.{i}.offset"), "Offset", format_length(c.offset, units), units.suffix()),
            angle: field(format!("constraint.{i}.angle"), "Angle", format_angle_deg(c.angle), "°"),
            flip: c.flip,
            lock: c.lock_rotation,
        })
        .collect()
}

/// `constraint.<i>.offset|angle` committed.
pub fn field_edit(a: &AppState, id: &str, text: &str) -> Result<EditOutcome, EditError> {
    let units = a.units();
    let value = if id.ends_with(".offset") { parse_length(text.trim(), units)? } else { parse_angle_deg(text.trim())? };
    edit(a, id, |cons, i| {
        if id.ends_with(".offset") {
            if format_length(cons[i].offset, units) != format_length(value, units) {
                cons[i].offset = value;
            }
        } else if format_angle_deg(cons[i].angle) != format_angle_deg(value) {
            cons[i].angle = value;
        }
    })
}

/// `constraint.<i>.kind|flip|lock|delete` clicked.
pub fn action_edit(a: &AppState, id: &str) -> Result<EditOutcome, EditError> {
    edit(a, id, |cons, i| match id.rsplit('.').next() {
        Some("kind") => {
            let kinds = valid_kinds(&cons[i]);
            let at = kinds.iter().position(|k| *k == cons[i].kind).map_or(0, |p| (p + 1) % kinds.len());
            cons[i].kind = kinds[at];
        }
        Some("flip") => cons[i].flip = !cons[i].flip,
        Some("lock") => cons[i].lock_rotation = !cons[i].lock_rotation,
        Some("delete") => {
            cons.remove(i);
        }
        _ => {}
    })
}

/// Applies `change` to constraint `<i>` of the selected node and re-solves from its current pose
/// (one `SetAttach`). No constraints left: the node is free where it stands.
fn edit(a: &AppState, id: &str, change: impl FnOnce(&mut Vec<Constraint>, usize)) -> Result<EditOutcome, EditError> {
    let Some(SelectionTarget::Node(nid)) = a.session.selection else { return Err(EditError::Stale) };
    let node = a.session.doc.node(nid).ok_or(EditError::Stale)?;
    let Attach::Constrained { constraints } = &node.attach else { return Err(EditError::Stale) };
    let i = id.split('.').nth(1).and_then(|s| s.parse::<usize>().ok()).filter(|&i| i < constraints.len()).ok_or(EditError::Stale)?;
    let mut cons = constraints.clone();
    change(&mut cons, i);
    if cons == *constraints {
        return Ok(EditOutcome::Nothing);
    }
    let (attach, (position, rotation)) = if cons.is_empty() {
        (Attach::Free, (node.position, node.rotation))
    } else {
        let pose = solve((node.position, node.rotation), &cons);
        (Attach::Constrained { constraints: cons }, pose)
    };
    let command = Command::SetAttach { id: nid, parent: node.parent, attach, position, rotation };
    Ok(EditOutcome::Command { command, label: format!("Edit {}", node.name), select: None, fixes: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_prefers_detected_hole_and_matching_plane() {
        let features = vec![
            Feature::Plane { origin: DVec3::new(0.0, 0.0, 10.0), normal: DVec3::Z },
            Feature::Axis { origin: DVec3::ZERO, dir: DVec3::Z, radius: 3.0, hole: true },
        ];
        // On the bore wall (normal towards the axis).
        let hit = snap(&features, DVec3::new(3.5, 0.0, 5.0), -DVec3::X);
        assert!(matches!(hit, Some(Feature::Axis { .. })), "{hit:?}");
        // On the top face.
        let hit = snap(&features, DVec3::new(8.0, 8.0, 10.5), DVec3::Z);
        assert!(matches!(hit, Some(Feature::Plane { .. })), "{hit:?}");
        // A side wall matches neither.
        assert!(snap(&features, DVec3::new(20.0, 0.0, 5.0), DVec3::X).is_none());
    }
}
