//! Drops from the library: a viewport `PlaceRequest` becomes one `AddNode` command
//! (M2-PLAN POLA-43).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::part::Geometry;
use lab_catalog::Catalog;
use lab_core::{Attach, Command, Finish, Node, NodeId, Parent, SelectionTarget, Shape};
use lab_ui::AppWindow;
use lab_core::glam::{DQuat, DVec3};
use lab_viewport::{DragItem, PlaceRequest, stack_chain};

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

pub fn on_place_requested(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, request: PlaceRequest) {
    let (root, label, command) = {
        let a = app.borrow();
        let first = a.session.doc.next_free_id();
        let Some(root) = new_node(&a.catalog, request.item.root(), first, request.parent, request.attach, request.position, request.rotation)
        else {
            return;
        };
        match &request.item {
            DragItem::Part(_) => (root.id, format!("Add {}", root.name), Command::AddNode { node: root, index: None }),
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
}

/// A part node from the catalog; `None` when `part` is not in it.
fn new_node(
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
