//! The constrain tool (M8-PLAN D): pick a face or axis on the part to move, then one on a part
//! or platform to constrain it to. The inspector's Constraints rows edit, re-solve and delete.

use std::cell::RefCell;
use std::rc::Rc;

use lab_app::{EditError, EditOutcome};
use lab_catalog::Catalog;
use lab_core::glam::DVec3;
use lab_core::part::{Feature, Geometry, PartDef};
use lab_core::units::{LengthUnit, format_angle_deg, format_length, parse_angle_deg, parse_length};
use lab_core::{Attach, Command, Constraint, ConstraintKind, Document, Node, NodeId, Parent, SelectionTarget, compute_holes};
use lab_geom::TriMesh;
use lab_mates::constraints::{constrain, default_kind, solve};
use lab_ui::{AppWindow, ConstraintRow, EditField};
use lab_viewport::ViewportRequest;

use crate::app_state::AppState;
use crate::controller::{self, show_toast};
use crate::ui_models::UiModels;

const HINT_FIRST: &str = "Pick a face or axis on the part to move";
const HINT_SECOND: &str = "Pick a face or axis to constrain to";
/// A detected feature wins over the clicked triangles within 2 mm / 3°.
const SNAP_MM: f64 = 2.0;
const SNAP_DEG: f64 = 3.0;

thread_local! {
    /// The moving part and its picked feature, after the first pick.
    static FIRST: RefCell<Option<(NodeId, Feature)>> = const { RefCell::new(None) };
}

/// The tool was switched (to any tool): forget the first pick and show the matching hint.
/// `SetTool` clears the viewport highlight.
pub fn reset(ui: &AppWindow, active: bool) {
    FIRST.with(|f| *f.borrow_mut() = None);
    controller::set_hint(ui, if active { HINT_FIRST } else { "" });
}

pub fn on_feature_point(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    target: SelectionTarget,
    point: DVec3,
    normal: DVec3,
) {
    let feature = {
        let a = app.borrow();
        resolve(&a.session.doc, &a.catalog, target, point, normal)
    };
    let Some(feature) = feature else {
        show_toast(ui, &app.borrow(), "No face or axis there", false);
        return;
    };
    let Some((child, child_feature)) = FIRST.with(|f| f.borrow().clone()) else {
        let SelectionTarget::Node(id) = target else {
            show_toast(ui, &app.borrow(), HINT_FIRST, false);
            return;
        };
        if app.borrow().session.doc.node(id).is_some_and(|n| n.locked) {
            show_toast(ui, &app.borrow(), "Part is locked", false);
            return;
        }
        FIRST.with(|f| *f.borrow_mut() = Some((id, feature.clone())));
        controller::set_hint(ui, HINT_SECOND);
        app.borrow().send(ViewportRequest::HighlightFeature(Some((target, feature))));
        return;
    };
    let parent = match target {
        SelectionTarget::Node(n) => Parent::Node(n),
        SelectionTarget::Platform(p) => Parent::Platform(p),
        _ => return,
    };
    let Some(kind) = default_kind(&child_feature, &feature) else {
        show_toast(ui, &app.borrow(), "Pick two faces or two axes", false);
        return;
    };
    let command = {
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
        let mut constraints = match &node.attach {
            Attach::Constrained { constraints } if node.parent == parent => constraints.clone(),
            _ => Vec::new(),
        };
        constraints.push(new);
        constrain(doc, child, parent, constraints)
    };
    let Some(command) = command else { return };
    controller::execute(ui, app, models, "tool.constrain", command, "Constrain".into(), Some(SelectionTarget::Node(child)), Vec::new());
    controller::run_action(ui, app, models, "tool.select");
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
fn part_mesh(def: &PartDef, node: &Node) -> Option<TriMesh> {
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
