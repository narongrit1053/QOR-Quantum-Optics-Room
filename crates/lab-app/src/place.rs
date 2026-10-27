//! Drops from the library: a viewport `PlaceRequest` becomes one `AddNode` command
//! (M2-PLAN POLA-43).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::part::{Frame, Geometry, ParamValue};
use lab_catalog::Catalog;
use lab_core::{Attach, Command, Finish, Node, NodeId, Parent, SelectionTarget, Shape};
use lab_ui::AppWindow;
use lab_core::glam::{DQuat, DVec3};
use lab_viewport::{DragItem, PlaceRequest, stack_chain};

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

pub fn on_place_requested(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, request: PlaceRequest) {
    let bundle = app.borrow_mut().library.bundle.take();
    if let (Some(name), DragItem::Part(_)) = (bundle, &request.item) {
        return place_bundle(ui, app, models, &name, &request);
    }
    let mut toast = None;
    let (root, label, command) = {
        let a = app.borrow();
        let first = a.session.doc.next_free_id();
        let Some(root) = new_node(&a.catalog, request.item.root(), first, request.parent, request.attach.clone(), request.position, request.rotation)
        else {
            return;
        };
        match &request.item {
            DragItem::Part(part) => match laser_lift(&a, part, &request) {
                Some(lift) if lift > 0.5 => {
                    let Some(riser) = riser_for(&a.catalog, part, lift, &root) else { return };
                    let seat = a.catalog.get(part).and_then(|d| d.connector("seat")).map(|c| c.frame);
                    // The riser is centred under the head's footprint; the head sits back over its origin.
                    let c = riser.position - root.position;
                    let c = root.rotation.inverse() * c;
                    let top = Frame { origin: DVec3::new(-c.x, -c.y, lift), z: DVec3::Z, x: DVec3::X };
                    let (position, rotation) =
                        seat.map(|s| lab_mates::mate_pose(&top, &s, 0.0, 0.0)).unwrap_or((top.origin, DQuat::IDENTITY));
                    let head = Node { id: NodeId(first + 1), parent: Parent::Node(riser.id), attach: Attach::Free, position, rotation, ..root };
                    (head.id, format!("Add {} on a riser", head.name), Command::PlaceAssembly { nodes: vec![riser, head] })
                }
                lift => {
                    if let Some(l) = lift.filter(|l| *l < -0.5) {
                        let h = a.stack.beam_height;
                        toast = Some(format!("Beam at {:.1} mm, above the {h:.1} mm beam height", h - l));
                    }
                    (root.id, format!("Add {}", root.name), Command::AddNode { node: root, index: None })
                }
            },
            DragItem::Stack(spec) => {
                let Some(chain) = stack_chain(&a.catalog, spec) else { return };
                let mut nodes = vec![root];
                for (part, attach, position, rotation) in chain {
                    let below = Parent::Node(nodes.last().unwrap().id);
                    let id = first + nodes.len() as u64;
                    let Some(node) = new_node(&a.catalog, &part, id, below, attach, position, rotation) else { return };
                    nodes.push(node);
                }
                (nodes[0].id, format!("Add stack of {} parts", nodes.len()), Command::PlaceAssembly { nodes })
            }
        }
    };
    controller::execute(ui, app, models, "place", command, label, Some(SelectionTarget::Node(root)), Vec::new());
    if let Some(text) = toast {
        controller::show_toast(ui, &app.borrow(), &text, false);
    }
}

/// POLA-203: a library bundle dropped where its ghost (anchor) part would go: one `PlaceAssembly`.
fn place_bundle(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, name: &str, request: &PlaceRequest) {
    let (command, root, skipped) = {
        let a = app.borrow();
        let Some(bundle) = a.settings.bundles.iter().find(|b| b.name == name) else { return };
        let keep = |n: &Node| !matches!(n.shape, Shape::Part) || a.catalog.get(&n.part).is_some();
        let next = a.session.doc.next_free_id();
        // shortcut: saved rotations are kept, the drop's rotation is ignored.
        let (nodes, skipped) =
            crate::clipboard::place_bundle(&bundle.nodes, next, request.parent, &request.attach, request.position, keep);
        let Some(root) = nodes.first().map(|n| n.id) else { return };
        (Command::PlaceAssembly { nodes }, root, skipped)
    };
    controller::execute(ui, app, models, "place", command, format!("Add bundle {name}"), Some(SelectionTarget::Node(root)), Vec::new());
    if skipped > 0 {
        controller::show_toast(ui, &app.borrow(), &format!("Skipped {skipped} missing parts"), false);
    }
}

/// Beam height minus the exit height of a part with a `source` port dropped on a platform or a node on one
/// (M4-PLAN POLA-62); `None` for anything else.
fn laser_lift(app: &AppState, part: &str, request: &PlaceRequest) -> Option<f64> {
    let doc = &app.session.doc;
    // Drop point's height above the platform top it ends up on (a node may sit on a platform).
    let base = match request.parent {
        Parent::Platform(_) => request.position.z,
        Parent::Node(n) => {
            let (np, nq) = lab_mates::constraints::world_pose(doc, Parent::Node(n))?;
            let (pp, _) = lab_mates::constraints::world_pose(doc, Parent::Platform(doc.root_platform(n)?))?;
            (np + nq * request.position).z - pp.z
        }
        Parent::Room => return None,
    };
    let port = app.catalog.get(part)?.optical_ports.iter().find(|p| p.kind == "source")?;
    // Exit height above the seat (the seat may not be at z = 0).
    let seat = app.catalog.get(part)?.connector("seat").map_or(0.0, |c| c.frame.origin.z);
    Some(app.stack.beam_height - base - (port.frame.origin.z - seat))
}

/// A `generic:riser` of the head's footprint and height `lift`, at the head's drop pose.
fn riser_for(catalog: &Catalog, part: &str, lift: f64, head: &Node) -> Option<Node> {
    let mesh = crate::wizard::part_mesh(catalog.get(part)?).ok()?;
    let (lo, hi) = mesh.positions.iter().fold((DVec3::MAX, DVec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    if lo.x > hi.x {
        return None;
    }
    // shortcut: footprint in the head's own frame, assuming its seat faces down; a tilted seat needs the seat frame.
    let (w, l) = (hi.x - lo.x, hi.y - lo.y);
    let centre = DVec3::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0, 0.0);
    let at = head.position + head.rotation * centre;
    let attach = if centre.truncate().length() > 1e-6 { Attach::Free } else { head.attach.clone() };
    let mut riser = new_node(catalog, "generic:riser", head.id.0, head.parent, attach, at, head.rotation)?;
    for (k, v) in [("length", l), ("width", w), ("height", lift)] {
        riser.params.insert(k.into(), ParamValue::Number(v));
    }
    Some(riser)
}

/// A part node from the catalog; `None` when `part` is not in it.
pub(crate) fn new_node(
    catalog: &Catalog,
    part: &str,
    id: u64,
    parent: Parent,
    attach: Attach,
    position: DVec3,
    rotation: DQuat,
) -> Option<Node> {
    let def = catalog.get(part)?;
    // The part's main finish: its first mesh piece.
    let finish = match &def.geometry {
        Geometry::Parametric { generator, params } => lab_parts::generate(generator, params)
            .ok()
            .and_then(|mesh| mesh.pieces.first().map(|(f, _)| f.clone())),
        Geometry::Mesh { .. } => def.meta.get("finish").map(|f| crate::import::finish_from_meta(f)),
    };
    Some(Node {
        note: String::new(),
        id: NodeId(id),
        name: def.name.clone(),
        part: def.id.clone(),
        parent,
        attach,
        position,
        rotation,
        shape: Shape::Part,
        finish: finish.unwrap_or(Finish::AnodizedBlack),
        params: Default::default(),
        pickable: true,
        locked: false,
    })
}
