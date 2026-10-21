//! The library sidebar: catalog rows grouped by the first category segment, search, and
//! dragging a part into the viewport (M2-PLAN POLA-42).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use lab_catalog::Catalog;
use lab_ui::{AppWindow, LibraryRow};
use lab_viewport::{DragItem, Modifiers, PartDrag, Vec2, ViewportRequest};
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::ui_models::{UiModels, sync};

type Group = (&'static str, &'static str, &'static str, (u8, u8, u8));

/// Group order and looks: (first category segment, title, letter, rgb).
const GROUPS: &[Group] = &[
    ("posts", "Posts", "P", (191, 90, 242)),
    ("mounts", "Mounts", "M", (10, 132, 255)),
    ("optics", "Optics", "O", (48, 209, 88)),
    ("beam", "Beam", "B", (255, 214, 10)),
    ("fiber", "Fiber", "F", (100, 210, 255)),
    ("modulators", "Modulators", "A", (94, 92, 230)),
    ("detectors", "Detectors", "D", (255, 159, 10)),
    ("crystals", "Crystals", "C", (172, 142, 104)),
    ("lasers", "Lasers", "L", (255, 55, 95)),
    ("room", "Room", "R", (142, 142, 147)),
];

#[derive(Default)]
pub struct LibraryState {
    pub query: String,
    /// Collapsed groups (first category segment).
    pub collapsed: BTreeSet<String>,
    /// The current drag is over the viewport (a Move was sent and not yet ended).
    pub drag_inside: bool,
}

fn group_of(category: &str) -> &str {
    category.split('/').next().unwrap_or("")
}

/// Header and part rows. While searching every matching group is shown expanded.
pub fn rows(catalog: &Catalog, query: &str, collapsed: &BTreeSet<String>) -> Vec<LibraryRow> {
    let parts = if query.trim().is_empty() { catalog.all() } else { catalog.search(query, 500) };
    let mut out = Vec::new();
    // Known groups in order, then any other first segment alphabetically.
    let mut keys: Vec<String> = GROUPS.iter().map(|g| g.0.to_string()).collect();
    let others: BTreeSet<String> =
        parts.iter().map(|p| group_of(&p.category).to_string()).filter(|k| !keys.contains(k)).collect();
    keys.extend(others);
    for key in keys {
        let members: Vec<_> = parts.iter().filter(|p| group_of(&p.category) == key).collect();
        if members.is_empty() {
            continue;
        }
        let (title, letter, (r, g, b)) = GROUPS
            .iter()
            .find(|g| g.0 == key)
            .map(|g| (g.1.to_string(), g.2, g.3))
            .unwrap_or_else(|| (key.clone(), "?", (142, 142, 147)));
        let tint = slint::Color::from_rgb_u8(r, g, b);
        let expanded = !query.trim().is_empty() || !collapsed.contains(&key);
        out.push(LibraryRow {
            id: key.as_str().into(),
            label: title.into(),
            detail: members.len().to_string().into(),
            letter: letter.into(),
            tint,
            header: true,
            expanded,
            thumb: Default::default(),
            has_thumb: false,
        });
        if expanded {
            out.extend(members.iter().map(|p| {
                let thumb = crate::thumbs::image(&p.id);
                LibraryRow {
                id: p.id.as_str().into(),
                label: p.name.as_str().into(),
                detail: p.part_number.as_str().into(),
                letter: "".into(),
                tint: slint::Color::from_argb_u8(0, 0, 0, 0),
                header: false,
                expanded: false,
                has_thumb: thumb.is_some(),
                thumb: thumb.unwrap_or_default(),
            }}));
        }
    }
    out
}

pub fn fill(app: &AppState, models: &UiModels) {
    sync(&models.library, rows(&app.catalog, &app.library.query, &app.library.collapsed));
}

/// Reloads the catalog from disk, sends it to the viewport and refreshes the sidebar
/// (M4-PLAN: after an import and after the connector wizard saves).
pub fn reload_catalog(app: &mut AppState, models: &UiModels) {
    let (catalog, warnings) = Catalog::load_default();
    for w in &warnings {
        eprintln!("library: {}: {}", w.path.display(), w.message);
    }
    app.catalog = std::sync::Arc::new(catalog);
    app.send(ViewportRequest::SetCatalog(app.catalog.clone()));
    fill(app, models);
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (a, m) = (app.clone(), models.clone());
    ui.on_library_search(move |text| {
        let mut app = a.borrow_mut();
        app.library.query = text.to_string();
        fill(&app, &m);
    });

    let (a, m) = (app.clone(), models.clone());
    ui.on_library_toggle(move |group| {
        let mut app = a.borrow_mut();
        let group = group.to_string();
        if !app.library.collapsed.remove(&group) {
            app.library.collapsed.insert(group);
        }
        fill(&app, &m);
    });

    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_library_drag(move |id, kind, x, y| {
        if let Some(ui) = ui_weak.upgrade() {
            drag(&ui, &a, DragItem::Part(id.to_string()), kind, x, y);
        }
    });
}

/// One step of a sidebar drag (kind 0 move, 1 drop, 2 cancel; window coordinates): sends
/// `DragPart` only while the pointer is over the viewport.
pub fn drag(ui: &AppWindow, app: &Rc<RefCell<AppState>>, item: DragItem, kind: i32, x: f32, y: f32) {
    let scale = ui.window().scale_factor();
    // The viewport fills the window; the sidebar floats over its left part.
    let inside =
        x > ui.get_sidebar_right() && x < ui.get_viewport_width() && y >= 0.0 && y < ui.get_viewport_height();
    let pos = Vec2::new(x * scale, y * scale);
    // shortcut: Slint gives no modifiers to a TouchArea move, so drags carry none; Shift-free placement needs them later.
    let mods = Modifiers::default();
    let mut app = app.borrow_mut();
    let was_inside = app.library.drag_inside;
    let drag = match (kind, inside) {
        (0, true) => Some(PartDrag::Move { item, pos, mods }),
        (1, true) if was_inside => Some(PartDrag::Drop { pos, mods }),
        (0 | 1, false) | (1, true) if was_inside => Some(PartDrag::Cancel),
        (2, _) if was_inside => Some(PartDrag::Cancel),
        _ => None,
    };
    app.library.drag_inside = kind == 0 && inside;
    if let Some(drag) = drag {
        app.send(ViewportRequest::DragPart(drag));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::part::PartDef;

    fn def(id: &str, name: &str, category: &str) -> PartDef {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "category": category, "part_number": name,
            "geometry": { "kind": "parametric", "generator": "post" },
        }))
        .unwrap()
    }

    #[test]
    fn groups_and_search() {
        let catalog = Catalog::from_defs(vec![
            def("thorlabs:KM100", "KM100", "mounts/kinematic"),
            def("thorlabs:TR50/M", "TR50/M", "posts/posts"),
        ]);
        let all = rows(&catalog, "", &BTreeSet::new());
        let labels: Vec<_> = all.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["Posts", "TR50/M", "Mounts", "KM100"]);
        let found = rows(&catalog, "km", &BTreeSet::from(["mounts".to_string()]));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].id, "thorlabs:KM100");
        let collapsed = rows(&catalog, "", &BTreeSet::from(["posts".to_string()]));
        assert_eq!(collapsed.len(), 3);
    }
}
