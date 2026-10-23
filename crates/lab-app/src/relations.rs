//! M9 G. The relationships bar: one chip per joint (by `seq`), then one per M8 constraint
//! (node order). Hover outlines both bodies, click selects the child, right-click opens a menu.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lab_core::{Attach, Command, Document, NodeId, Parent, SelectionTarget};
use lab_ui::{AppWindow, PaletteItem, RelationChip};
use lab_viewport::ViewportRequest;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app_state::AppState;
use crate::ui_models::UiModels;

pub struct Chip {
    pub node: NodeId,
    pub parent: Parent,
    pub label: String,
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

pub fn chips(doc: &Document) -> Vec<Chip> {
    let mut joints: Vec<_> = doc
        .nodes
        .iter()
        .filter_map(|n| match &n.attach {
            Attach::Joint(j) => Some((j.seq, n, j)),
            _ => None,
        })
        .collect();
    joints.sort_by_key(|(seq, ..)| *seq);
    let mut out: Vec<Chip> = joints
        .into_iter()
        .map(|(_, n, j)| Chip {
            node: n.id,
            parent: n.parent,
            label: format!("{} → {} · {:?}", n.name, parent_name(doc, n.parent), j.kind),
            icon: "joint".into(),
            suppressed: j.suppressed,
            joint: true,
        })
        .collect();
    for n in &doc.nodes {
        if let Attach::Constrained { constraints } = &n.attach {
            out.extend(constraints.iter().map(|c| Chip {
                node: n.id,
                parent: n.parent,
                label: format!("{} → {} · {:?}", n.name, parent_name(doc, n.parent), c.kind),
                icon: "constraint".into(),
                suppressed: false,
                joint: false,
            }));
        }
    }
    out
}

/// Updates the bar's model in place (keeps its scroll position); call on every document change.
pub fn fill(ui: &AppWindow, doc: &Document) {
    let rows: Vec<RelationChip> = chips(doc)
        .into_iter()
        .map(|c| RelationChip { label: c.label.into(), icon: c.icon.into(), suppressed: c.suppressed })
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
    usize::try_from(i).ok().and_then(|i| chips(&app.borrow().session.doc).into_iter().nth(i))
}

fn select(ui: &AppWindow, target: SelectionTarget) {
    let SelectionTarget::Node(id) = target else { return };
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
        select(&ui, SelectionTarget::Node(c.node));
    });

    // shortcut: selects only; scrolling the inspector to its Joint section waits for E's inspector section.
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_relation_double_clicked(move |i| {
        let (Some(ui), Some(c)) = (w.upgrade(), chip(&a, i)) else { return };
        select(&ui, SelectionTarget::Node(c.node));
    });

    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_relation_menu(move |i, x, y| {
        let (Some(ui), Some(c)) = (w.upgrade(), chip(&a, i)) else { return };
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
    let Some(c) = MENU.take().and_then(|i| chips(&app.borrow().session.doc).into_iter().nth(i)) else { return };
    if id == "relation.select_bodies" {
        // shortcut: single selection only (no multi-select yet), so the parent node is not added.
        select(ui, SelectionTarget::Node(c.node));
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
    fn joints_by_seq_then_constraints() {
        let mut doc = lab_core::sample::demo_document();
        assert!(doc.nodes.len() >= 3);
        let plane = Feature::Plane { origin: DVec3::ZERO, normal: DVec3::Z };
        doc.nodes[0].attach = Attach::Constrained {
            constraints: vec![Constraint {
                kind: ConstraintKind::Mate,
                child: plane.clone(),
                parent: plane,
                offset: 0.0,
                angle: 0.0,
                flip: false,
                lock_rotation: false,
            }],
        };
        doc.nodes[1].attach = joint(5);
        doc.nodes[2].attach = joint(2);
        let ids = [doc.nodes[2].id, doc.nodes[1].id, doc.nodes[0].id];
        let got = chips(&doc);
        assert_eq!(got.iter().map(|c| c.node).collect::<Vec<_>>(), ids);
        assert!(got[0].label.ends_with("· Revolute"), "{}", got[0].label);
        assert!(got[2].label.ends_with("· Mate") && !got[2].joint);
    }
}
