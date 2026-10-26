//! The library sidebar: catalog rows grouped by the first category segment, search, and
//! dragging a part into the viewport (M2-PLAN POLA-42). POLA-162: separate libraries, type,
//! units and vendor filters, and hidden parts.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use lab_catalog::{Catalog, Source};
use lab_core::part::{ConnectorKind, PartDef};
use lab_file::UserLibrary;
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
    ("stages", "Stages", "S", (90, 200, 250)),
    ("fiber", "Fiber", "F", (100, 210, 255)),
    ("modulators", "Modulators", "A", (94, 92, 230)),
    ("detectors", "Detectors", "D", (255, 159, 10)),
    ("crystals", "Crystals", "C", (172, 142, 104)),
    ("lasers", "Lasers", "L", (255, 55, 95)),
    ("hardware", "Hardware", "H", (160, 160, 165)),
    ("room", "Room", "R", (142, 142, 147)),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnitFilter {
    #[default]
    All,
    Metric,
    Inch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Metric,
    Inch,
}

#[derive(Default)]
pub struct LibraryState {
    pub query: String,
    /// Collapsed groups (first category segment).
    pub collapsed: BTreeSet<String>,
    /// The current drag is over the viewport (a Move was sent and not yet ended).
    pub drag_inside: bool,
    /// POLA-162 filters, not saved: type group, units, vendor ("" = Generic), hidden parts listed.
    pub group: Option<String>,
    pub units: UnitFilter,
    pub vendor: Option<String>,
    pub show_hidden: bool,
    /// "New library…" from the part menu: the part the new library starts with.
    pub pending_part: Option<String>,
    /// POLA-165: grid columns the library menu reported for its width.
    pub columns: usize,
}

pub struct LibraryFilter<'a> {
    pub query: &'a str,
    /// None = Default (the whole catalog).
    pub library: Option<&'a [String]>,
    /// First category segment, None = all types.
    pub group: Option<&'a str>,
    pub units: UnitFilter,
    /// None = all vendors; Some("") = Generic.
    pub vendor: Option<&'a str>,
    pub show_builtin: bool,
    pub show_hidden: bool,
    pub hidden: &'a BTreeSet<String>,
}

fn group_of(category: &str) -> &str {
    category.split('/').next().unwrap_or("")
}

/// Metric or inch from the tags, the part number, then thread connector specs; first match wins.
pub fn unit_system(def: &PartDef) -> Option<Unit> {
    let tag = |names: &[&str]| def.tags.iter().any(|t| names.iter().any(|n| t.eq_ignore_ascii_case(n)));
    if tag(&["metric"]) {
        return Some(Unit::Metric);
    }
    if tag(&["imperial", "inch"]) {
        return Some(Unit::Inch);
    }
    let p = def.part_number.as_str();
    if ["/M", "-M", "_M"].iter().any(|s| p.ends_with(s)) || ["-M-", "_M-", "-M "].iter().any(|s| p.contains(s)) {
        return Some(Unit::Metric);
    }
    // Thorlabs: no metric suffix means imperial, whatever the (auto-detected) hole threads say.
    if def.vendor.eq_ignore_ascii_case("thorlabs") {
        return Some(Unit::Inch);
    }
    def.connectors.iter().find_map(|c| match &c.kind {
        ConnectorKind::Thread { spec, .. } => {
            let b = spec.as_bytes();
            if b.len() > 1 && b[0] == b'M' && b[1].is_ascii_digit() {
                Some(Unit::Metric)
            } else if spec.contains('-') {
                Some(Unit::Inch)
            } else {
                None
            }
        }
        _ => None,
    })
}

/// Parts left after every filter but the vendor one.
fn candidates<'c>(catalog: &'c Catalog, f: &LibraryFilter) -> Vec<&'c PartDef> {
    let mut parts = if f.query.trim().is_empty() { catalog.all() } else { catalog.search(f.query, 500) };
    match f.library {
        // The user picked these parts on purpose: `show_builtin` does not apply.
        Some(ids) => parts.retain(|p| ids.contains(&p.id)),
        None if !f.show_builtin => parts.retain(|p| catalog.source(&p.id) != Some(Source::BuiltIn)),
        None => {}
    }
    if !f.show_hidden {
        parts.retain(|p| !f.hidden.contains(&p.id));
    }
    if let Some(group) = f.group {
        parts.retain(|p| group_of(&p.category) == group);
    }
    match f.units {
        UnitFilter::All => {}
        UnitFilter::Metric => parts.retain(|p| unit_system(p) == Some(Unit::Metric)),
        UnitFilter::Inch => parts.retain(|p| unit_system(p) == Some(Unit::Inch)),
    }
    parts
}

/// Distinct vendors of the parts visible before the vendor filter ("" = Generic).
pub fn vendors(catalog: &Catalog, f: &LibraryFilter) -> Vec<String> {
    let set: BTreeSet<String> = candidates(catalog, f).iter().map(|p| p.vendor.clone()).collect();
    set.into_iter().collect()
}

/// Header and part rows. While searching every matching group is shown expanded.
pub fn rows(catalog: &Catalog, f: &LibraryFilter, collapsed: &BTreeSet<String>) -> Vec<LibraryRow> {
    let query = f.query;
    let mut parts = candidates(catalog, f);
    if let Some(vendor) = f.vendor {
        parts.retain(|p| p.vendor == vendor);
    }
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
            hidden: false,
            ..Default::default()
        });
        if expanded {
            out.extend(members.iter().map(|p| {
                let thumb = crate::thumbs::image(&p.id);
                LibraryRow {
                id: p.id.as_str().into(),
                label: p.name.as_str().into(),
                detail: p.part_number.as_str().into(),
                letter: letter.into(),
                tint,
                header: false,
                expanded: false,
                has_thumb: thumb.is_some(),
                thumb: thumb.unwrap_or_default(),
                hidden: f.hidden.contains(&p.id),
                info: if p.vendor.is_empty() { p.category.as_str().into() } else { format!("{} · {}", p.vendor, p.category).into() },
                ..Default::default()
            }}));
        }
    }
    out
}

fn active_parts(app: &AppState) -> Option<&[String]> {
    let name = &app.settings.active_library;
    app.settings.libraries.iter().find(|l| &l.name == name).map(|l| l.parts.as_slice())
}

fn filter<'a>(app: &'a AppState, vendor: Option<&'a str>) -> LibraryFilter<'a> {
    let l = &app.library;
    LibraryFilter {
        query: &l.query,
        library: active_parts(app),
        group: l.group.as_deref(),
        units: l.units,
        vendor,
        show_builtin: app.settings.show_builtin_parts,
        show_hidden: l.show_hidden,
        hidden: &app.settings.hidden_parts,
    }
}

/// Trimmed, non-empty and unique (" 2", " 3"… appended on a clash).
fn unique_name(libs: &[UserLibrary], name: &str) -> Option<String> {
    let base = name.trim();
    if base.is_empty() {
        return None;
    }
    let mut name = base.to_string();
    let mut i = 2;
    while libs.iter().any(|l| l.name == name) {
        name = format!("{base} {i}");
        i += 1;
    }
    Some(name)
}

/// Library menu tile size and gap, and the group header height (POLA-165).
pub const TILE_W: f32 = 120.0;
pub const TILE_H: f32 = 140.0;
pub const TILE_GAP: f32 = 8.0;
pub const HEADER_H: f32 = 28.0;

/// Grid positions: a header takes a full row, parts fill `columns` slots per row.
pub fn grid_layout(rows: &[LibraryRow], columns: usize) -> Vec<(f32, f32)> {
    let columns = columns.max(1);
    let (mut y, mut col, mut out) = (0.0, 0, Vec::with_capacity(rows.len()));
    for r in rows {
        if r.header {
            if col > 0 {
                y += TILE_H + TILE_GAP;
                col = 0;
            }
            out.push((0.0, y));
            y += HEADER_H + TILE_GAP;
        } else {
            if col == columns {
                y += TILE_H + TILE_GAP;
                col = 0;
            }
            out.push((col as f32 * (TILE_W + TILE_GAP), y));
            col += 1;
        }
    }
    out
}

pub fn fill(app: &AppState, models: &UiModels) {
    let f = filter(app, app.library.vendor.as_deref());
    let mut rows = rows(&app.catalog, &f, &app.library.collapsed);
    if let Some(ui) = models.ui.upgrade() {
        let total = candidates(&app.catalog, &LibraryFilter { query: "", group: None, units: UnitFilter::All, ..f }).len();
        let shown: usize = rows.iter().filter(|r| r.header).filter_map(|r| r.detail.parse::<usize>().ok()).sum();
        ui.set_library_count(if shown == total { format!("{total} parts") } else { format!("{shown} of {total} parts") }.into());
    }
    let pos = grid_layout(&rows, app.library.columns);
    for (r, (x, y)) in rows.iter_mut().zip(pos) {
        (r.x, r.y) = (x, y);
    }
    sync(&models.library, rows);
}

fn strings(items: Vec<slint::SharedString>) -> slint::ModelRc<slint::SharedString> {
    Rc::new(slint::VecModel::from(items)).into()
}

/// The picker and filter labels.
pub fn sync_ui(ui: &AppWindow, app: &AppState) {
    let (s, l) = (&app.settings, &app.library);
    ui.set_library_names(strings(s.libraries.iter().map(|l| l.name.as_str().into()).collect()));
    ui.set_library_active(s.active_library.as_str().into());
    let group = l.group.as_deref().map_or("All".to_string(), |g| {
        GROUPS.iter().find(|x| x.0 == g).map_or(g.to_string(), |x| x.1.to_string())
    });
    ui.set_library_type(group.into());
    let vendor = l.vendor.as_deref().map_or("All", |v| if v.is_empty() { "Generic" } else { v });
    ui.set_library_vendor(vendor.into());
    ui.set_library_units(l.units as i32);
    ui.set_library_show_hidden(l.show_hidden);
    ui.set_library_filtered(l.group.is_some() || l.vendor.is_some() || l.units != UnitFilter::All);
    ui.set_library_list_view(s.library_view == "list");
}

/// One sidebar library command (POLA-162). Saves the settings when they changed.
fn command(ui: &AppWindow, app: &mut AppState, models: &UiModels, verb: &str, part: &str, arg: &str) {
    let step = |i: usize, n: usize| if arg == "-1" { (i + n - 1) % n } else { (i + 1) % n };
    let mut save = true;
    match verb {
        // Part menu about to open: is the part hidden, which libraries can it go to.
        "menu" => {
            let s = &app.settings;
            ui.set_library_menu_hidden(s.hidden_parts.contains(part));
            ui.set_library_menu_targets(strings(
                s.libraries.iter().filter(|l| !l.parts.iter().any(|p| p == part)).map(|l| l.name.as_str().into()).collect(),
            ));
            return;
        }
        "new" => {
            app.library.pending_part = (!part.is_empty()).then(|| part.to_string());
            return;
        }
        "cancel" => {
            app.library.pending_part = None;
            return;
        }
        "step" => {
            let s = &mut app.settings;
            let names: Vec<&str> = std::iter::once("").chain(s.libraries.iter().map(|l| l.name.as_str())).collect();
            let i = names.iter().position(|n| *n == s.active_library).unwrap_or(0);
            s.active_library = names[step(i, names.len())].to_string();
        }
        "create" => {
            let Some(name) = unique_name(&app.settings.libraries, arg) else { return };
            let parts = app.library.pending_part.take().into_iter().collect();
            app.settings.libraries.push(UserLibrary { name: name.clone(), parts });
            app.settings.active_library = name;
        }
        "delete" => {
            let name = std::mem::take(&mut app.settings.active_library);
            app.settings.libraries.retain(|l| l.name != name);
        }
        "hide" => {
            if !app.settings.hidden_parts.remove(part) {
                app.settings.hidden_parts.insert(part.to_string());
            }
        }
        "add" => {
            if let Some(lib) = app.settings.libraries.iter_mut().find(|l| l.name == arg) {
                if !lib.parts.iter().any(|p| p == part) {
                    lib.parts.push(part.to_string());
                }
            }
        }
        "remove" => {
            let name = app.settings.active_library.clone();
            if let Some(lib) = app.settings.libraries.iter_mut().find(|l| l.name == name) {
                lib.parts.retain(|p| p != part);
            }
        }
        "type" => {
            save = false;
            let keys: Vec<Option<&str>> = std::iter::once(None).chain(GROUPS.iter().map(|g| Some(g.0))).collect();
            let i = keys.iter().position(|k| *k == app.library.group.as_deref()).unwrap_or(0);
            app.library.group = keys[step(i, keys.len())].map(str::to_string);
        }
        "vendor" => {
            save = false;
            let list = vendors(&app.catalog, &filter(app, None));
            let keys: Vec<Option<&str>> = std::iter::once(None).chain(list.iter().map(|v| Some(v.as_str()))).collect();
            let i = keys.iter().position(|k| *k == app.library.vendor.as_deref()).unwrap_or(0);
            app.library.vendor = keys[step(i, keys.len())].map(str::to_string);
        }
        "units" => {
            save = false;
            app.library.units = match arg {
                "1" => UnitFilter::Metric,
                "2" => UnitFilter::Inch,
                _ => UnitFilter::All,
            };
        }
        "hidden" => {
            save = false;
            app.library.show_hidden = !app.library.show_hidden;
        }
        // POLA-165 library menu: Grid | List (saved) and the column count for its width.
        "view" => app.settings.library_view = arg.to_string(),
        "columns" => {
            let n = arg.parse().unwrap_or(1);
            if n == app.library.columns {
                return;
            }
            save = false;
            app.library.columns = n;
        }
        _ => return,
    }
    if save {
        if let Err(error) = lab_file::save_settings(&app.settings) {
            eprintln!("settings not saved: {error}");
        }
    }
    sync_ui(ui, app);
    fill(app, models);
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

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_library_cmd(move |verb, part, arg| {
        let Some(ui) = w.upgrade() else { return };
        command(&ui, &mut a.borrow_mut(), &m, &verb, &part, &arg);
    });
    sync_ui(ui, &app.borrow());

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
    // The viewport fills the window (the sidebar floats over its left part), except in Split,
    // where it starts at `viewport_x`.
    let vx = ui.get_viewport_x();
    let inside =
        x > ui.get_sidebar_right() && x < vx + ui.get_viewport_width() && y >= 0.0 && y < ui.get_viewport_height();
    let pos = Vec2::new((x - vx) * scale, y * scale);
    let ds = ui.global::<lab_ui::DragState>();
    let mods = Modifiers { shift: ds.get_shift(), ctrl: ds.get_ctrl(), alt: false };
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

    fn def(id: &str, name: &str, category: &str) -> PartDef {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "category": category, "part_number": name,
            "geometry": { "kind": "parametric", "generator": "post" },
        }))
        .unwrap()
    }

    fn filt<'a>(query: &'a str, show_builtin: bool, hidden: &'a BTreeSet<String>) -> LibraryFilter<'a> {
        LibraryFilter { query, library: None, group: None, units: UnitFilter::All, vendor: None, show_builtin, show_hidden: false, hidden }
    }

    fn ids(rows: &[LibraryRow]) -> Vec<String> {
        rows.iter().filter(|r| !r.header).map(|r| r.id.to_string()).collect()
    }

    #[test]
    fn groups_and_search() {
        let catalog = Catalog::from_defs(vec![
            def("thorlabs:KM100", "KM100", "mounts/kinematic"),
            def("thorlabs:TR50/M", "TR50/M", "posts/posts"),
        ]);
        let none = BTreeSet::new();
        // `from_defs` marks every part built-in: hidden unless asked for.
        assert!(rows(&catalog, &filt("", false, &none), &BTreeSet::new()).is_empty());
        let all = rows(&catalog, &filt("", true, &none), &BTreeSet::new());
        let labels: Vec<_> = all.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["Posts", "TR50/M", "Mounts", "KM100"]);
        let found = rows(&catalog, &filt("km", true, &none), &BTreeSet::from(["mounts".to_string()]));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].id, "thorlabs:KM100");
        let collapsed = rows(&catalog, &filt("", true, &none), &BTreeSet::from(["posts".to_string()]));
        assert_eq!(collapsed.len(), 3);
    }

    #[test]
    fn grid_layout_rows() {
        let part = LibraryRow::default();
        let header = LibraryRow { header: true, ..Default::default() };
        let rows: Vec<_> = std::iter::once(header.clone())
            .chain(std::iter::repeat(part.clone()).take(5))
            .chain(std::iter::once(header))
            .chain(std::iter::repeat(part).take(2))
            .collect();
        let (row, h, w) = (TILE_H + TILE_GAP, HEADER_H + TILE_GAP, TILE_W + TILE_GAP);
        let y1 = h;
        let y2 = h + row;
        let y3 = h + 2.0 * row;
        let y4 = y3 + h;
        assert_eq!(
            grid_layout(&rows, 3),
            [(0.0, 0.0), (0.0, y1), (w, y1), (2.0 * w, y1), (0.0, y2), (w, y2), (0.0, y3), (0.0, y4), (w, y4)]
        );
    }

    #[test]
    fn unit_systems() {
        let mut tagged = def("a", "A", "posts");
        tagged.tags = vec!["metric".into()];
        assert_eq!(unit_system(&tagged), Some(Unit::Metric));
        assert_eq!(unit_system(&def("b", "TR50/M", "posts")), Some(Unit::Metric));
        let mut threaded = def("c", "TR2", "posts");
        threaded.connectors = serde_json::from_value(serde_json::json!([{
            "name": "top", "kind": "thread", "spec": "8-32", "gender": "female",
            "frame": { "origin": [0.0, 0.0, 0.0], "z": [0.0, 0.0, 1.0], "x": [1.0, 0.0, 0.0] },
        }]))
        .unwrap();
        assert_eq!(unit_system(&threaded), Some(Unit::Inch));
        assert_eq!(unit_system(&def("d", "KM100", "mounts")), None);
        let mut km = def("thorlabs:KM100", "KM100", "mounts");
        km.vendor = "Thorlabs".into();
        km.connectors = serde_json::from_value(serde_json::json!([{
            "name": "hole_1", "kind": "thread", "spec": "M6", "gender": "female",
            "frame": { "origin": [0.0, 0.0, 0.0], "z": [0.0, 0.0, 1.0], "x": [1.0, 0.0, 0.0] },
        }]))
        .unwrap();
        assert_eq!(unit_system(&km), Some(Unit::Inch));
        for metric in ["TR50-M", "KC1T_M", "TR50_M", "ST1XY-A_M", "SPT2-M-Stellring1", "AB_M-2", "XR25-M (x)"] {
            assert_eq!(unit_system(&def("e", metric, "posts")), Some(Unit::Metric), "{metric}");
        }
    }

    #[test]
    fn library_type_units_and_hidden_filters() {
        let catalog = Catalog::from_defs(vec![
            def("thorlabs:KM100", "KM100", "mounts/kinematic"),
            def("thorlabs:TR50/M", "TR50/M", "posts/posts"),
            def("thorlabs:TR2", "TR2", "posts/posts"),
        ]);
        let (none, c) = (BTreeSet::new(), BTreeSet::new());
        let lib = vec!["thorlabs:KM100".to_string()];
        // A separate library ignores `show_builtin`.
        let f = LibraryFilter { library: Some(&lib), ..filt("", false, &none) };
        assert_eq!(ids(&rows(&catalog, &f, &c)), ["thorlabs:KM100"]);
        let f = LibraryFilter { group: Some("posts"), ..filt("", true, &none) };
        assert_eq!(ids(&rows(&catalog, &f, &c)), ["thorlabs:TR2", "thorlabs:TR50/M"]);
        let f = LibraryFilter { units: UnitFilter::Metric, ..filt("", true, &none) };
        assert_eq!(ids(&rows(&catalog, &f, &c)), ["thorlabs:TR50/M"]);
        let hidden = BTreeSet::from(["thorlabs:KM100".to_string()]);
        assert!(!ids(&rows(&catalog, &filt("", true, &hidden), &c)).contains(&"thorlabs:KM100".to_string()));
        let shown = rows(&catalog, &LibraryFilter { show_hidden: true, ..filt("", true, &hidden) }, &c);
        assert!(shown.iter().any(|r| r.id == "thorlabs:KM100" && r.hidden));
    }
}
