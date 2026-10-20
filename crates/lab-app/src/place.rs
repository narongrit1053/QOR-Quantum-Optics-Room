//! Drops from the library: a viewport `PlaceRequest` becomes one `AddNode` command
//! (M2-PLAN POLA-43).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::part::Geometry;
use lab_core::{Command, Finish, Node, NodeId, SelectionTarget, Shape};
use lab_ui::AppWindow;
use lab_viewport::{DragItem, PlaceRequest};

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

pub fn on_place_requested(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, request: PlaceRequest) {
    let DragItem::Part(part) = &request.item;
    let node = {
        let a = app.borrow();
        let Some(def) = a.catalog.get(part) else { return };
        // The part's main finish: its first mesh piece.
        let finish = match &def.geometry {
            Geometry::Parametric { generator, params } => lab_parts::generate(generator, params)
                .ok()
                .and_then(|mesh| mesh.pieces.first().map(|(f, _)| f.clone())),
            Geometry::Mesh { .. } => None,
        };
        Node {
            id: NodeId(a.session.doc.next_free_id()),
            name: def.name.clone(),
            part: def.id.clone(),
            parent: request.parent,
            attach: request.attach,
            position: request.position,
            rotation: request.rotation,
            shape: Shape::Part,
            finish: finish.unwrap_or(Finish::AnodizedBlack),
            params: Default::default(),
            pickable: true,
            locked: false,
        }
    };
    let (id, label) = (node.id, format!("Add {}", node.name));
    controller::execute(
        ui,
        app,
        models,
        "place",
        Command::AddNode { node, index: None },
        label,
        Some(SelectionTarget::Node(id)),
        Vec::new(),
    );
}
