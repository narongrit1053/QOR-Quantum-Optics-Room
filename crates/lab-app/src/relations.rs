//! M9 G. The relationships tree in the node inspector: only the selected part's chain (the
//! parts linked to it by joints or M8 constraints), root first, one row per part. Hover outlines
//! both bodies, click selects, right-click opens a menu.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lab_core::{Attach, Command, Document, Node, NodeId, Parent, SelectionTarget};
use lab_ui::{AppWindow, PaletteItem, RelationChip};
use lab_viewport::ViewportRequest;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app_state::AppState;
use crate::ui_models::UiModels;

pub struct Chip {
    pub node: NodeId,
    pub parent: Parent,
    pub label: String,
    /// The relation kinds to the parent ("Concentric, Mate"); empty on the chain root.
    pub detail: String,
    pub depth: i32,
    /// "joint", "constraint", or "" for the chain root (no menu).
    pub icon: String,
    pub suppressed: bool,
    /// Joint chips can be suppressed; M8 constraint chips cannot.
    pub joint: bool,
}

fn parent_name(doc: &Document, parent: Parent) -> String {
    match parent {
        Parent::Room => "Room".into(),
        Parent::Platform(p) => doc.platform(p).map_or_else(|| "Platform".into(), |p| p.name.clone()),
        Parent::Node(n) => doc.node(n).map_or_else(|| "Part".into(), |n| n.name.clone()),
    }
}

fn related(n: &Node) -> bool {
    matches!(n.attach, Attach::Joint(_) | Attach::Constrained { .. })
}

/// The tree rows of the chain `selected` belongs to: walk up through related parents to the
/// root, then depth-first down through related children. Empty when nothing is related.
pub fn chips(doc: &Document, selected: Option<SelectionTarget>) -> Vec<Chip> {
    let Some(SelectionTarget::Node(mut root)) = selected else { return Vec::new() };
    while let Some(n) = doc.node(root).filter(|n| related(n)) {
        let Parent::Node(p) = n.parent else { break };
        root = p;
    }
    let mut out = Vec::new();
    let mut stack = vec![(root, 0)];
    while let Some((id, depth)) = stack.pop() {
        let Some(n) = doc.node(id) else { continue };
        let (detail, icon, suppressed, joint) = match &n.attach {
            Attach::Joint(j) => (format!("{:?}", j.kind), "joint", j.suppressed, true),
            Attach::Constrained { constraints } => {
                (constraints.iter().map(|c| format!("{:?}", c.kind)).collect::<Vec<_>>().join(", "), "constraint", false, false)
            }
            _ => (String::new(), "", false, false),
        };
        // A related root sits on a platform or the room: name it, the relation is to that.
        let detail = match (depth, n.parent) {
            (0, p) if !detail.is_empty() => format!("{detail} → {}", parent_name(doc, p)),
            _ => detail,
        };
        out.push(Chip { node: n.id, parent: n.parent, label: n.name.clone(), detail, depth, icon: icon.into(), suppressed, joint });
        let children: Vec<NodeId> = doc.children(Parent::Node(id)).filter(|c| related(c)).map(|c| c.id).collect();
        stack.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
    }
    if out.len() == 1 && out[0].icon.is_empty() {
        out.clear(); // a lone part with no relations
    }
    out
}

/// Updates the bar's model in place (keeps its scroll position); call on every document change.
pub fn fill(ui: &AppWindow, doc: &Document, selected: Option<SelectionTarget>) {
    let rows: Vec<RelationChip> = chips(doc, selected)
        .into_iter()
        .map(|c| RelationChip {
            current: selected == Some(SelectionTarget::Node(c.node)),
            label: c.label.into(),
            detail: c.detail.into(),
            depth: c.depth,
            icon: c.icon.into(),
            suppressed: c.suppressed,
        })
        .collect();
    let model = ui.get_relation_chips();
    match model.as_any().downcast_ref::<VecModel<RelationChip>>() {
        Some(vec) if vec.iter().ne(rows.iter().cloned()) => vec.set_vec(rows),
        Some(_) => {}
        None => ui.set_relation_chips(ModelRc::new(VecModel::from(rows))),
    }
}

thread_local! {
    /// The chip whose right-click menu is open.
    static MENU: Cell<Option<usize>> = const { Cell::new(None) };
}

fn chip(app: &Rc<RefCell<AppState>>, i: i32) -> Option<Chip> {
    let a = app.borrow();
    usize::try_from(i).ok().and_then(|i| chips(&a.session.doc, a.session.selection).into_iter().nth(i))
}

/// Selects the node; a no-op when it is already selected (a row click there does not toggle it off).
fn select(ui: &AppWindow, app: &Rc<RefCell<AppState>>, target: SelectionTarget) {
    let SelectionTarget::Node(id) = target else { return };
    if app.borrow().session.selection == Some(target) {
        return;
    }
    ui.invoke_scene_row_clicked(0, id.0 as i32);
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let a = app.clone();
    ui.on_relation_hovered(move |i| {
        let ids = chip(&a, i).map_or_else(Vec::new, |c| {
            let mut ids = vec![c.node];
            if let Parent::Node(p) = c.parent {
                ids.push(p);
            }
            ids
        });
        a.borrow().send(ViewportRequest::HighlightNodes(ids));
    });

    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_relation_clicked(move |i| {
        let (Some(ui), Some(c)) = (w.upgrade(), chip(&a, i)) else { return };
        select(&ui, &a, SelectionTarget::Node(c.node));
    });

    // shortcut: selects only; scrolling the inspector to its Joint section waits for E's inspector section.
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_relation_double_clicked(move |i| {
        let (Some(ui), Some(c)) = (w.upgrade(), chip(&a, i)) else { return };
        select(&ui, &a, SelectionTarget::Node(c.node));
    });

    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_relation_menu(move |i, x, y| {
        let (Some(ui), Some(c)) = (w.upgrade(), chip(&a, i)) else { return };
        if c.icon.is_empty() {
            return; // the chain root has no relation to act on
        }
        MENU.set(usize::try_from(i).ok());
        let mut rows = Vec::new();
        if c.joint {
            rows.push(("relation.suppress", if c.suppressed { "Unsuppress" } else { "Suppress" }));
        }
        rows.extend([("relation.delete", "Delete"), ("relation.select_bodies", "Select Bodies")]);
        let items: Vec<PaletteItem> = rows
            .into_iter()
            .map(|(id, title)| PaletteItem { id: id.into(), title: title.into(), group: "".into(), shortcut: "".into() })
            .collect();
        ui.set_context_rows(ModelRc::new(VecModel::from(items)));
        ui.set_context_x(x);
        ui.set_context_y(y);
        ui.invoke_open_context_menu();
    });
}

/// The `relation.*` menu actions, for the chip whose menu was opened last.
pub fn run(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str) {
    let Some(c) = MENU.take().and_then(|i| chip(app, i as i32)) else { return };
    if id == "relation.select_bodies" {
        // shortcut: single selection only (no multi-select yet), so the parent node is not added.
        select(ui, app, SelectionTarget::Node(c.node));
        return;
    }
    let (command, label) = {
        let a = app.borrow();
        let Some(node) = a.session.doc.node(c.node) else { return };
        let (attach, label) = match (&node.attach, id) {
            (Attach::Joint(j), "relation.suppress") => {
                let mut j = j.clone();
                j.suppressed = !j.suppressed;
                let label = if j.suppressed { "Suppress Joint" } else { "Unsuppress Joint" };
                (Attach::Joint(j), label)
            }
            (_, "relation.delete") => (Attach::Free, if c.joint { "Delete Joint" } else { "Delete Constraints" }),
            _ => return,
        };
        // Free under the same parent at the current relative pose: the body stays where it is.
        let command =
            Command::SetAttach { id: c.node, parent: node.parent, attach, position: node.position, rotation: node.rotation };
        (command, label.to_string())
    };
    crate::controller::execute(ui, app, models, id, command, label, None, Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::part::{Feature, Frame};
    use lab_core::{Constraint, ConstraintKind, Joint, JointKind};
    use lab_core::glam::DVec3;

    fn joint(seq: u64) -> Attach {
        let f = Frame { origin: DVec3::ZERO, z: DVec3::Z, x: DVec3::X };
        Attach::Joint(Joint {
            kind: JointKind::Revolute,
            child: f,
            parent: f,
            offset: DVec3::ZERO,
            angle: 0.0,
            flip: false,
            values: [0.0; 3],
            limits: [None; 3],
            suppressed: false,
            seq,
        })
    }

    #[test]
    fn chain_of_the_selection_only() {
        let mut doc = lab_core::sample::demo_document();
        assert!(doc.nodes.len() >= 4);
        let plane = Feature::Plane { origin: DVec3::ZERO, normal: DVec3::Z };
        let mate = Constraint {
            kind: ConstraintKind::Mate,
            child: plane.clone(),
            parent: plane,
            offset: 0.0,
            angle: 0.0,
            flip: false,
            lock_rotation: false,
        };
        let ids: Vec<NodeId> = doc.nodes.iter().map(|n| n.id).collect();
        // ids[0] ← ids[1] (constraint) ← ids[2] (joint); ids[3] stays free.
        doc.nodes[1].parent = Parent::Node(ids[0]);
        doc.nodes[1].attach = Attach::Constrained { constraints: vec![mate.clone(), Constraint { kind: ConstraintKind::Concentric, ..mate }] };
        doc.nodes[2].parent = Parent::Node(ids[1]);
        doc.nodes[2].attach = joint(5);
        doc.nodes[3].attach = Attach::Free;
        let got = chips(&doc, Some(SelectionTarget::Node(ids[2])));
        assert_eq!(got.iter().map(|c| (c.node, c.depth)).collect::<Vec<_>>(), [(ids[0], 0), (ids[1], 1), (ids[2], 2)]);
        assert_eq!(got[1].detail, "Mate, Concentric");
        assert!(got[0].icon.is_empty() && got[2].joint);
        assert!(chips(&doc, Some(SelectionTarget::Node(ids[3]))).is_empty());
        assert!(chips(&doc, None).is_empty());
    }
}
