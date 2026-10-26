//! The joint tool (docs/M9-PLAN.md E): hover shows the snap under the pointer, the first pick is
//! the joint origin on the part to move (the part box opens), the second the origin to join to.
//! The joint then applies for real as one coalescing gesture that the floating panel edits;
//! OK keeps it, Cancel undoes it. The inspector's Joint section edits a placed joint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::rc::Rc;

use lab_core::glam::{DMat3, DQuat, DVec3};
use lab_core::part::Frame;
use lab_core::units::{LengthUnit, format_angle_deg, format_length, parse_angle_deg, parse_length};
use lab_core::{Attach, Command, Document, Joint, JointKind, Limit, Node, NodeId, Parent, SelectionTarget, compute_holes};
use lab_import::snaps::{SnapKind, SnapMesh};
use lab_mates::constraints::world_pose;
use lab_mates::joints::{Dof, dofs, rigid_group};
use lab_ui::{AppWindow, EditField};
use lab_viewport::{SnapGlyph, ViewportRequest};
use slint::SharedString;

use crate::app_state::AppState;
use crate::controller::{self, show_toast};
use crate::files;
use crate::ui_models::UiModels;

const HINT_FIRST: &str = "Pick a joint origin on the part to move";
const HINT_SECOND: &str = "Pick the origin to join to";
/// Snap search radius round the pointer, mm.
const SNAP_MM: f64 = 3.0;
const GESTURE: &str = "joint";
const KINDS: [JointKind; 7] = [
    JointKind::Rigid,
    JointKind::Revolute,
    JointKind::Slider,
    JointKind::Cylindrical,
    JointKind::PinSlot,
    JointKind::Planar,
    JointKind::Ball,
];

#[derive(Default)]
struct State {
    active: bool,
    /// The part to move, its picked origin and whether that was an axis (edge centre).
    first: Option<(NodeId, Frame, bool)>,
    /// The node whose new joint is being placed (panel open); `applied` once the gesture has
    /// an undo entry.
    gesture: Option<NodeId>,
    applied: bool,
    /// Welded part meshes (part frame) by asset: hover snaps reuse them.
    meshes: HashMap<String, Rc<SnapMesh>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The tool was switched (to any tool). A joint being placed is kept (as OK).
pub fn reset(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, active: bool) {
    let had_first = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.active = active;
        s.meshes.clear();
        s.first.take().is_some()
    });
    if had_first {
        crate::part_box::close(ui, app);
    }
    if STATE.with(|s| s.borrow().gesture.is_some()) {
        finish(ui, app, models, false);
    }
    if active {
        controller::set_hint(ui, HINT_FIRST);
    }
}

pub fn is_active() -> bool {
    STATE.with(|s| s.borrow().active)
}

pub fn on_feature_hover(app: &Rc<RefCell<AppState>>, target: SelectionTarget, point: DVec3, normal: DVec3) {
    if !is_active() {
        return crate::constrain::on_feature_hover(app, target, point, normal);
    }
    let snap = resolve(&app.borrow(), target, point, normal);
    app.borrow().send(ViewportRequest::HighlightSnap(snap.map(|(frame, kind)| (target, frame, glyph(kind)))));
}

pub fn on_feature_point(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    target: SelectionTarget,
    point: DVec3,
    normal: DVec3,
) {
    let Some((frame, kind)) = resolve(&app.borrow(), target, point, normal) else {
        show_toast(ui, &app.borrow(), "No joint origin there", false);
        return;
    };
    let axis = kind == SnapKind::EdgeCentre;
    let first = STATE.with(|s| s.borrow().first);
    // First pick, or body 1 again: (re)place origin 1.
    if let SelectionTarget::Node(id) = target
        && first.is_none_or(|(f, ..)| f == id)
    {
        if app.borrow().session.doc.node(id).is_some_and(|n| n.locked) {
            show_toast(ui, &app.borrow(), "Part is grounded", false);
            return;
        }
        STATE.with(|s| s.borrow_mut().first = Some((id, frame, axis)));
        if first.is_none() {
            crate::part_box::open(ui, app, id);
        }
        controller::set_hint(ui, HINT_SECOND);
        return;
    }
    let Some((child, child_frame, child_axis)) = first else {
        show_toast(ui, &app.borrow(), HINT_FIRST, false);
        return;
    };
    let parent = match target {
        SelectionTarget::Node(n) => Parent::Node(n),
        SelectionTarget::Platform(p) => Parent::Platform(p),
        _ => return,
    };
    let command = {
        let a = app.borrow();
        let doc = &a.session.doc;
        if let Parent::Node(n) = parent
            && doc.descendants(child).contains(&n)
        {
            drop(a);
            show_toast(ui, &app.borrow(), "Pick a part this one does not carry", false);
            return;
        }
        let mut j = Joint {
            kind: if child_axis && axis { JointKind::Revolute } else { JointKind::Rigid },
            child: child_frame,
            parent: frame,
            offset: DVec3::ZERO,
            angle: 0.0,
            // Snap z points out of the material on both sides: mating faces run against each other.
            flip: true,
            values: [0.0; 3],
            limits: [None; 3],
            suppressed: false,
            seq: 0,
        };
        fit_start(doc, child, parent, &mut j);
        lab_mates::joints::joint(doc, child, parent, j)
    };
    let Some(command) = command else { return };
    // Switching to Select closes the part box and clears the first pick.
    controller::run_action(ui, app, models, "tool.select");
    let label = format!("Joint {}", label_of(kind_of(&command)));
    {
        let mut a = app.borrow_mut();
        a.session.history.end_gesture();
        a.session.selection = Some(SelectionTarget::Node(child));
        a.send(ViewportRequest::Select(Some(SelectionTarget::Node(child))));
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.gesture = Some(child);
        s.applied = false;
    });
    ui.set_joint_editing(true);
    live(ui, app, models, command, label);
}

/// OK (`keep`) or Cancel: ends the gesture; Cancel undoes it.
fn finish(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, undo: bool) {
    let applied = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.gesture = None;
        std::mem::take(&mut s.applied)
    });
    app.borrow_mut().session.history.end_gesture();
    ui.set_joint_editing(false);
    if undo && applied {
        controller::run_action(ui, app, models, "edit.undo");
    } else {
        controller::refresh(ui, app, models);
    }
}

/// Runs `command` inside the open joint gesture (one undo step for the whole placement).
fn live(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, command: Command, label: String) -> bool {
    let result = {
        let mut a = app.borrow_mut();
        let session = &mut a.session;
        let result = session.history.execute_coalescing(&mut session.doc, command, label, GESTURE);
        result.map(|changes| (!changes.is_empty()).then(|| (Box::new(session.doc.clone()), changes)))
    };
    match result {
        Ok(Some((snapshot, changes))) => {
            STATE.with(|s| s.borrow_mut().applied = true);
            app.borrow().send(ViewportRequest::ApplyChanges { snapshot, changes });
            controller::refresh(ui, app, models);
            files::arm_autosave(app);
            true
        }
        Ok(None) => true,
        Err(error) => {
            show_toast(ui, &app.borrow(), &error.to_string(), false);
            false
        }
    }
}

/// Snap frame (target's frame) and kind under a hit: a part's snap, a platform's nearest hole
/// (z up), else the hit point on the platform top.
fn resolve(a: &AppState, target: SelectionTarget, point: DVec3, normal: DVec3) -> Option<(Frame, SnapKind)> {
    let doc = &a.session.doc;
    match target {
        SelectionTarget::Node(id) => {
            let node = doc.node(id)?;
            let def = a.catalog.get(&node.part)?;
            let key = match &def.geometry {
                lab_core::part::Geometry::Mesh { asset } => asset.clone(),
                _ => format!("{}#{:?}", node.part, node.params),
            };
            let cached = STATE.with(|s| s.borrow().meshes.get(&key).cloned());
            let mesh = match cached {
                Some(m) => m,
                None => {
                    let m = Rc::new(SnapMesh::new(&crate::constrain::part_mesh(def, node)?));
                    STATE.with(|s| s.borrow_mut().meshes.insert(key, m.clone()));
                    m
                }
            };
            mesh.snap_at(point, normal, SNAP_MM).map(|s| (s.frame, s.kind))
        }
        SelectionTarget::Platform(pid) => {
            let holes = compute_holes(doc.platform(pid)?).ok()?;
            let up = |origin: DVec3| Frame { origin, z: DVec3::Z, x: DVec3::X };
            Some(match lab_mates::snap(&holes, point.truncate(), holes.pitch.max_element()) {
                Some(hit) => (up(hit.position.extend(0.0)), SnapKind::EdgeCentre),
                None => (up(point.truncate().extend(0.0)), SnapKind::FaceCentre),
            })
        }
        _ => None,
    }
}

fn glyph(kind: SnapKind) -> SnapGlyph {
    match kind {
        SnapKind::EdgeCentre => SnapGlyph::Circle,
        SnapKind::Vertex | SnapKind::MidEdge => SnapGlyph::Dot,
        SnapKind::FaceCentre => SnapGlyph::Square,
    }
}

/// Sets the joint's DOF values (and `angle` when the kind has no RotZ) so the child stays where
/// it is now as far as the joint allows: its current pose projected onto the DOFs.
fn fit_start(doc: &Document, child: NodeId, parent: Parent, j: &mut Joint) {
    let (Some((cp_w, cq_w)), Some((pp_w, pq_w))) = (world_pose(doc, Parent::Node(child)), world_pose(doc, parent)) else {
        return;
    };
    let inv = pq_w.inverse();
    let (pc, qc) = (inv * (cp_w - pp_w), inv * cq_w);
    let (pp, pq) = frame_pose(&j.parent);
    let (cp, cq) = frame_pose(&j.child);
    let flip = if j.flip { DQuat::from_rotation_x(PI) } else { DQuat::IDENTITY };
    // pq · Rz(angle + θ) · flip · cq⁻¹ ≈ qc: the twist of D about z.
    let d = pq.inverse() * qc * cq * flip.inverse();
    let twist = wrap(2.0 * d.z.atan2(d.w));
    let ds = dofs(j.kind);
    j.values = [0.0; 3];
    match ds.iter().position(|d| *d == Dof::RotZ) {
        Some(i) => j.values[i] = wrap(twist - j.angle),
        None => j.angle = twist,
    }
    let local = DQuat::from_rotation_z(j.angle).inverse() * (pq.inverse() * (pc + qc * cp - pp) - j.offset);
    for (i, d) in ds.iter().enumerate() {
        match d {
            Dof::SlideX => j.values[i] = local.x,
            Dof::SlideY => j.values[i] = local.y,
            Dof::SlideZ => j.values[i] = local.z,
            _ => {}
        }
    }
}

fn wrap(a: f64) -> f64 {
    let a = a.rem_euclid(TAU);
    if a > PI { a - TAU } else { a }
}

/// As `lab_mates::joints` poses a frame.
fn frame_pose(f: &Frame) -> (DVec3, DQuat) {
    let z = f.z.normalize_or(DVec3::Z);
    let x = (f.x - z * f.x.dot(z)).normalize_or(z.any_orthonormal_vector());
    (f.origin, DQuat::from_mat3(&DMat3::from_cols(x, z.cross(x), z)))
}

fn kind_of(command: &Command) -> JointKind {
    match command {
        Command::SetAttach { attach: Attach::Joint(j), .. } => j.kind,
        _ => JointKind::Rigid,
    }
}

fn label_of(kind: JointKind) -> &'static str {
    match kind {
        JointKind::Rigid => "Rigid",
        JointKind::Revolute => "Revolute",
        JointKind::Slider => "Slider",
        JointKind::Cylindrical => "Cylindrical",
        JointKind::PinSlot => "Pin-Slot",
        JointKind::Planar => "Planar",
        JointKind::Ball => "Ball",
    }
}

fn dof_label(d: Dof) -> &'static str {
    match d {
        Dof::RotZ => "Rotate Z",
        Dof::RotX => "Rotate X",
        Dof::RotY => "Rotate Y",
        Dof::SlideX => "Slide X",
        Dof::SlideY => "Slide Y",
        Dof::SlideZ => "Slide Z",
    }
}

fn is_rot(d: Dof) -> bool {
    matches!(d, Dof::RotX | Dof::RotY | Dof::RotZ)
}

// ── Panel and inspector ───────────────────────────────────────────────────────────────────

/// The node the Joint fields show: the joint being placed, else the selected jointed node.
fn shown(a: &AppState) -> Option<(&Node, &Joint)> {
    let id = STATE.with(|s| s.borrow().gesture).or(match a.session.selection {
        Some(SelectionTarget::Node(id)) => Some(id),
        _ => None,
    })?;
    let node = a.session.doc.node(id)?;
    match &node.attach {
        Attach::Joint(j) => Some((node, j)),
        _ => None,
    }
}

/// Fills the panel / inspector Joint fields (cleared when nothing jointed is shown). Drops a
/// gesture whose node is gone or no longer jointed (undo, file open).
pub fn fill(ui: &AppWindow, a: &AppState, models: &UiModels) {
    let shown = shown(a);
    if shown.is_none() && STATE.with(|s| s.borrow_mut().gesture.take().is_some()) {
        ui.set_joint_editing(false);
    }
    let Some((_, j)) = shown else {
        ui.set_joint_kind(SharedString::new());
        crate::ui_models::sync(&models.joint_fields, Vec::new());
        return;
    };
    ui.set_joint_kind(label_of(j.kind).into());
    ui.set_joint_flip(j.flip);
    crate::ui_models::sync(&models.joint_fields, fields(j, a.units()));
}

fn fields(j: &Joint, units: LengthUnit) -> Vec<EditField> {
    let field = |id: String, label: String, value: String, unit: &str| EditField {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        unit: unit.into(),
        enabled: true,
        invalid: false,
    };
    let len = |v: f64| format_length(v, units);
    let mut out = vec![
        field("joint.offset.x".into(), "Offset X".into(), len(j.offset.x), units.suffix()),
        field("joint.offset.y".into(), "Offset Y".into(), len(j.offset.y), units.suffix()),
        field("joint.offset.z".into(), "Offset Z".into(), len(j.offset.z), units.suffix()),
        field("joint.angle".into(), "Angle".into(), format_angle_deg(j.angle), "°"),
    ];
    for (i, &d) in dofs(j.kind).iter().enumerate() {
        let (fmt, unit): (&dyn Fn(f64) -> String, &str) =
            if is_rot(d) { (&format_angle_deg, "°") } else { (&len, units.suffix()) };
        let name = dof_label(d);
        let limit = j.limits[i];
        out.push(field(format!("joint.{i}.value"), name.into(), fmt(j.values[i]), unit));
        out.push(field(format!("joint.{i}.min"), format!("{name} min"), limit.map_or(String::new(), |l| fmt(l.min)), unit));
        out.push(field(format!("joint.{i}.max"), format!("{name} max"), limit.map_or(String::new(), |l| fmt(l.max)), unit));
        out.push(field(format!("joint.{i}.rest"), format!("{name} rest"), limit.map_or(String::new(), |l| fmt(l.rest)), unit));
    }
    out
}

/// `joint.offset.*|angle|<i>.value|min|max|rest` committed. An empty min or max removes the limit.
pub fn field_committed(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str, text: &str) -> bool {
    let units = app.borrow().units();
    let text = text.trim();
    let dof = |j: &Joint| -> Option<(usize, Dof)> {
        let i = id.split('.').nth(1)?.parse::<usize>().ok()?;
        dofs(j.kind).get(i).map(|&d| (i, d))
    };
    apply(ui, app, models, |_, _, j| {
        let rot = id == "joint.angle" || dof(j).is_some_and(|(_, d)| is_rot(d));
        if text.is_empty() && (id.ends_with(".min") || id.ends_with(".max")) {
            let (i, _) = dof(j).ok_or("Unknown field")?;
            j.limits[i] = None;
            return Ok(());
        }
        let v = if rot { parse_angle_deg(text) } else { parse_length(text, units) }.map_err(|_| "Not a number")?;
        match id {
            "joint.offset.x" => j.offset.x = v,
            "joint.offset.y" => j.offset.y = v,
            "joint.offset.z" => j.offset.z = v,
            "joint.angle" => j.angle = v,
            _ => {
                let (i, _) = dof(j).ok_or("Unknown field")?;
                let current = j.values[i];
                let mut l = j.limits[i].unwrap_or(Limit { min: current.min(v), max: current.max(v), rest: current });
                match id.rsplit('.').next() {
                    Some("value") => j.values[i] = v,
                    Some("min") => l.min = v,
                    Some("max") => l.max = v,
                    Some("rest") => l.rest = v,
                    _ => return Err("Unknown field"),
                }
                if !id.ends_with(".value") {
                    j.limits[i] = Some(l);
                }
            }
        }
        Ok(())
    })
}

/// `joint.kind|flip|delete|ok|cancel` clicked.
pub fn action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str) {
    match id {
        "joint.ok" => finish(ui, app, models, false),
        "joint.cancel" => finish(ui, app, models, true),
        "joint.kind" => {
            apply(ui, app, models, |doc, node, j| {
                let at = KINDS.iter().position(|k| *k == j.kind).map_or(0, |p| (p + 1) % KINDS.len());
                j.kind = KINDS[at];
                j.limits = [None; 3];
                // Keep the child where it stands under the new kind's DOFs.
                fit_start(doc, node.id, node.parent, j);
                Ok(())
            });
        }
        "joint.flip" => {
            apply(ui, app, models, |_, _, j| {
                j.flip = !j.flip;
                Ok(())
            });
        }
        "joint.delete" => {
            let command = {
                let a = app.borrow();
                shown(&a).map(|(node, _)| {
                    let command = Command::SetAttach {
                        id: node.id,
                        parent: node.parent,
                        attach: Attach::Free,
                        position: node.position,
                        rotation: node.rotation,
                    };
                    (command, format!("Delete joint of {}", node.name))
                })
            };
            if let Some((command, label)) = command {
                controller::execute(ui, app, models, id, command, label, None, Vec::new());
            }
        }
        _ => {}
    }
}

/// Applies `change` to the shown joint: inside the placement gesture, or as one edit.
fn apply(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    change: impl FnOnce(&Document, &Node, &mut Joint) -> Result<(), &'static str>,
) -> bool {
    let prepared = {
        let a = app.borrow();
        let Some((node, old)) = shown(&a) else { return false };
        let mut j = old.clone();
        match change(&a.session.doc, node, &mut j) {
            Err(message) => Err(message),
            Ok(()) if j == *old => return true,
            Ok(()) => Ok((lab_mates::joints::joint(&a.session.doc, node.id, node.parent, j), node.name.clone())),
        }
    };
    let (command, name) = match prepared {
        Err(message) => {
            show_toast(ui, &app.borrow(), message, false);
            return false;
        }
        Ok((None, _)) => return false,
        Ok((Some(command), name)) => (command, name),
    };
    if STATE.with(|s| s.borrow().gesture.is_some()) {
        let label = format!("Joint {}", label_of(kind_of(&command)));
        live(ui, app, models, command, label)
    } else {
        controller::execute(ui, app, models, "joint", command, format!("Edit {name}"), None, Vec::new())
    }
}

/// `tool.ground`: toggles `locked` on the selected node.
pub fn ground(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let command = {
        let a = app.borrow();
        let Some(SelectionTarget::Node(id)) = a.session.selection else { return };
        let Some(node) = a.session.doc.node(id) else { return };
        let label = if node.locked { "Unground" } else { "Ground" };
        (Command::SetLocked { id, locked: !node.locked }, format!("{label} {}", node.name))
    };
    controller::execute(ui, app, models, "tool.ground", command.0, command.1, None, Vec::new());
}

/// `tool.rigid_group`: rigid joints from every other selected node to the first.
pub fn rigid(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    // shortcut: the app selects one node at a time, so this list has at most one entry until
    // multi-selection exists.
    let ids: Vec<NodeId> = match app.borrow().session.selection {
        Some(SelectionTarget::Node(id)) => vec![id],
        _ => Vec::new(),
    };
    if ids.len() < 2 {
        show_toast(ui, &app.borrow(), "Select two or more parts", false);
        return;
    }
    let command = rigid_group(&app.borrow().session.doc, &ids);
    if let Some(command) = command {
        controller::execute(ui, app, models, "tool.rigid_group", command, "Rigid Group".into(), None, Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_geom::TriMesh;

    /// A child already seated on the joint keeps its pose: the start values absorb the twist.
    #[test]
    fn start_values_keep_a_seated_child_in_place() {
        let doc = lab_core::sample::demo_document();
        let node = doc.nodes.iter().find(|n| matches!(n.parent, Parent::Platform(_))).expect("demo node on a platform");
        let child = Frame { origin: DVec3::new(1.0, 2.0, -3.0), z: DVec3::Z, x: DVec3::X };
        let z = -(node.rotation * DVec3::Z);
        let parent = Frame { origin: node.position + node.rotation * child.origin, z, x: z.any_orthonormal_vector() };
        for kind in [JointKind::Revolute, JointKind::Rigid, JointKind::Cylindrical] {
            let mut j = Joint {
                kind,
                child,
                parent,
                offset: DVec3::ZERO,
                angle: 0.0,
                flip: true,
                values: [0.0; 3],
                limits: [None; 3],
                suppressed: false,
                seq: 0,
            };
            fit_start(&doc, node.id, node.parent, &mut j);
            let (p, q) = lab_mates::joints::joint_pose(&j);
            assert!((p - node.position).length() < 1e-6, "{kind:?}: {p} vs {}", node.position);
            assert!(q.angle_between(node.rotation) < 1e-6, "{kind:?}");
        }
    }

    #[test]
    fn post_holder_top_snaps_to_its_bore() {
        let mut mesh = TriMesh::default();
        let params = lab_core::part::Params::default();
        for (_, m) in &lab_parts::generate("post_holder", &params).unwrap().pieces {
            mesh.append(m);
        }
        let mesh = SnapMesh::new(&mesh);
        let side = mesh.snap_at(DVec3::new(15.0, 0.0, 25.0), DVec3::X, SNAP_MM);
        let top = mesh.snap_at(DVec3::new(8.0, 0.0, 50.0), DVec3::Z, SNAP_MM);
        assert!(top.is_some_and(|s| s.kind == SnapKind::EdgeCentre), "top {top:?} side {side:?}");
    }

    #[test]
    fn twist_wraps_into_half_turn() {
        assert!((wrap(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-12);
        assert!((wrap(-PI / 4.0) + PI / 4.0).abs() < 1e-12);
    }
}
