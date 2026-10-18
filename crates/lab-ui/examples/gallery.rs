//! UI gallery: the real `AppWindow` driven by mock data, without the document or the viewport.
//!
//! ```text
//! cargo run -p lab-ui --example gallery [-- --state N] [--size WxH] [--selftest]
//! ```
//!
//! Keys (with the viewport focused):
//! 1 node · 2 platform (rect) · 3 platform (L) · 4 platform (custom, invalid row) ·
//! 5 room with doors · 6 sheet + toast · 7 toggle "Edited" · 9 set the surface from Rust
//! (the mock refuses "Black" when it is clicked, so the thumb must not move until 9) ·
//! 0 toggle busy.
//!
//! `--selftest` replays the keyboard contract of spec §6.4 through `Window::dispatch_event`
//! (the same routing as real key presses), prints PASS/FAIL per check and exits with a
//! non-zero status if anything failed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lab_ui::{
    AppWindow, CornerRow, DoorRow, EditField, EditState, InspectorField, LibraryRow, OutlinePreview,
    PresetRow, PreviewLabel, SceneRow, TableFocus, WallChoice, ZoneRow,
};
use slint::platform::{Key, WindowEvent};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};

// ── Mock models (one persistent VecModel per list property, as in D16) ──────────────

struct Models {
    library: Rc<VecModel<LibraryRow>>,
    scene: Rc<VecModel<SceneRow>>,
    inspector: Rc<VecModel<InspectorField>>,
    outline_fields: Rc<VecModel<EditField>>,
    corners: Rc<VecModel<CornerRow>>,
    preview_labels: Rc<VecModel<PreviewLabel>>,
    placement: Rc<VecModel<EditField>>,
    grid_fields: Rc<VecModel<EditField>>,
    zones: Rc<VecModel<ZoneRow>>,
    support_fields: Rc<VecModel<EditField>>,
    presets: Rc<VecModel<PresetRow>>,
    room_fields: Rc<VecModel<EditField>>,
    room_corners: Rc<VecModel<CornerRow>>,
    room_labels: Rc<VecModel<PreviewLabel>>,
    doors: Rc<VecModel<DoorRow>>,
    walls: Rc<VecModel<WallChoice>>,
    sheet_buttons: Rc<VecModel<SharedString>>,
}

impl Models {
    fn new(ui: &AppWindow) -> Rc<Self> {
        let m = Rc::new(Models {
            library: Rc::new(VecModel::default()),
            scene: Rc::new(VecModel::default()),
            inspector: Rc::new(VecModel::default()),
            outline_fields: Rc::new(VecModel::default()),
            corners: Rc::new(VecModel::default()),
            preview_labels: Rc::new(VecModel::default()),
            placement: Rc::new(VecModel::default()),
            grid_fields: Rc::new(VecModel::default()),
            zones: Rc::new(VecModel::default()),
            support_fields: Rc::new(VecModel::default()),
            presets: Rc::new(VecModel::default()),
            room_fields: Rc::new(VecModel::default()),
            room_corners: Rc::new(VecModel::default()),
            room_labels: Rc::new(VecModel::default()),
            doors: Rc::new(VecModel::default()),
            walls: Rc::new(VecModel::default()),
            sheet_buttons: Rc::new(VecModel::default()),
        });
        ui.set_library(ModelRc::from(m.library.clone()));
        ui.set_scene(ModelRc::from(m.scene.clone()));
        ui.set_inspector(ModelRc::from(m.inspector.clone()));
        ui.set_platform_outline_fields(ModelRc::from(m.outline_fields.clone()));
        ui.set_platform_corners(ModelRc::from(m.corners.clone()));
        ui.set_platform_preview_labels(ModelRc::from(m.preview_labels.clone()));
        ui.set_platform_placement(ModelRc::from(m.placement.clone()));
        ui.set_platform_grid_fields(ModelRc::from(m.grid_fields.clone()));
        ui.set_platform_zones(ModelRc::from(m.zones.clone()));
        ui.set_platform_support_fields(ModelRc::from(m.support_fields.clone()));
        ui.set_presets(ModelRc::from(m.presets.clone()));
        ui.set_room_fields(ModelRc::from(m.room_fields.clone()));
        ui.set_room_corners(ModelRc::from(m.room_corners.clone()));
        ui.set_room_preview_labels(ModelRc::from(m.room_labels.clone()));
        ui.set_room_doors(ModelRc::from(m.doors.clone()));
        ui.set_room_walls(ModelRc::from(m.walls.clone()));
        ui.set_sheet_buttons(ModelRc::from(m.sheet_buttons.clone()));
        m
    }

    fn edit_field_models(&self) -> [&Rc<VecModel<EditField>>; 5] {
        [&self.outline_fields, &self.placement, &self.grid_fields, &self.support_fields, &self.room_fields]
    }
}

fn sync_vec<T: Clone + 'static>(model: &VecModel<T>, items: Vec<T>) {
    let old_len = model.row_count();
    let new_len = items.len();
    for (i, item) in items.into_iter().enumerate() {
        if i < old_len {
            model.set_row_data(i, item);
        } else {
            model.push(item);
        }
    }
    for i in (new_len..old_len).rev() {
        model.remove(i);
    }
}

fn s(text: &str) -> SharedString {
    SharedString::from(text)
}

fn field(id: &str, label: &str, value: &str, unit: &str) -> EditField {
    EditField { id: s(id), label: s(label), value: s(value), unit: s(unit), enabled: true, invalid: false }
}

fn corner(index: i32, x: f64, y: f64, r: f64) -> CornerRow {
    CornerRow { index, x: fmt(x), y: fmt(y), r: fmt(r), invalid: false }
}

/// Mock number formatting: whole numbers without decimals, others with up to two.
fn fmt(v: f64) -> SharedString {
    if (v - v.round()).abs() < 1e-9 {
        SharedString::from(format!("{:.0}", v))
    } else {
        let t = format!("{:.2}", v);
        SharedString::from(t.trim_end_matches('0').trim_end_matches('.'))
    }
}

/// Outline preview data the way lab-app builds it: SVG path with y negated and a square
/// viewbox around the bounding box, padded by 8 % (at least 10 mm).
fn preview(points: &[(f64, f64)], zones: &str, error_edges: &[(usize, usize)]) -> OutlinePreview {
    let path = |pts: &[(f64, f64)], close: bool| {
        let mut out = String::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            out.push_str(&format!("{} {:.2} {:.2} ", if i == 0 { "M" } else { "L" }, x, -y));
        }
        if close {
            out.push('Z');
        }
        out
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in points {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(-y);
        y1 = y1.max(-y);
    }
    let extent = (x1 - x0).max(y1 - y0);
    let size = extent + 2.0 * (0.08 * extent).max(10.0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let mut errors = String::new();
    for &(a, b) in error_edges {
        errors.push_str(&path(&[points[a], points[b]], false));
    }
    OutlinePreview {
        outline: s(&path(points, true)),
        zones: s(zones),
        error_edges: s(&errors),
        vb_x: (cx - size / 2.0) as f32,
        vb_y: (cy - size / 2.0) as f32,
        vb_size: size as f32,
    }
}

fn corner_labels(points: &[(f64, f64)]) -> Vec<PreviewLabel> {
    points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| PreviewLabel {
            index: i as i32,
            kind: 0,
            x: x as f32,
            y: -y as f32,
            text: SharedString::from(format!("{}", i + 1)),
        })
        .collect()
}

/// Wall numbers at the wall midpoints, pushed outward by 4 % of the viewbox.
fn wall_labels(points: &[(f64, f64)], vb: &OutlinePreview) -> Vec<PreviewLabel> {
    let n = points.len();
    let area: f64 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let ccw = area > 0.0;
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-9);
            // Outward normal (document frame, y up).
            let (nx, ny) = if ccw { (dy / len, -dx / len) } else { (-dy / len, dx / len) };
            let off = 0.04 * vb.vb_size as f64;
            let (mx, my) = ((a.0 + b.0) / 2.0 + nx * off, (a.1 + b.1) / 2.0 + ny * off);
            PreviewLabel {
                index: i as i32,
                kind: 1,
                x: mx as f32,
                y: -my as f32,
                text: SharedString::from(format!("W{}", i + 1)),
            }
        })
        .collect()
}

// ── States ──────────────────────────────────────────────────────────────────────────

fn common(ui: &AppWindow, m: &Models) {
    ui.set_window_title(s("Demo Lab — LabSetup"));
    ui.set_project_name(s("Demo Lab"));
    ui.set_project_detail(s("2 tables · 37 parts · offline"));
    ui.set_dirty(false);
    ui.set_document_note(s(""));
    ui.set_can_undo(true);
    ui.set_can_redo(true);
    ui.set_undo_label(s("Undo Change Width"));
    ui.set_redo_label(s("Redo Move Mirror M3"));
    ui.set_status(s("Idle"));
    ui.set_gpu(s("gallery · no viewport"));
    ui.set_rendering(false);
    ui.set_busy(false);
    ui.set_inspector_error(s(""));
    ui.set_sheet_open(false);
    ui.set_paste_target(s(""));
    ui.set_toast(s(""));
    ui.set_toast_action(s(""));

    let tint = |r, g, b| Color::from_rgb_u8(r, g, b);
    sync_vec(
        &m.library,
        vec![
            LibraryRow { label: s("Mirror mounts"), letter: s("M"), tint: tint(10, 132, 255) },
            LibraryRow { label: s("Lenses"), letter: s("L"), tint: tint(48, 209, 88) },
            LibraryRow { label: s("Lasers"), letter: s("B"), tint: tint(255, 55, 95) },
        ],
    );
    sync_vec(
        &m.presets,
        vec![
            PresetRow { id: s("table-1200x900"), group: s("Optical tables (metric)"), label: s("Table 1200 × 900"), hint: s("M6 · 25 mm") },
            PresetRow { id: s("table-1500x1200"), group: s("Optical tables (metric)"), label: s("Table 1500 × 1200"), hint: s("M6 · 25 mm") },
            PresetRow { id: s("mb6060m"), group: s("Breadboards (metric)"), label: s("Breadboard 600 × 600"), hint: s("MB6060/M") },
            PresetRow { id: s("mb3045m"), group: s("Breadboards (metric)"), label: s("Breadboard 300 × 450"), hint: s("MB3045/M") },
            PresetRow { id: s("mb1224"), group: s("Breadboards (imperial)"), label: s("Breadboard 12 × 24 in"), hint: s("MB1224") },
            PresetRow { id: s("custom"), group: s("Custom"), label: s("Custom platform"), hint: s("") },
        ],
    );
    ui.set_display_unit(0);
    ui.set_wall_display(0);
}

fn scene(m: &Models, selected: (i32, i32)) {
    let row = |kind: i32, id: i32, label: &str, detail: &str, depth: i32, beam: bool| SceneRow {
        kind,
        id,
        label: s(label),
        detail: s(detail),
        depth,
        selected: (kind, id) == selected,
        is_beam: beam,
    };
    sync_vec(
        &m.scene,
        vec![
            row(2, 0, "Room", "6.0 × 5.0 m", 0, false),
            row(1, 1, "Table A", "1200 × 900", 0, false),
            row(0, 10, "Laser 633 nm", "", 1, false),
            row(0, 11, "Beam", "", 1, true),
            row(0, 12, "Mirror M3", "C17", 1, false),
            row(0, 13, "Lens · f=100", "D21", 1, false),
            row(1, 2, "Breadboard L", "900 × 600", 0, false),
            row(0, 20, "Detector", "B4", 1, false),
        ],
    );
}

fn rect_points(w: f64, d: f64) -> Vec<(f64, f64)> {
    vec![(-w / 2.0, -d / 2.0), (w / 2.0, -d / 2.0), (w / 2.0, d / 2.0), (-w / 2.0, d / 2.0)]
}

fn state_node(ui: &AppWindow, m: &Models) {
    scene(m, (0, 12));
    ui.set_selection_kind(1);
    ui.set_selection_title(s("Mirror M3"));
    ui.set_selection_detail(s("Kinematic mirror mount · on Table A"));
    sync_vec(
        &m.inspector,
        vec![
            InspectorField { label: s("Position"), value: s("412.5, 287.5 mm") },
            InspectorField { label: s("Hole"), value: s("C17 · row 12") },
            InspectorField { label: s("Height"), value: s("75.0 mm") },
            InspectorField { label: s("Rotation"), value: s("45°") },
            InspectorField { label: s("Part"), value: s("KM100") },
        ],
    );
}

fn platform_common(ui: &AppWindow, m: &Models) {
    ui.set_selection_kind(2);
    ui.set_platform_node_count(12);
    ui.set_platform_warning(s(""));
    ui.set_platform_grid_note(s(""));
    ui.set_platform_corner_invalid(-1);
    sync_vec(
        &m.placement,
        vec![
            field("platform.x", "X", "0", "mm"),
            field("platform.y", "Y", "0", "mm"),
            field("platform.rotation", "Rotation", "0", "°"),
            field("platform.top_height", "Top height", "750", "mm"),
            field("platform.thickness", "Thickness", "203.2", "mm"),
        ],
    );
}

fn state_platform_rect(ui: &AppWindow, m: &Models) {
    scene(m, (1, 1));
    platform_common(ui, m);
    ui.set_platform_name(s("Table A"));
    ui.set_platform_subtitle(s("Optical table · 1200 × 900 mm"));
    ui.set_platform_kind(0);
    ui.set_platform_surface(0);
    ui.set_platform_outline_kind(0);
    sync_vec(
        &m.outline_fields,
        vec![
            field("platform.outline.width", "Width", "1200", "mm"),
            field("platform.outline.depth", "Depth", "900", "mm"),
        ],
    );
    let pts = rect_points(1200.0, 900.0);
    sync_vec(&m.corners, pts.iter().enumerate().map(|(i, p)| corner(i as i32, p.0, p.1, 0.0)).collect::<Vec<_>>());
    ui.set_platform_preview(preview(&pts, "M 225.00 -250.00 L 375.00 -250.00 L 375.00 -150.00 L 225.00 -150.00 Z", &[]));
    sync_vec(&m.preview_labels, corner_labels(&pts));
    ui.set_platform_grid_standard(1);
    ui.set_platform_thread(1);
    ui.set_platform_origin_mode(0);
    sync_vec(&m.grid_fields, vec![]);
    ui.set_platform_hole_summary(s("1,692 holes · 47 × 36 grid"));
    sync_vec(
        &m.zones,
        vec![ZoneRow {
            index: 0,
            name: s("Isolator post"),
            kind: 0,
            x: s("300"),
            y: s("200"),
            w: s("150"),
            h: s("100"),
            r: s(""),
        }],
    );
    ui.set_platform_support(0);
    sync_vec(
        &m.support_fields,
        vec![
            field("platform.support.leg_size", "Leg size", "80", "mm"),
            field("platform.support.leg_inset", "Leg inset", "100", "mm"),
        ],
    );
}

fn state_platform_l(ui: &AppWindow, m: &Models) {
    scene(m, (1, 2));
    platform_common(ui, m);
    ui.set_platform_node_count(1);
    ui.set_platform_name(s("Breadboard L"));
    ui.set_platform_subtitle(s("Breadboard · 900 × 600 mm · L-shape"));
    ui.set_platform_kind(1);
    ui.set_platform_surface(1);
    ui.set_platform_outline_kind(2);
    ui.set_platform_notch_corner(1);
    sync_vec(
        &m.outline_fields,
        vec![
            field("platform.outline.width", "Width", "900", "mm"),
            field("platform.outline.depth", "Depth", "600", "mm"),
            field("platform.outline.notch_width", "Notch width", "300", "mm"),
            field("platform.outline.notch_depth", "Notch depth", "250", "mm"),
        ],
    );
    // Notch at the bottom-right (↘) corner.
    let pts = vec![(-450.0, -300.0), (150.0, -300.0), (150.0, -50.0), (450.0, -50.0), (450.0, 300.0), (-450.0, 300.0)];
    sync_vec(&m.corners, pts.iter().enumerate().map(|(i, p)| corner(i as i32, p.0, p.1, 0.0)).collect::<Vec<_>>());
    ui.set_platform_preview(preview(
        &pts,
        "M -350.00 -150.00 L -200.00 -150.00 L -200.00 -50.00 L -350.00 -50.00 Z M 300.00 -150.00 A 60 60 0 1 0 300.00 -149.99 Z",
        &[],
    ));
    sync_vec(&m.preview_labels, corner_labels(&pts));
    ui.set_platform_grid_standard(2);
    ui.set_platform_thread(3);
    ui.set_platform_origin_mode(1);
    sync_vec(
        &m.grid_fields,
        vec![
            field("platform.grid.origin_x", "Origin X", "-431.8", "mm"),
            field("platform.grid.origin_y", "Origin Y", "-279.4", "mm"),
        ],
    );
    ui.set_platform_hole_summary(s("744 holes · 35 × 23 grid"));
    ui.set_platform_grid_note(s("Grid fixed because parts are placed on this platform"));
    sync_vec(
        &m.zones,
        vec![
            ZoneRow { index: 0, name: s("Post base"), kind: 0, x: s("-275"), y: s("100"), w: s("150"), h: s("100"), r: s("") },
            ZoneRow { index: 1, name: s(""), kind: 1, x: s("300"), y: s("150"), w: s(""), h: s(""), r: s("60") },
        ],
    );
    ui.set_platform_support(1);
    sync_vec(&m.support_fields, vec![]);
    ui.set_platform_warning(s("2 parts lie outside the outline"));
}

fn state_platform_custom(ui: &AppWindow, m: &Models) {
    scene(m, (1, 2));
    platform_common(ui, m);
    ui.set_platform_name(s("Breadboard L"));
    ui.set_platform_subtitle(s("Custom · 6 corners"));
    ui.set_platform_kind(2);
    ui.set_platform_surface(0);
    ui.set_platform_outline_kind(3);
    sync_vec(&m.outline_fields, vec![]);
    // Corner 3 makes edge 3 cross edge 1.
    let pts = vec![(-500.0, -300.0), (500.0, -300.0), (500.0, 300.0), (-200.0, -450.0), (-350.0, 150.0), (-500.0, 300.0)];
    let mut rows: Vec<CornerRow> = pts.iter().enumerate().map(|(i, p)| corner(i as i32, p.0, p.1, 0.0)).collect();
    rows[1].r = s("25");
    rows[2].invalid = true;
    sync_vec(&m.corners, rows);
    ui.set_platform_corner_invalid(2);
    ui.set_platform_preview(preview(&pts, "", &[(0, 1), (2, 3)]));
    sync_vec(&m.preview_labels, corner_labels(&pts));
    ui.set_inspector_error(s("Edges 1 and 3 cross. Move corner 3 or 4 so that the outline does not intersect itself."));
    ui.set_platform_grid_standard(3);
    ui.set_platform_thread(2);
    ui.set_platform_origin_mode(0);
    sync_vec(
        &m.grid_fields,
        vec![
            field("platform.grid.pitch_x", "Pitch X", "12.5", "mm"),
            field("platform.grid.pitch_y", "Pitch Y", "12.5", "mm"),
            field("platform.grid.margin", "Margin", "15", "mm"),
        ],
    );
    ui.set_platform_hole_summary(s("Holes not updated while the outline is invalid"));
    sync_vec(&m.zones, vec![]);
    ui.set_platform_support(0);
    sync_vec(
        &m.support_fields,
        vec![
            field("platform.support.leg_size", "Leg size", "60", "mm"),
            field("platform.support.leg_inset", "Leg inset", "80", "mm"),
        ],
    );
}

fn state_room(ui: &AppWindow, m: &Models) {
    scene(m, (2, 0));
    ui.set_selection_kind(3);
    ui.set_room_subtitle(s("6.00 × 5.00 m · 25.0 m² · 2 doors · 1 window"));
    ui.set_room_is_rectangle(false);
    sync_vec(
        &m.room_fields,
        vec![
            field("room.height", "Wall height", "2700", "mm"),
            field("room.thickness", "Wall thickness", "150", "mm"),
        ],
    );
    let pts = vec![(0.0, 0.0), (6000.0, 0.0), (6000.0, 3000.0), (3500.0, 3000.0), (3500.0, 5000.0), (0.0, 5000.0)];
    sync_vec(&m.room_corners, pts.iter().enumerate().map(|(i, p)| corner(i as i32, p.0, p.1, 0.0)).collect::<Vec<_>>());
    ui.set_room_corner_invalid(-1);
    let vb = preview(&pts, "", &[]);
    let mut labels = corner_labels(&pts);
    labels.extend(wall_labels(&pts, &vb));
    ui.set_room_preview(vb);
    sync_vec(&m.room_labels, labels);
    let dirs = ["South", "East", "North", "East", "North", "West"];
    let walls: Vec<WallChoice> = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() / 1000.0;
            WallChoice { index: i as i32, label: SharedString::from(format!("Wall {} · {} · {:.2} m", i + 1, dirs[i], len)) }
        })
        .collect();
    let label_of = |w: usize| walls[w].label.clone();
    sync_vec(
        &m.doors,
        vec![
            DoorRow { id: 41, name: s("Door 1"), wall: 0, wall_label: label_of(0), offset: s("800"), width: s("900"), height: s("2100"), sill: s("0") },
            DoorRow { id: 42, name: s("Window 1"), wall: 1, wall_label: label_of(1), offset: s("1000"), width: s("1200"), height: s("1200"), sill: s("900") },
            DoorRow { id: 43, name: s("Door 2"), wall: 4, wall_label: label_of(4), offset: s("1200"), width: s("1000"), height: s("2100"), sill: s("0") },
        ],
    );
    sync_vec(&m.walls, walls);
}

fn state_sheet(ui: &AppWindow, m: &Models) {
    state_platform_rect(ui, m);
    ui.set_dirty(true);
    ui.set_window_title(s("Demo Lab — Edited — LabSetup"));
    ui.set_document_note(s("Recovered — not saved yet"));
    ui.set_busy(true);
    ui.set_status(s("Saving…"));
    open_sheet(ui, m);
    ui.set_toast(s("Door 1 moved to fit wall 2"));
    ui.set_toast_action(s("Undo"));
}

fn open_sheet(ui: &AppWindow, m: &Models) {
    ui.set_sheet_title(s("Save changes to “Demo Lab” before opening another project?"));
    ui.set_sheet_message(s("Your changes will be lost if you don’t save them."));
    sync_vec(&m.sheet_buttons, vec![s("Save"), s("Don’t Save"), s("Cancel")]);
    ui.set_sheet_default(0);
    ui.set_sheet_destructive(1);
    ui.set_sheet_open(true);
}

fn apply_state(ui: &AppWindow, m: &Models, state: u32) {
    common(ui, m);
    match state {
        1 => state_node(ui, m),
        2 => state_platform_rect(ui, m),
        3 => state_platform_l(ui, m),
        4 => state_platform_custom(ui, m),
        5 => state_room(ui, m),
        _ => state_sheet(ui, m),
    }
}

// ── Mock controller ─────────────────────────────────────────────────────────────────

type Log = Rc<RefCell<Vec<String>>>;

fn record(log: &Log, entry: String) {
    println!("{entry}");
    log.borrow_mut().push(entry);
}

/// Accepts a number (or any non-empty name) and writes the formatted value back into the
/// model row in place, as lab-app does.
fn commit(ui: &AppWindow, m: &Models, id: &str, text: &str) -> bool {
    let text = text.trim();
    let is_name = id.ends_with(".name");
    let value = if is_name {
        if text.is_empty() {
            ui.set_inspector_error(s("A name cannot be empty."));
            return false;
        }
        SharedString::from(text)
    } else {
        match text.parse::<f64>() {
            Ok(v) if v.is_finite() => fmt(v),
            _ => {
                ui.set_inspector_error(SharedString::from(format!("“{text}” is not a number.")));
                return false;
            }
        }
    };
    ui.set_inspector_error(s(""));
    if id == "platform.name" {
        ui.set_platform_name(value);
        return true;
    }
    for model in m.edit_field_models() {
        for row in 0..model.row_count() {
            let mut f = model.row_data(row).unwrap_or_default();
            if f.id.as_str() == id {
                f.value = value;
                model.set_row_data(row, f);
                return true;
            }
        }
    }
    let parts: Vec<&str> = id.split('.').collect();
    match parts.as_slice() {
        [prefix @ ("platform" | "room"), "corner", k, axis] => {
            let model = if *prefix == "platform" { &m.corners } else { &m.room_corners };
            let Some(row) = k.parse::<usize>().ok().filter(|&r| r < model.row_count()) else { return false };
            let mut c = model.row_data(row).unwrap_or_default();
            match *axis {
                "x" => c.x = value,
                "y" => c.y = value,
                _ => c.r = value,
            }
            model.set_row_data(row, c);
            true
        }
        ["platform", "zone", k, what] => {
            let Some(row) = k.parse::<usize>().ok().filter(|&r| r < m.zones.row_count()) else { return false };
            let mut z = m.zones.row_data(row).unwrap_or_default();
            match *what {
                "name" => z.name = value,
                "x" => z.x = value,
                "y" => z.y = value,
                "w" => z.w = value,
                "h" => z.h = value,
                _ => z.r = value,
            }
            m.zones.set_row_data(row, z);
            true
        }
        ["room", "door", id, what] => {
            for row in 0..m.doors.row_count() {
                let mut d = m.doors.row_data(row).unwrap_or_default();
                if d.id.to_string() == *id {
                    match *what {
                        "name" => d.name = value,
                        "offset" => d.offset = value,
                        "width" => d.width = value,
                        "height" => d.height = value,
                        _ => d.sill = value,
                    }
                    m.doors.set_row_data(row, d);
                    return true;
                }
            }
            false
        }
        _ => true,
    }
}

fn choose(ui: &AppWindow, m: &Models, id: &str, index: i32) {
    match id {
        "units" => ui.set_display_unit(index),
        "walls" => ui.set_wall_display(index),
        "platform.kind" => ui.set_platform_kind(index),
        // The mock refuses the black surface: the property stays, so the thumb must not move.
        "platform.surface" if index == 1 => {
            ui.set_toast(s("The mock refused “Black” (press 9 to set it from Rust)"));
            ui.set_toast_action(s(""));
        }
        "platform.surface" => ui.set_platform_surface(index),
        "platform.outline.kind" => ui.set_platform_outline_kind(index),
        "platform.outline.notch_corner" => ui.set_platform_notch_corner(index),
        "platform.grid.standard" => ui.set_platform_grid_standard(index),
        "platform.grid.thread" => ui.set_platform_thread(index),
        "platform.grid.origin" => ui.set_platform_origin_mode(index),
        "platform.support" => ui.set_platform_support(index),
        _ if id.starts_with("room.door.") && id.ends_with(".wall") => {
            let door_id = id.trim_start_matches("room.door.").trim_end_matches(".wall");
            let label = m.walls.row_data(index as usize).map(|w| w.label).unwrap_or_default();
            for row in 0..m.doors.row_count() {
                let mut d = m.doors.row_data(row).unwrap_or_default();
                if d.id.to_string() == door_id {
                    d.wall = index;
                    d.wall_label = label.clone();
                    m.doors.set_row_data(row, d);
                }
            }
        }
        _ => {}
    }
}

fn act(ui: &AppWindow, id: &str) {
    match id {
        "inspector.dismiss_error" => ui.set_inspector_error(s("")),
        "paste.cancel" => ui.set_paste_target(s("")),
        "platform.corners.paste" => ui.set_paste_target(s("platform")),
        "room.corners.paste" => ui.set_paste_target(s("room")),
        "view.top" => ui.set_view_mode(1),
        "view.3d" => ui.set_view_mode(0),
        _ => {}
    }
}

fn paste(ui: &AppWindow, m: &Models, target: &str, text: &str) -> SharedString {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let nums: Result<Vec<f64>, _> =
            line.split([',', '\t']).map(|t| t.trim().parse::<f64>()).collect();
        match nums {
            Ok(v) if v.len() == 2 || v.len() == 3 => {
                rows.push(corner(rows.len() as i32, v[0], v[1], v.get(2).copied().unwrap_or(0.0)));
            }
            _ => return SharedString::from(format!("Line {}: expected “x, y” or “x, y, r”.", n + 1)),
        }
    }
    if rows.len() < 3 {
        return s("An outline needs at least 3 corners.");
    }
    if target == "room" { sync_vec(&m.room_corners, rows) } else { sync_vec(&m.corners, rows) }
    ui.set_paste_target(s(""));
    s("")
}

fn wire(ui: &AppWindow, m: &Rc<Models>, log: &Log) {
    let weak = ui.as_weak();
    {
        let (w, m, log) = (weak.clone(), m.clone(), log.clone());
        ui.on_field_committed(move |id, text| {
            record(&log, format!("field-committed({id}, {text})"));
            w.upgrade().is_some_and(|ui| commit(&ui, &m, &id, &text))
        });
    }
    {
        let log = log.clone();
        ui.on_field_edited(move |id, text| {
            // Live edits are noisy; keep them out of stdout but in the log.
            log.borrow_mut().push(format!("field-edited({id}, {text})"));
        });
    }
    {
        let (w, m, log) = (weak.clone(), m.clone(), log.clone());
        ui.on_choice_selected(move |id, index| {
            record(&log, format!("choice-selected({id}, {index})"));
            if let Some(ui) = w.upgrade() {
                choose(&ui, &m, &id, index);
            }
        });
    }
    {
        let (w, log) = (weak.clone(), log.clone());
        ui.on_action(move |id| {
            record(&log, format!("action({id})"));
            if let Some(ui) = w.upgrade() {
                act(&ui, &id);
            }
        });
    }
    {
        let (w, m, log) = (weak.clone(), m.clone(), log.clone());
        ui.on_paste_corners(move |target, text| {
            record(&log, format!("paste-corners({target}, {text:?})"));
            w.upgrade().map(|ui| paste(&ui, &m, &target, &text)).unwrap_or_default()
        });
    }
    {
        let (w, log) = (weak.clone(), log.clone());
        ui.on_sheet_button(move |i| {
            record(&log, format!("sheet-button({i})"));
            if let Some(ui) = w.upgrade() {
                ui.set_sheet_open(false);
                ui.set_busy(false);
                ui.set_status(s("Idle"));
            }
        });
    }
    {
        let (w, log) = (weak.clone(), log.clone());
        ui.on_toast_action_clicked(move || {
            record(&log, "toast-action-clicked".into());
            if let Some(ui) = w.upgrade() {
                ui.set_toast(s(""));
            }
        });
    }
    {
        let (w, m, log) = (weak.clone(), m.clone(), log.clone());
        ui.on_scene_row_clicked(move |kind, id| {
            record(&log, format!("scene-row-clicked({kind}, {id})"));
            if let Some(ui) = w.upgrade() {
                match kind {
                    0 => apply_state(&ui, &m, 1),
                    1 if id == 2 => apply_state(&ui, &m, 3),
                    1 => apply_state(&ui, &m, 2),
                    _ => apply_state(&ui, &m, 5),
                }
            }
        });
    }
    {
        let log = log.clone();
        ui.on_view_mode_requested(move |mode| record(&log, format!("view-mode-requested({mode})")));
    }
    {
        let (w, log) = (weak.clone(), log.clone());
        ui.on_clear_selection(move || {
            record(&log, "clear-selection".into());
            if let Some(ui) = w.upgrade() {
                ui.set_selection_kind(0);
            }
        });
    }
    {
        let (w, m, log) = (weak.clone(), m.clone(), log.clone());
        ui.on_gallery_key(move |text| {
            let Some(ui) = w.upgrade() else { return false };
            match text.as_str() {
                k @ ("1" | "2" | "3" | "4" | "5" | "6") => {
                    let state = k.parse().unwrap_or(2);
                    record(&log, format!("gallery-state({state})"));
                    apply_state(&ui, &m, state);
                    true
                }
                "7" => {
                    ui.set_dirty(!ui.get_dirty());
                    true
                }
                "9" => {
                    ui.set_platform_surface(1 - ui.get_platform_surface());
                    record(&log, format!("rust-set-surface({})", ui.get_platform_surface()));
                    true
                }
                "0" => {
                    ui.set_busy(!ui.get_busy());
                    true
                }
                _ => false,
            }
        });
    }
}

// ── Self-test (spec §6.4 keyboard contract, R5) ────────────────────────────────────

struct Tester {
    ui: AppWindow,
    models: Rc<Models>,
    log: Log,
    failures: Cell<u32>,
    passes: Cell<u32>,
}

impl Tester {
    fn press(&self, key: Key) {
        self.ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    }
    fn release(&self, key: Key) {
        self.ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
    fn tap(&self, text: &str) {
        let text = SharedString::from(text);
        self.ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.ui.window().dispatch_event(WindowEvent::KeyReleased { text });
    }
    fn tap_key(&self, key: Key) {
        self.press(key);
        self.release(key);
    }
    fn chord(&self, modifiers: &[Key], text: &str) {
        for &k in modifiers {
            self.press(k);
        }
        self.tap(text);
        for &k in modifiers.iter().rev() {
            self.release(k);
        }
    }
    fn chord_key(&self, modifiers: &[Key], key: Key) {
        for &k in modifiers {
            self.press(k);
        }
        self.tap_key(key);
        for &k in modifiers.iter().rev() {
            self.release(k);
        }
    }
    fn focus_field(&self, id: &str) {
        self.ui.global::<TableFocus>().set_request(SharedString::from(id));
    }
    fn focused_id(&self) -> String {
        self.ui.global::<EditState>().get_focused_id().to_string()
    }
    fn clear_log(&self) {
        self.log.borrow_mut().clear();
    }
    fn logged(&self, needle: &str) -> bool {
        self.log.borrow().iter().any(|e| e.contains(needle))
    }
    fn check(&self, name: &str, ok: bool, detail: impl FnOnce() -> String) {
        if ok {
            self.passes.set(self.passes.get() + 1);
            println!("PASS {name}");
        } else {
            self.failures.set(self.failures.get() + 1);
            println!("FAIL {name}: {}", detail());
        }
    }
    fn log_text(&self) -> String {
        format!("log = {:?}, focused-id = {:?}", self.log.borrow(), self.focused_id())
    }
}

type Step = Box<dyn Fn(&Tester)>;

fn selftest_steps() -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut step = |f: Box<dyn Fn(&Tester)>| steps.push(f);

    // 1. Enter on an unchanged field: no field-committed, focus moves to the viewport.
    step(Box::new(|t| {
        apply_state(&t.ui, &t.models, 2);
        t.focus_field("platform.outline.width");
    }));
    step(Box::new(|t| {
        t.check("focus by TableFocus", t.focused_id() == "platform.outline.width", || t.log_text());
        t.clear_log();
        t.tap_key(Key::Return);
    }));
    step(Box::new(|t| {
        t.check("enter-unchanged-no-commit", !t.logged("field-committed"), || t.log_text());
        t.check(
            "enter-unchanged-blurs",
            !t.ui.global::<EditState>().get_text_focused() && t.focused_id().is_empty(),
            || t.log_text(),
        );
        t.focus_field("platform.outline.width");
    }));
    // 2. Ctrl+Z with a clean focused field undoes the document.
    step(Box::new(|t| {
        t.clear_log();
        t.chord(&[Key::Control], "z");
        t.check("ctrl-z-clean-field-undoes-document", t.logged("action(edit.undo)"), || t.log_text());
        t.check("ctrl-z-clean-field-keeps-focus", t.focused_id() == "platform.outline.width", || t.log_text());
        // 3. Ctrl+Z after typing does text undo instead. Replacing the selected "1200" with
        // "5" pushes two undo items in Slint's TextInput (remove "1200", insert "5"): the
        // first Ctrl+Z undoes the insert (leaving "", still dirty, so Ctrl+Y is text redo),
        // and two Ctrl+Z after that restore "1200" (clean again).
        t.tap("5");
        t.check("typing-marks-field-dirty", t.ui.global::<EditState>().get_field_dirty(), || t.log_text());
        t.clear_log();
        t.chord(&[Key::Control], "z");
        t.check("ctrl-z-dirty-field-no-document-undo", !t.logged("action(edit.undo)"), || t.log_text());
        t.clear_log();
        t.chord(&[Key::Control], "y");
        t.check("ctrl-y-after-text-undo-is-text-redo", !t.logged("action(edit.redo)"), || t.log_text());
        t.chord(&[Key::Control], "z");
        t.chord(&[Key::Control], "z");
        t.check("ctrl-z-dirty-field-text-undo", !t.ui.global::<EditState>().get_field_dirty(), || t.log_text());
        t.tap_key(Key::Escape);
    }));
    // Enter on a rejected value keeps focus and the text.
    step(Box::new(|t| {
        t.check("escape-blurs", t.focused_id().is_empty(), || t.log_text());
        t.focus_field("platform.placement.none");
        t.focus_field("platform.x");
    }));
    step(Box::new(|t| {
        t.tap("a");
        t.tap("b");
        t.clear_log();
        t.tap_key(Key::Return);
        t.check("rejected-commit-called", t.logged("field-committed(platform.x, ab)"), || t.log_text());
        t.check("rejected-commit-keeps-focus", t.focused_id() == "platform.x", || t.log_text());
        t.tap_key(Key::Escape);
    }));
    // 4. Enter in a corner cell moves to the same column of the next row.
    step(Box::new(|t| {
        apply_state(&t.ui, &t.models, 4);
    }));
    step(Box::new(|t| {
        t.focus_field("platform.corner.0.x");
    }));
    step(Box::new(|t| {
        t.check("corner-cell-focused", t.focused_id() == "platform.corner.0.x", || t.log_text());
        t.clear_log();
        t.tap_key(Key::Return);
    }));
    step(Box::new(|t| {
        t.check("corner-enter-moves-to-next-row", t.focused_id() == "platform.corner.1.x", || t.log_text());
        t.check("corner-enter-unchanged-no-commit", !t.logged("field-committed"), || t.log_text());
        t.tap("7");
        t.tap_key(Key::Return);
        t.check("corner-enter-commits", t.logged("field-committed(platform.corner.1.x, 7)"), || t.log_text());
    }));
    step(Box::new(|t| {
        t.check("corner-commit-moves-to-next-row", t.focused_id() == "platform.corner.2.x", || t.log_text());
        let x = t.models.corners.row_data(1).map(|c| c.x.to_string()).unwrap_or_default();
        t.check("corner-commit-updates-model", x == "7", || format!("corner 2 x = {x}"));
        t.tap_key(Key::Escape);
    }));
    // 5. Shift+F frames the selection, F frames all (viewport keys).
    step(Box::new(|t| {
        t.check("escape-returns-to-viewport", t.focused_id().is_empty(), || t.log_text());
        t.clear_log();
        t.chord(&[Key::Shift], "F");
        t.tap("f");
        let log = t.log.borrow().clone();
        t.check(
            "shift-f-then-f",
            log == ["action(view.frame_selection)", "action(view.frame_all)"],
            || format!("{log:?}"),
        );
        // Document undo/redo and delete with the viewport focused.
        t.clear_log();
        t.chord(&[Key::Control], "z");
        t.chord(&[Key::Control], "y");
        t.chord(&[Key::Control, Key::Shift], "Z");
        t.tap_key(Key::Delete);
        let log = t.log.borrow().clone();
        t.check(
            "viewport-undo-redo-delete",
            log == ["action(edit.undo)", "action(edit.redo)", "action(edit.redo)", "action(selection.delete)"],
            || format!("{log:?}"),
        );
        t.clear_log();
        t.chord(&[Key::Control], "s");
        t.chord(&[Key::Control, Key::Shift], "S");
        let log = t.log.borrow().clone();
        t.check("ctrl-s-ctrl-shift-s", log == ["action(file.save)", "action(file.save_as)"], || format!("{log:?}"));
        t.focus_field("platform.grid.margin");
    }));
    step(Box::new(|t| {
        t.clear_log();
        t.tap_key(Key::Backspace);
        t.tap_key(Key::Delete);
        t.check("delete-in-field-edits-text", !t.logged("selection.delete"), || t.log_text());
        t.tap_key(Key::Escape);
    }));
    // 6. Sheet: Return/Escape reach the sheet even if a field had focus; focus returns.
    step(Box::new(|t| {
        apply_state(&t.ui, &t.models, 2);
    }));
    step(Box::new(|t| {
        t.focus_field("platform.outline.width");
    }));
    step(Box::new(|t| {
        t.tap("9");
        t.clear_log();
        open_sheet(&t.ui, &t.models);
    }));
    step(Box::new(|t| {
        t.check("sheet-open-blurs-and-commits-field", t.logged("field-committed(platform.outline.width, 9)"), || t.log_text());
        t.check("sheet-open-takes-focus", t.focused_id().is_empty(), || t.log_text());
        t.clear_log();
        t.chord(&[Key::Control], "z");
        t.check("sheet-blocks-shortcuts", !t.logged("edit.undo"), || t.log_text());
        t.tap_key(Key::Return);
        t.check("sheet-return-default-button", t.logged("sheet-button(0)"), || t.log_text());
    }));
    step(Box::new(|t| {
        t.clear_log();
        t.tap("f");
        t.check("sheet-close-returns-focus-to-viewport", t.logged("action(view.frame_all)"), || t.log_text());
        t.focus_field("platform.outline.depth");
    }));
    step(Box::new(|t| {
        open_sheet(&t.ui, &t.models);
    }));
    step(Box::new(|t| {
        t.clear_log();
        t.tap_key(Key::Escape);
        t.check("sheet-escape-last-button", t.logged("sheet-button(2)"), || t.log_text());
    }));
    // 7. PasteSheet: Enter inserts a newline, Ctrl+Return applies, Escape cancels.
    step(Box::new(|t| {
        t.ui.set_paste_target(s("platform"));
    }));
    step(Box::new(|t| {
        t.clear_log();
        for k in ["0", ",", " ", "0"] {
            t.tap(k);
        }
        t.tap_key(Key::Return);
        for k in ["1", "0", "0", ",", " ", "0"] {
            t.tap(k);
        }
        t.tap_key(Key::Return);
        for k in ["1", "0", "0", ",", " ", "8", "0"] {
            t.tap(k);
        }
        t.check("paste-enter-does-not-apply", !t.logged("paste-corners"), || t.log_text());
        t.chord_key(&[Key::Control], Key::Return);
        t.check(
            "paste-ctrl-return-applies-with-newlines",
            t.logged("paste-corners(platform, \"0, 0\\n100, 0\\n100, 80\")"),
            || t.log_text(),
        );
    }));
    step(Box::new(|t| {
        t.check("paste-applied-closes", t.ui.get_paste_target().is_empty(), || t.log_text());
        t.check("paste-applied-rows", t.models.corners.row_count() == 3, || t.models.corners.row_count().to_string());
        t.ui.set_paste_target(s("room"));
    }));
    step(Box::new(|t| {
        t.clear_log();
        t.tap("x");
        t.chord_key(&[Key::Control], Key::Return);
        t.check("paste-error-keeps-sheet-open", t.logged("paste-corners(room, \"x\")") && !t.ui.get_paste_target().is_empty(), || t.log_text());
        t.clear_log();
        t.tap_key(Key::Escape);
        t.check("paste-escape-cancels", t.logged("action(paste.cancel)"), || t.log_text());
    }));
    step(Box::new(|t| {
        t.clear_log();
        t.tap("f");
        t.check("paste-close-returns-focus-to-viewport", t.logged("action(view.frame_all)"), || t.log_text());
        // 8. Controlled SegmentedControl: refused choice leaves property unchanged; key 9 sets it from Rust.
        t.ui.invoke_choice_selected(s("platform.surface"), 1);
        t.check("refused-choice-keeps-surface", t.ui.get_platform_surface() == 0, || t.log_text());
        t.tap("9");
        t.check("rust-sets-surface-after-refusal", t.ui.get_platform_surface() == 1, || t.log_text());
        // 9. Gallery keys reach the gallery hook; "3" is not taken by the viewport's P/3.
        t.tap("3");
        t.check("gallery-key-3", t.ui.get_platform_outline_kind() == 2, || t.log_text());
        t.tap("5");
        t.check("gallery-key-5", t.ui.get_selection_kind() == 3, || t.log_text());
        // 10. Undo/redo labels are populated for toolbar tooltips.
        apply_state(&t.ui, &t.models, 2);
        t.check(
            "undo-redo-labels",
            t.ui.get_undo_label() == "Undo Change Width"
                && t.ui.get_redo_label() == "Redo Move Mirror M3",
            || format!("undo={:?}, redo={:?}", t.ui.get_undo_label(), t.ui.get_redo_label()),
        );
    }));
    steps
}

fn run_selftest(tester: Rc<Tester>, steps: Rc<Vec<Step>>, index: usize) {
    if index >= steps.len() {
        println!("SELFTEST {} passed, {} failed", tester.passes.get(), tester.failures.get());
        let code = if tester.failures.get() == 0 { 0 } else { 1 };
        let _ = slint::quit_event_loop();
        std::process::exit(code);
    }
    // Each step runs in its own event-loop turn so deferred `changed` handlers (focus
    // moves, focus-lost commits) settle before the next step checks them.
    slint::Timer::single_shot(std::time::Duration::from_millis(120), move || {
        (steps[index])(&tester);
        run_selftest(tester.clone(), steps.clone(), index + 1);
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let mut state = 2u32;
    let mut size: Option<(f32, f32)> = None;
    let mut selftest = false;
    let mut hover_undo = false;
    let mut open_paste = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state" => state = args.next().and_then(|v| v.parse().ok()).unwrap_or(2).clamp(1, 6),
            "--size" => {
                size = args.next().and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            "--selftest" => selftest = true,
            "--hover-undo" => hover_undo = true,
            "--paste" => open_paste = true,
            other => eprintln!("gallery: ignoring unknown argument {other}"),
        }
    }

    let ui = AppWindow::new()?;
    let models = Models::new(&ui);
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    wire(&ui, &models, &log);
    apply_state(&ui, &models, state);
    if open_paste {
        ui.set_paste_target(s("platform"));
    }
    if let Some((w, h)) = size {
        ui.window().set_size(slint::LogicalSize::new(w, h));
    }
    if hover_undo {
        let weak = ui.as_weak();
        let (w, _h) = size.unwrap_or((1440.0, 900.0));
        slint::Timer::single_shot(std::time::Duration::from_millis(250), move || {
            if let Some(ui) = weak.upgrade() {
                let (undo_x, undo_y) = if w < 1200.0 {
                    (368.0, 581.0)
                } else {
                    (531.0, 862.0)
                };
                ui.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(undo_x, undo_y),
                });
            }
        });
    }

    if selftest {
        let tester = Rc::new(Tester {
            ui: ui.clone_strong(),
            models: models.clone(),
            log: log.clone(),
            failures: Cell::new(0),
            passes: Cell::new(0),
        });
        run_selftest(tester, Rc::new(selftest_steps()), 0);
    }
    ui.run()
}
