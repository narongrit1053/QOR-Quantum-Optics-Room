//! Persistent Slint list models and the `fill_*` functions copying view data into Slint (D16).
//!
//! Every list property gets one `Rc<VecModel>` at startup that is never replaced. Updates go
//! through [`sync`], which only touches rows that changed, so a focused field inside a list row
//! (a corner cell, a door name) keeps its focus and the scene list keeps its scroll position.

use std::rc::Rc;

use lab_app::{
    CornerRowData, DoorRowData, EditFieldData, InspectorFieldData, NodeViewData, SetRowData, OutlinePreviewData,
    PlatformViewData, PresetRowData, PreviewLabelData, RoomViewData, SceneRowData, WallChoiceData, ZoneRowData,
};
use lab_ui::{
    AppWindow, CornerRow, DoorRow, EditField, InspectorField, LibraryRow, OutlinePreview, PresetRow, PreviewLabel,
    SceneRow, SetRow, WallChoice, ZoneRow,
};
use slint::{Model, ModelRc, SharedString, VecModel};

#[derive(Default)]
pub struct UiModels {
    pub scene: Rc<VecModel<SceneRow>>,
    pub inspector: Rc<VecModel<InspectorField>>,
    pub node_fields: Rc<VecModel<EditField>>,
    pub set_rows: Rc<VecModel<SetRow>>,
    pub platform_outline_fields: Rc<VecModel<EditField>>,
    pub platform_corners: Rc<VecModel<CornerRow>>,
    pub platform_preview_labels: Rc<VecModel<PreviewLabel>>,
    pub platform_placement: Rc<VecModel<EditField>>,
    pub platform_grid_fields: Rc<VecModel<EditField>>,
    pub platform_zones: Rc<VecModel<ZoneRow>>,
    pub platform_support_fields: Rc<VecModel<EditField>>,
    pub presets: Rc<VecModel<PresetRow>>,
    pub room_fields: Rc<VecModel<EditField>>,
    pub room_corners: Rc<VecModel<CornerRow>>,
    pub room_preview_labels: Rc<VecModel<PreviewLabel>>,
    pub room_doors: Rc<VecModel<DoorRow>>,
    pub room_walls: Rc<VecModel<WallChoice>>,
    pub sheet_buttons: Rc<VecModel<SharedString>>,
    pub library: Rc<VecModel<LibraryRow>>,
}

impl UiModels {
    /// Creates the models and hands each to its AppWindow property (once, at startup).
    pub fn install(ui: &AppWindow) -> Rc<Self> {
        let models = Rc::new(Self::default());
        let m = &models;
        ui.set_scene(ModelRc::from(m.scene.clone()));
        ui.set_inspector(ModelRc::from(m.inspector.clone()));
        ui.set_node_fields(ModelRc::from(m.node_fields.clone()));
        ui.set_set_rows(ModelRc::from(m.set_rows.clone()));
        ui.set_platform_outline_fields(ModelRc::from(m.platform_outline_fields.clone()));
        ui.set_platform_corners(ModelRc::from(m.platform_corners.clone()));
        ui.set_platform_preview_labels(ModelRc::from(m.platform_preview_labels.clone()));
        ui.set_platform_placement(ModelRc::from(m.platform_placement.clone()));
        ui.set_platform_grid_fields(ModelRc::from(m.platform_grid_fields.clone()));
        ui.set_platform_zones(ModelRc::from(m.platform_zones.clone()));
        ui.set_platform_support_fields(ModelRc::from(m.platform_support_fields.clone()));
        ui.set_presets(ModelRc::from(m.presets.clone()));
        ui.set_room_fields(ModelRc::from(m.room_fields.clone()));
        ui.set_room_corners(ModelRc::from(m.room_corners.clone()));
        ui.set_room_preview_labels(ModelRc::from(m.room_preview_labels.clone()));
        ui.set_room_doors(ModelRc::from(m.room_doors.clone()));
        ui.set_room_walls(ModelRc::from(m.room_walls.clone()));
        ui.set_sheet_buttons(ModelRc::from(m.sheet_buttons.clone()));
        ui.set_library(ModelRc::from(m.library.clone()));
        models
    }
}

/// Makes `model` equal to `rows`, touching only rows that differ.
pub fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: impl IntoIterator<Item = T>) {
    let mut count = 0;
    for (i, row) in rows.into_iter().enumerate() {
        if i < model.row_count() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        } else {
            model.push(row);
        }
        count = i + 1;
    }
    while model.row_count() > count {
        model.remove(model.row_count() - 1);
    }
}

/// The persistent `VecModel` behind a list property set by [`UiModels::install`].
pub fn vec_model<T: Clone + 'static>(model: &ModelRc<T>) -> Option<&VecModel<T>> {
    model.as_any().downcast_ref::<VecModel<T>>()
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

pub fn scene_row(r: SceneRowData) -> SceneRow {
    SceneRow {
        kind: r.kind,
        id: r.id,
        label: s(r.label),
        detail: s(r.detail),
        depth: r.depth,
        selected: r.selected,
        is_beam: r.is_beam,
        locked: r.locked,
        expandable: r.expandable,
        expanded: r.expanded,
    }
}

pub fn edit_field(f: EditFieldData) -> EditField {
    EditField { id: s(f.id), label: s(f.label), value: s(f.value), unit: s(f.unit), enabled: f.enabled, invalid: f.invalid }
}

pub fn corner_row(c: CornerRowData) -> CornerRow {
    CornerRow { index: c.index, x: s(c.x), y: s(c.y), r: s(c.r), invalid: c.invalid }
}

pub fn zone_row(z: ZoneRowData) -> ZoneRow {
    ZoneRow { index: z.index, name: s(z.name), kind: z.kind, x: s(z.x), y: s(z.y), w: s(z.w), h: s(z.h), r: s(z.r) }
}

pub fn door_row(d: DoorRowData) -> DoorRow {
    DoorRow {
        id: d.id,
        name: s(d.name),
        wall: d.wall,
        wall_label: s(d.wall_label),
        offset: s(d.offset),
        width: s(d.width),
        height: s(d.height),
        sill: s(d.sill),
    }
}

pub fn wall_choice(w: WallChoiceData) -> WallChoice {
    WallChoice { index: w.index, label: s(w.label) }
}

pub fn outline_preview(p: OutlinePreviewData) -> OutlinePreview {
    OutlinePreview {
        outline: s(p.outline),
        zones: s(p.zones),
        error_edges: s(p.error_edges),
        vb_x: p.vb_x,
        vb_y: p.vb_y,
        vb_size: p.vb_size,
    }
}

pub fn preview_label(l: PreviewLabelData) -> PreviewLabel {
    PreviewLabel { index: l.index, kind: l.kind, x: l.x, y: l.y, text: s(l.text) }
}

pub fn preset_row(p: PresetRowData) -> PresetRow {
    PresetRow { id: s(p.id), group: s(p.group), label: s(p.label), hint: s(p.hint) }
}

pub fn inspector_field(f: InspectorFieldData) -> InspectorField {
    InspectorField { label: s(f.label), value: s(f.value) }
}

pub fn fill_scene(models: &UiModels, rows: Vec<SceneRowData>) {
    sync(&models.scene, rows.into_iter().map(scene_row));
}

pub fn fill_presets(models: &UiModels, rows: Vec<PresetRowData>) {
    sync(&models.presets, rows.into_iter().map(preset_row));
}

pub fn fill_node(ui: &AppWindow, models: &UiModels, v: NodeViewData) {
    ui.set_selection_title(s(v.title));
    ui.set_selection_detail(s(v.detail));
    sync(&models.inspector, v.fields.into_iter().map(inspector_field));
    sync(&models.node_fields, v.edit_fields.into_iter().map(edit_field));
    sync(&models.set_rows, v.set_rows.into_iter().map(set_row));
}

fn set_row(r: SetRowData) -> SetRow {
    SetRow { node: r.node.0 as i32, header: r.header, current: r.current, field: edit_field(r.field) }
}

pub fn fill_platform(ui: &AppWindow, models: &UiModels, v: PlatformViewData) {
    ui.set_platform_name(s(v.name));
    ui.set_platform_subtitle(s(v.subtitle));
    ui.set_platform_kind(v.kind);
    ui.set_platform_surface(v.surface);
    ui.set_platform_outline_kind(v.outline_kind);
    sync(&models.platform_outline_fields, v.outline_fields.into_iter().map(edit_field));
    ui.set_platform_notch_corner(v.notch_corner);
    sync(&models.platform_corners, v.corners.into_iter().map(corner_row));
    ui.set_platform_corner_invalid(v.corner_invalid);
    fill_platform_preview(ui, models, v.preview, v.preview_labels);
    sync(&models.platform_placement, v.placement.into_iter().map(edit_field));
    ui.set_platform_grid_standard(v.grid_standard);
    ui.set_platform_thread(v.thread);
    ui.set_platform_origin_mode(v.origin_mode);
    sync(&models.platform_grid_fields, v.grid_fields.into_iter().map(edit_field));
    ui.set_platform_hole_summary(s(v.hole_summary));
    ui.set_platform_grid_note(s(v.grid_note));
    sync(&models.platform_zones, v.zones.into_iter().map(zone_row));
    ui.set_platform_support(v.support);
    ui.set_platform_support_bases(ModelRc::from(Rc::new(VecModel::from(v.support_bases.into_iter().map(SharedString::from).collect::<Vec<_>>()))));
    sync(&models.platform_support_fields, v.support_fields.into_iter().map(edit_field));
    ui.set_platform_node_count(v.node_count);
    ui.set_platform_warning(s(v.warning));
}

pub fn fill_platform_preview(ui: &AppWindow, models: &UiModels, preview: OutlinePreviewData, labels: Vec<PreviewLabelData>) {
    ui.set_platform_preview(outline_preview(preview));
    sync(&models.platform_preview_labels, labels.into_iter().map(preview_label));
}

pub fn fill_room(ui: &AppWindow, models: &UiModels, v: RoomViewData) {
    ui.set_room_subtitle(s(v.subtitle));
    sync(&models.room_fields, v.fields.into_iter().map(edit_field));
    ui.set_room_is_rectangle(v.is_rectangle);
    sync(&models.room_corners, v.corners.into_iter().map(corner_row));
    ui.set_room_corner_invalid(v.corner_invalid);
    fill_room_preview(ui, models, v.preview, v.preview_labels);
    // Walls before doors: a door row's wall dropdown indexes into the wall list.
    sync(&models.room_walls, v.walls.into_iter().map(wall_choice));
    sync(&models.room_doors, v.doors.into_iter().map(door_row));
}

pub fn fill_room_preview(ui: &AppWindow, models: &UiModels, preview: OutlinePreviewData, labels: Vec<PreviewLabelData>) {
    ui.set_room_preview(outline_preview(preview));
    sync(&models.room_preview_labels, labels.into_iter().map(preview_label));
}
