//! Pure document -> view models and preview generators (M1-SPEC §7.4).
//! No Slint types in this library.

use lab_core::glam::DVec2;
use lab_core::holes::{ExclusionZone, HoleSet, ZoneShape};
use std::collections::HashSet;

use lab_core::model::{Document, Finish, Node, NodeId, Parent, Platform, PlatformKind, Shape, Support, Surface};
use lab_core::outline::Outline;
use lab_core::presets::{PRESETS, PresetGroup};
use lab_core::room::Room;
use lab_core::selection::SelectionTarget;
use lab_core::units::{
    LengthUnit, format_angle_deg, format_length, format_length_with_unit,
};

use lab_catalog::Catalog;
use lab_core::model::Attach;
use lab_core::part::{Geometry, ParamValue};

use crate::edit::{EditError, node_mate, node_yaw};

#[derive(Clone, Debug, PartialEq)]
pub struct SceneRowData {
    pub kind: i32, // 0 node, 1 platform, 2 room
    pub id: i32,
    pub label: String,
    pub detail: String,
    pub depth: i32,
    pub selected: bool,
    pub is_beam: bool,
    pub locked: bool,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditFieldData {
    pub id: String,
    pub label: String,
    pub value: String,
    pub unit: String,
    pub enabled: bool,
    pub invalid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CornerRowData {
    pub index: i32,
    pub x: String,
    pub y: String,
    pub r: String,
    pub invalid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZoneRowData {
    pub index: i32,
    pub name: String,
    pub kind: i32, // 0 rect, 1 circle
    pub x: String,
    pub y: String,
    pub w: String,
    pub h: String,
    pub r: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DoorRowData {
    pub id: i32,
    pub name: String,
    pub wall: i32,
    pub wall_label: String,
    pub offset: String,
    pub width: String,
    pub height: String,
    pub sill: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WallChoiceData {
    pub index: i32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutlinePreviewData {
    pub outline: String,
    pub zones: String,
    pub error_edges: String,
    pub vb_x: f32,
    pub vb_y: f32,
    pub vb_size: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewLabelData {
    pub index: i32,
    pub kind: i32, // 0 corner, 1 wall
    pub x: f32,
    pub y: f32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PresetRowData {
    pub id: String,
    pub group: String,
    pub label: String,
    pub hint: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorFieldData {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatformViewData {
    /// Names of the other platforms (the "On platform" picker).
    pub support_bases: Vec<String>,
    pub name: String,
    pub subtitle: String,
    pub kind: i32,
    pub surface: i32,
    pub outline_kind: i32,
    pub outline_fields: Vec<EditFieldData>,
    pub notch_corner: i32,
    pub corners: Vec<CornerRowData>,
    pub corner_invalid: i32,
    pub preview: OutlinePreviewData,
    pub preview_labels: Vec<PreviewLabelData>,
    pub placement: Vec<EditFieldData>,
    pub grid_standard: i32,
    pub thread: i32,
    pub origin_mode: i32,
    pub grid_fields: Vec<EditFieldData>,
    pub hole_summary: String,
    pub grid_note: String,
    pub zones: Vec<ZoneRowData>,
    pub support: i32,
    pub support_fields: Vec<EditFieldData>,
    pub node_count: i32,
    pub warning: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomViewData {
    pub subtitle: String,
    pub fields: Vec<EditFieldData>,
    pub is_rectangle: bool,
    pub corners: Vec<CornerRowData>,
    pub corner_invalid: i32,
    pub preview: OutlinePreviewData,
    pub preview_labels: Vec<PreviewLabelData>,
    pub doors: Vec<DoorRowData>,
    pub walls: Vec<WallChoiceData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeViewData {
    pub title: String,
    pub detail: String,
    pub fields: Vec<InspectorFieldData>,
    pub edit_fields: Vec<EditFieldData>,
    /// The Set list (M6): empty unless the node has a mated parent or mated children.
    pub set_rows: Vec<SetRowData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetRowData {
    pub node: NodeId,
    /// A member header; otherwise `field` is one of that member's edit fields.
    pub header: bool,
    pub current: bool,
    pub field: EditFieldData,
}

/// Root of the mate chain `id` belongs to.
pub fn set_root(doc: &Document, mut id: NodeId) -> NodeId {
    while let Some(Node { parent: Parent::Node(pid), attach: Attach::Mate { .. }, .. }) = doc.node(id) {
        id = *pid;
    }
    id
}

/// Set rows for the chain root → leaves (breadth-first): a header per member, then its
/// length/height/diameter params, slide and twist. Field ids are `set.<node id>:<node field>`.
fn set_rows(doc: &Document, catalog: &Catalog, node: &Node, units: LengthUnit) -> Vec<SetRowData> {
    let root = set_root(doc, node.id);
    let mut members = vec![root];
    let mut i = 0;
    while i < members.len() {
        let id = members[i];
        members.extend(doc.children(Parent::Node(id)).filter(|n| matches!(n.attach, Attach::Mate { .. })).map(|n| n.id));
        i += 1;
    }
    if members.len() < 2 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for member in members.iter().filter_map(|id| doc.node(*id)) {
        let row = |header: bool, field: EditFieldData| SetRowData { node: member.id, header, current: member.id == node.id, field };
        let field = |name: &str, label: &str, value: String, unit: &str, enabled: bool| EditFieldData {
            id: format!("set.{}:{name}", member.id.0),
            label: label.into(),
            value,
            unit: unit.into(),
            enabled,
            invalid: false,
        };
        rows.push(row(true, field("", &member.name, String::new(), "", true)));
        if let Some(Geometry::Parametric { params, .. }) = catalog.get(&member.part).map(|d| &d.geometry) {
            for name in ["length", "height", "diameter"] {
                if let Some(v) = member.params.get(name).or_else(|| params.get(name)).and_then(ParamValue::as_f64) {
                    let f = field(&format!("node.param.{name}"), &title_case(name), format_length(v, units), units.suffix(), true);
                    rows.push(row(false, f));
                }
            }
        }
        if let Attach::Mate { slide, twist, .. } = &member.attach {
            let ranged = node_mate(doc, catalog, member).is_some_and(|(_, _, (lo, hi))| lo < hi);
            rows.push(row(false, field("node.slide", "Slide", format_length(*slide, units), units.suffix(), ranged)));
            rows.push(row(false, field("node.rotation", "Twist", format_angle_deg(*twist), "°", true)));
        }
    }
    rows
}

/// Builds the scene tree rows: room, platforms (stacked ones under their parent), nodes by
/// `Parent` chain. `collapsed` holds the `(kind, id)` of rows whose children are hidden.
pub fn scene_rows(
    doc: &Document,
    selection: Option<SelectionTarget>,
    collapsed: &HashSet<(i32, i32)>,
) -> Vec<SceneRowData> {
    let mut rows = Vec::new();
    let room_children = doc.children(Parent::Room).count() + doc.platforms.len();
    let room_open = !collapsed.contains(&(2, 0));
    rows.push(SceneRowData {
        kind: 2,
        id: 0,
        label: "Room".into(),
        detail: "".into(),
        depth: 0,
        selected: selection == Some(SelectionTarget::Room),
        is_beam: false,
        locked: false,
        expandable: room_children > 0,
        expanded: room_open,
    });
    if !room_open {
        return rows;
    }
    let cx = SceneCx { doc, selection, collapsed };
    for n in doc.children(Parent::Room) {
        cx.node_rows(n, 1, &mut rows);
    }
    // Roots: platforms not stacked on another existing platform.
    for p in &doc.platforms {
        let stacked = matches!(p.support, Support::OnPlatform { parent, .. } if doc.platform(parent).is_some());
        if !stacked {
            cx.platform_rows(p, 1, &mut rows);
        }
    }
    rows
}

struct SceneCx<'a> {
    doc: &'a Document,
    selection: Option<SelectionTarget>,
    collapsed: &'a HashSet<(i32, i32)>,
}

impl SceneCx<'_> {
    fn platform_rows(&self, p: &Platform, depth: i32, rows: &mut Vec<SceneRowData>) {
        if depth as usize > self.doc.platforms.len() + 1 {
            return; // cycle in unvalidated data
        }
        let stacked = || {
            self.doc.platforms.iter().filter(move |q| matches!(q.support, Support::OnPlatform { parent, .. } if parent == p.id))
        };
        let nodes = || self.doc.children(Parent::Platform(p.id));
        let count = self.doc.nodes_on(p.id).count();
        let open = !self.collapsed.contains(&(1, p.id.0 as i32));
        rows.push(SceneRowData {
            kind: 1,
            id: p.id.0 as i32,
            label: p.name.clone(),
            detail: if count > 0 { count.to_string() } else { "".into() },
            depth,
            selected: self.selection == Some(SelectionTarget::Platform(p.id)),
            is_beam: false,
            locked: false,
            expandable: stacked().next().is_some() || nodes().next().is_some(),
            expanded: open,
        });
        if open {
            for q in stacked() {
                self.platform_rows(q, depth + 1, rows);
            }
            for n in nodes() {
                self.node_rows(n, depth + 1, rows);
            }
        }
    }

    fn node_rows(&self, n: &Node, depth: i32, rows: &mut Vec<SceneRowData>) {
        if depth as usize > self.doc.nodes.len() + self.doc.platforms.len() + 2 {
            return; // cycle in unvalidated data
        }
        let kids = || self.doc.children(Parent::Node(n.id));
        let open = !self.collapsed.contains(&(0, n.id.0 as i32));
        rows.push(SceneRowData {
            kind: 0,
            id: n.id.0 as i32,
            label: n.name.clone(),
            detail: "".into(),
            depth,
            selected: self.selection == Some(SelectionTarget::Node(n.id)),
            is_beam: matches!(n.finish, Finish::Beam { .. }),
            locked: n.locked,
            expandable: kids().next().is_some(),
            expanded: open,
        });
        if open {
            for c in kids() {
                self.node_rows(c, depth + 1, rows);
            }
        }
    }
}

pub fn platform_view(doc: &Document, p: &Platform, holes: &HoleSet, units: LengthUnit) -> PlatformViewData {
    let kind_str = match p.kind {
        PlatformKind::OpticalTable => "Optical table",
        PlatformKind::Breadboard => "Breadboard",
        PlatformKind::Custom => "Custom plate",
    };
    let (lo, hi) = p.outline.bounds();
    let sz = hi - lo;
    let subtitle = format!(
        "{kind_str} · {} × {} {}",
        format_length(sz.x, units),
        format_length(sz.y, units),
        units.suffix()
    );

    let outline_kind = match &p.outline {
        Outline::Rectangle { .. } => 0,
        Outline::RoundedRectangle { .. } => 1,
        Outline::LShape { .. } => 2,
        Outline::Polygon { .. } => 3,
    };

    let mut outline_fields = Vec::new();
    let u_suffix = units.suffix().to_string();
    match &p.outline {
        Outline::Rectangle { size } => {
            outline_fields.push(EditFieldData {
                id: "platform.outline.width".into(),
                label: "Width".into(),
                value: format_length(size.x, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.depth".into(),
                label: "Depth".into(),
                value: format_length(size.y, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
        }
        Outline::RoundedRectangle { size, radius } => {
            outline_fields.push(EditFieldData {
                id: "platform.outline.width".into(),
                label: "Width".into(),
                value: format_length(size.x, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.depth".into(),
                label: "Depth".into(),
                value: format_length(size.y, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.radius".into(),
                label: "Corner radius".into(),
                value: format_length(*radius, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
        }
        Outline::LShape { size, notch, .. } => {
            outline_fields.push(EditFieldData {
                id: "platform.outline.width".into(),
                label: "Width".into(),
                value: format_length(size.x, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.depth".into(),
                label: "Depth".into(),
                value: format_length(size.y, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.notch_width".into(),
                label: "Notch width".into(),
                value: format_length(notch.x, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
            outline_fields.push(EditFieldData {
                id: "platform.outline.notch_depth".into(),
                label: "Notch depth".into(),
                value: format_length(notch.y, units),
                unit: u_suffix.clone(),
                enabled: true,
                invalid: false,
            });
        }
        Outline::Polygon { .. } => {}
    }

    let notch_corner = match &p.outline {
        Outline::LShape { notch_corner, .. } => notch_corner.index() as i32,
        _ => 0,
    };

    let corners = p
        .outline
        .corners()
        .iter()
        .enumerate()
        .map(|(i, c)| CornerRowData {
            index: i as i32,
            x: format_length(c.point.x, units),
            y: format_length(c.point.y, units),
            r: format_length(c.radius, units),
            invalid: false,
        })
        .collect();

    let flat = p.outline_path().map(|f| f.points).unwrap_or_default();
    let empty_zones = Vec::new();
    let zones_slice = p.hole_grid.as_ref().map(|g| g.exclusions.as_slice()).unwrap_or(&empty_zones);
    let preview = outline_preview(&flat, zones_slice, &[]);

    let corner_pts: Vec<DVec2> = p.outline.corners().iter().map(|c| c.point).collect();
    let preview_labels = corner_labels(&corner_pts, &preview);

    let placement = vec![
        EditFieldData {
            id: "platform.x".into(),
            label: "X".into(),
            value: format_length(p.position.x, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        },
        EditFieldData {
            id: "platform.y".into(),
            label: "Y".into(),
            value: format_length(p.position.y, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        },
        EditFieldData {
            id: "platform.rotation".into(),
            label: "Rotation".into(),
            value: format_angle_deg(p.rotation_z),
            unit: "°".into(),
            enabled: true,
            invalid: false,
        },
        EditFieldData {
            id: "platform.top_height".into(),
            label: "Top height".into(),
            value: format_length(p.top_height, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        },
        EditFieldData {
            id: "platform.thickness".into(),
            label: "Thickness".into(),
            value: format_length(p.thickness, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        },
    ];

    let grid_standard = match &p.hole_grid {
        None => 0,
        Some(g) if g.pitch == DVec2::splat(25.0) && g.thread == lab_core::holes::Thread::M6 && (g.margin - 12.5).abs() < 1e-6 => 1,
        Some(g) if (g.pitch.x - lab_core::units::INCH).abs() < 1e-6 && (g.pitch.y - lab_core::units::INCH).abs() < 1e-6 && g.thread == lab_core::holes::Thread::Unc1_4_20 && (g.margin - 0.5 * lab_core::units::INCH).abs() < 1e-6 => 2,
        Some(_) => 3,
    };

    let thread = p.hole_grid.as_ref().map(|g| g.thread.index() as i32).unwrap_or(0);
    let origin_mode = p.hole_grid.as_ref().map(|g| match g.origin {
        lab_core::holes::GridOrigin::Auto => 0,
        lab_core::holes::GridOrigin::At(_) => 1,
    }).unwrap_or(0);

    let mut grid_fields = Vec::new();
    if let Some(g) = &p.hole_grid {
        grid_fields.push(EditFieldData {
            id: "platform.grid.pitch_x".into(),
            label: "Pitch X".into(),
            value: format_length(g.pitch.x, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        });
        grid_fields.push(EditFieldData {
            id: "platform.grid.pitch_y".into(),
            label: "Pitch Y".into(),
            value: format_length(g.pitch.y, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        });
        grid_fields.push(EditFieldData {
            id: "platform.grid.margin".into(),
            label: "Edge margin".into(),
            value: format_length(g.margin, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        });
        let cur_o = match g.origin {
            lab_core::holes::GridOrigin::At(pt) => pt,
            lab_core::holes::GridOrigin::Auto => lab_core::holes::lattice_origin(g, p.outline.bounds()),
        };
        grid_fields.push(EditFieldData {
            id: "platform.grid.origin_x".into(),
            label: "Origin X".into(),
            value: format_length(cur_o.x, units),
            unit: u_suffix.clone(),
            enabled: origin_mode == 1,
            invalid: false,
        });
        grid_fields.push(EditFieldData {
            id: "platform.grid.origin_y".into(),
            label: "Origin Y".into(),
            value: format_length(cur_o.y, units),
            unit: u_suffix.clone(),
            enabled: origin_mode == 1,
            invalid: false,
        });
    }

    let hole_summary = if p.hole_grid.is_some() {
        holes.summary()
    } else {
        "".into()
    };

    let grid_note = if doc.nodes_on(p.id).count() > 0 && p.hole_grid.as_ref().is_some_and(|g| matches!(g.origin, lab_core::holes::GridOrigin::At(_))) {
        "Grid fixed because parts are placed on this platform".into()
    } else {
        "".into()
    };

    let zones = p.hole_grid.as_ref().map(|g| {
        g.exclusions.iter().enumerate().map(|(i, z)| {
            let kind = match z.shape { ZoneShape::Rect { .. } => 0, ZoneShape::Circle { .. } => 1 };
            let (x, y, w, h, r) = match z.shape {
                ZoneShape::Rect { center, size } => (format_length(center.x, units), format_length(center.y, units), format_length(size.x, units), format_length(size.y, units), "".into()),
                ZoneShape::Circle { center, radius } => (format_length(center.x, units), format_length(center.y, units), "".into(), "".into(), format_length(radius, units)),
            };
            ZoneRowData { index: i as i32, name: z.name.clone(), kind, x, y, w, h, r }
        }).collect()
    }).unwrap_or_default();

    let length_field = |id: &str, label: &str, v: f64| EditFieldData {
        id: id.into(),
        label: label.into(),
        value: format_length(v, units),
        unit: u_suffix.clone(),
        enabled: true,
        invalid: false,
    };
    let (support, support_fields) = match p.support {
        Support::Legs { size, inset } => (0, vec![
            length_field("platform.support.leg_size", "Leg size", size),
            length_field("platform.support.leg_inset", "Leg inset", inset),
        ]),
        Support::Isolators { size, inset } => (1, vec![
            length_field("platform.support.leg_size", "Isolator size", size),
            length_field("platform.support.leg_inset", "Isolator inset", inset),
        ]),
        Support::Fixed => (2, Vec::new()),
        Support::OnPlatform { parent, standoff } => (3, vec![
            EditFieldData {
                id: "platform.support.on.name".into(),
                label: "On platform".into(),
                value: doc.platform(parent).map(|b| b.name.clone()).unwrap_or_default(),
                unit: String::new(),
                enabled: true,
                invalid: doc.platform(parent).is_none(),
            },
            length_field("platform.support.standoff", "Standoff", standoff),
        ]),
    };

    let node_count = doc.nodes_on(p.id).count() as i32;
    let warning = outside_warning(doc, p);

    PlatformViewData {
        support_bases: doc.platforms.iter().filter(|b| b.id != p.id).map(|b| b.name.clone()).collect(),
        name: p.name.clone(),
        subtitle,
        kind: match p.kind { PlatformKind::OpticalTable => 0, PlatformKind::Breadboard => 1, PlatformKind::Custom => 2 },
        surface: match p.surface { Surface::Stainless => 0, Surface::BlackAnodized => 1 },
        outline_kind,
        outline_fields,
        notch_corner,
        corners,
        corner_invalid: -1,
        preview,
        preview_labels,
        placement,
        grid_standard,
        thread,
        origin_mode,
        grid_fields,
        hole_summary,
        grid_note,
        zones,
        support,
        support_fields,
        node_count,
        warning,
    }
}

pub fn room_view(doc: &Document, units: LengthUnit) -> RoomViewData {
    let (lo, hi) = lab_geom::bounds(&doc.room.outline);
    let (w_m, d_m) = ((hi.x - lo.x) / 1000.0, (hi.y - lo.y) / 1000.0);
    let area_m2 = lab_geom::signed_area(&doc.room.outline).abs() / 1_000_000.0;
    let doors = doc.room.openings.iter().filter(|o| o.sill <= 1e-6).count();
    let windows = doc.room.openings.iter().filter(|o| o.sill > 1e-6).count();

    let mut subtitle_parts = vec![
        format!("{w_m:.2} × {d_m:.2} m"),
        format!("{area_m2:.1} m²"),
    ];
    if doors == 1 {
        subtitle_parts.push("1 door".into());
    } else if doors > 1 {
        subtitle_parts.push(format!("{doors} doors"));
    }
    if windows == 1 {
        subtitle_parts.push("1 window".into());
    } else if windows > 1 {
        subtitle_parts.push(format!("{windows} windows"));
    }
    let subtitle = subtitle_parts.join(" · ");

    let is_rectangle = doc.room.is_centered_rectangle().is_some();
    let u_suffix = units.suffix().to_string();
    let mut fields = Vec::new();

    if let Some(sz) = doc.room.is_centered_rectangle() {
        fields.push(EditFieldData {
            id: "room.width".into(),
            label: "Width".into(),
            value: format_length(sz.x, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        });
        fields.push(EditFieldData {
            id: "room.depth".into(),
            label: "Depth".into(),
            value: format_length(sz.y, units),
            unit: u_suffix.clone(),
            enabled: true,
            invalid: false,
        });
    }

    fields.push(EditFieldData {
        id: "room.height".into(),
        label: "Height".into(),
        value: format_length(doc.room.wall_height, units),
        unit: u_suffix.clone(),
        enabled: true,
        invalid: false,
    });
    fields.push(EditFieldData {
        id: "room.thickness".into(),
        label: "Wall thickness".into(),
        value: format_length(doc.room.wall_thickness, units),
        unit: u_suffix.clone(),
        enabled: true,
        invalid: false,
    });

    let corners = doc
        .room
        .outline
        .iter()
        .enumerate()
        .map(|(i, pt)| CornerRowData {
            index: i as i32,
            x: format_length(pt.x, units),
            y: format_length(pt.y, units),
            r: "".into(),
            invalid: false,
        })
        .collect();

    let preview = outline_preview(&doc.room.outline, &[], &[]);
    let mut preview_labels = corner_labels(&doc.room.outline, &preview);
    preview_labels.extend(wall_labels(&doc.room, &preview));

    let doors_data = doc
        .room
        .openings
        .iter()
        .map(|o| DoorRowData {
            id: o.id.0 as i32,
            name: o.name.clone(),
            wall: o.wall as i32,
            wall_label: wall_choice_label(&doc.room, o.wall as usize, units),
            offset: format_length(o.offset, units),
            width: format_length(o.width, units),
            height: format_length(o.height, units),
            sill: format_length(o.sill, units),
        })
        .collect();

    let walls_data = (0..doc.room.outline.len())
        .map(|w| WallChoiceData {
            index: w as i32,
            label: wall_choice_label(&doc.room, w, units),
        })
        .collect();

    RoomViewData {
        subtitle,
        fields,
        is_rectangle,
        corners,
        corner_invalid: -1,
        preview,
        preview_labels,
        doors: doors_data,
        walls: walls_data,
    }
}

/// Generator params that are plain counts, not lengths (the rack's `units`).
pub fn is_count_param(name: &str) -> bool {
    name == "units"
}

pub fn node_view(
    doc: &Document,
    catalog: &Catalog,
    node: &Node,
    holes: Option<&HoleSet>,
    units: LengthUnit,
) -> NodeViewData {
    let platform = doc.root_platform(node.id).and_then(|p| doc.platform(p));
    let def = catalog.get(&node.part);
    let attach = match &node.attach {
        Attach::Free => "Free".to_string(),
        Attach::Constrained { constraints } => format!("Constrained ({})", constraints.len()),
        Attach::Hole { hole } => match holes.and_then(|set| set.label(*hole)) {
            Some(label) => format!("Hole {label}"),
            None => "Hole (outside the grid)".to_string(),
        },
        Attach::Mate { slide, .. } => {
            let on = match node.parent {
                Parent::Node(pid) => doc.node(pid).map_or("?".to_string(), |p| {
                    catalog.get(&p.part).map_or(p.name.clone(), |d| {
                        if d.part_number.is_empty() { d.name.clone() } else { d.part_number.clone() }
                    })
                }),
                _ => "?".to_string(),
            };
            format!("On {on} · slide {}", format_length_with_unit(*slide, units))
        }
    };

    let size = match &node.shape {
        Shape::Box { size } => format!(
            "{} × {} × {} {}",
            format_length(size.x, units),
            format_length(size.y, units),
            format_length(size.z, units),
            units.suffix()
        ),
        Shape::Cylinder { radius, length } => format!(
            "Ø{} × {} {}",
            format_length(radius * 2.0, units),
            format_length(*length, units),
            units.suffix()
        ),
        Shape::Part => String::new(),
    };

    let finish = match &node.finish {
        Finish::AnodizedBlack => "Black anodized".to_string(),
        Finish::Stainless => "Stainless steel".to_string(),
        Finish::Mirror => "Mirror coating".to_string(),
        Finish::Glass => "Glass".to_string(),
        Finish::Painted { .. } => "Painted".to_string(),
        Finish::Beam { wavelength_nm } => format!("Beam, {wavelength_nm:.0} nm"),
    };

    let mut fields = Vec::new();
    if let Some(def) = def {
        let number = if def.part_number.is_empty() { def.id.clone() } else { def.part_number.clone() };
        fields.push(InspectorFieldData { label: "Part number".into(), value: number });
    }
    fields.push(InspectorFieldData { label: "Platform".into(), value: platform.map_or("—".to_string(), |p| p.name.clone()) });
    fields.push(InspectorFieldData { label: "Attach".into(), value: attach });
    if !size.is_empty() {
        fields.push(InspectorFieldData { label: "Size".into(), value: size });
    }
    fields.push(InspectorFieldData { label: "Finish".into(), value: finish });

    let mated = matches!(node.attach, Attach::Mate { .. });
    let length = |id: &str, label: &str, value: f64, enabled: bool| EditFieldData {
        id: id.into(),
        label: label.into(),
        value: format_length(value, units),
        unit: units.suffix().into(),
        enabled,
        invalid: false,
    };
    let rotation = match &node.attach {
        Attach::Mate { twist, .. } => *twist,
        _ => node_yaw(node),
    };
    let mut edit_fields = vec![
        EditFieldData {
            id: "node.note".into(),
            label: "Note".into(),
            value: node.note.clone(),
            unit: String::new(),
            enabled: true,
            invalid: false,
        },
        length("node.x", "X", node.position.x, !mated),
        length("node.y", "Y", node.position.y, !mated),
        length("node.z", "Height", node.position.z, !mated),
        EditFieldData {
            id: "node.rotation".into(),
            label: "Rotation".into(),
            value: format_angle_deg(rotation),
            unit: "°".into(),
            enabled: true,
            invalid: false,
        },
    ];
    if let Attach::Mate { slide, .. } = &node.attach {
        let ranged = node_mate(doc, catalog, node).is_some_and(|(_, _, (lo, hi))| lo < hi);
        edit_fields.push(length("node.slide", "Slide", *slide, ranged));
    }
    if let Some(Geometry::Parametric { generator, params }) = def.map(|d| &d.geometry)
        && let Some(info) = lab_parts::generators().iter().find(|g| g.name == generator)
    {
        for (name, default) in info.params {
            let value = node.params.get(*name).or_else(|| params.get(*name)).and_then(ParamValue::as_f64).unwrap_or(*default);
            let counted = is_count_param(name);
            edit_fields.push(EditFieldData {
                id: format!("node.param.{name}"),
                label: title_case(name),
                value: if counted { format!("{value}") } else { lab_core::units::format_length(value, units) },
                unit: if counted { String::new() } else { units.suffix().into() },
                enabled: true,
                invalid: false,
            });
        }
    }

    NodeViewData {
        title: node.name.clone(),
        detail: def.map_or(node.part.clone(), |d| d.category.clone()),
        fields,
        edit_fields,
        set_rows: set_rows(doc, catalog, node, units),
    }
}

/// "base_diameter" -> "Base Diameter".
fn title_case(name: &str) -> String {
    name.split('_')
        .map(|w| w.chars().next().map_or(String::new(), |c| c.to_uppercase().chain(w.chars().skip(1)).collect()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// SVG "M x y L … Z" with y negated, "{:.2}" numbers; zones as extra sub-paths; square viewbox = bbox padded 8% (≥ 10 mm).
pub fn outline_preview(flat: &[DVec2], zones: &[ExclusionZone], error_edges: &[(DVec2, DVec2)]) -> OutlinePreviewData {
    let mut x0 = f64::MAX;
    let mut x1 = f64::MIN;
    let mut y0 = f64::MAX;
    let mut y1 = f64::MIN;

    for &p in flat {
        x0 = x0.min(p.x);
        x1 = x1.max(p.x);
        y0 = y0.min(-p.y);
        y1 = y1.max(-p.y);
    }

    let outline = if flat.is_empty() {
        String::new()
    } else {
        let mut out = format!("M {:.2} {:.2}", flat[0].x, -flat[0].y);
        for p in &flat[1..] {
            out.push_str(&format!(" L {:.2} {:.2}", p.x, -p.y));
        }
        out.push_str(" Z");
        out
    };

    let mut zones_path = String::new();
    for z in zones {
        match z.shape {
            ZoneShape::Rect { center, size } => {
                let h = size / 2.0;
                let c = [
                    DVec2::new(center.x - h.x, center.y - h.y),
                    DVec2::new(center.x + h.x, center.y - h.y),
                    DVec2::new(center.x + h.x, center.y + h.y),
                    DVec2::new(center.x - h.x, center.y + h.y),
                ];
                zones_path.push_str(&format!(
                    "M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} Z ",
                    c[0].x, -c[0].y, c[1].x, -c[1].y, c[2].x, -c[2].y, c[3].x, -c[3].y
                ));
            }
            ZoneShape::Circle { center, radius } => {
                let cx = center.x;
                let cy = -center.y;
                let r = radius;
                zones_path.push_str(&format!(
                    "M {:.2} {:.2} A {:.2} {:.2} 0 1 0 {:.2} {:.2} A {:.2} {:.2} 0 1 0 {:.2} {:.2} Z ",
                    cx - r, cy, r, r, cx + r, cy, r, r, cx - r, cy
                ));
            }
        }
    }

    let mut err_path = String::new();
    for &(a, b) in error_edges {
        err_path.push_str(&format!("M {:.2} {:.2} L {:.2} {:.2} ", a.x, -a.y, b.x, -b.y));
    }

    let extent = if x1 >= x0 && y1 >= y0 {
        (x1 - x0).max(y1 - y0)
    } else {
        100.0
    };
    let pad = (0.08 * extent).max(10.0);
    let size = extent + 2.0 * pad;
    let cx = if x1 >= x0 { (x0 + x1) / 2.0 } else { 0.0 };
    let cy = if y1 >= y0 { (y0 + y1) / 2.0 } else { 0.0 };

    OutlinePreviewData {
        outline,
        zones: zones_path.trim_end().to_string(),
        error_edges: err_path.trim_end().to_string(),
        vb_x: (cx - size / 2.0) as f32,
        vb_y: (cy - size / 2.0) as f32,
        vb_size: size as f32,
    }
}

/// 1-based numbers at the corners (viewbox coords, y negated).
pub fn corner_labels(corners: &[DVec2], _vb: &OutlinePreviewData) -> Vec<PreviewLabelData> {
    corners
        .iter()
        .enumerate()
        .map(|(i, &pt)| PreviewLabelData {
            index: i as i32,
            kind: 0,
            x: pt.x as f32,
            y: -pt.y as f32,
            text: format!("{}", i + 1),
        })
        .collect()
}

/// Wall numbers at midpoints, offset outward by 4% of vb.
pub fn wall_labels(room: &Room, vb: &OutlinePreviewData) -> Vec<PreviewLabelData> {
    let n = room.outline.len();
    if n < 3 {
        return Vec::new();
    }
    let ccw = lab_geom::signed_area(&room.outline) > 0.0;
    let offset = 0.04 * (vb.vb_size as f64);

    (0..n)
        .map(|i| {
            let a = room.outline[i];
            let b = room.outline[(i + 1) % n];
            let d = b - a;
            let len = d.length().max(1e-9);
            // Outward normal in doc frame (+Y North)
            let normal = if ccw {
                DVec2::new(d.y / len, -d.x / len)
            } else {
                DVec2::new(-d.y / len, d.x / len)
            };
            let mid = (a + b) / 2.0 + normal * offset;
            PreviewLabelData {
                index: i as i32,
                kind: 1,
                x: mid.x as f32,
                y: -mid.y as f32,
                text: format!("{}", i + 1),
            }
        })
        .collect()
}

/// "Wall 2 · East · 5.00 m" (in units for inches)
pub fn wall_choice_label(room: &Room, wall: usize, units: LengthUnit) -> String {
    let len = room.wall_length(wall);
    let compass = room.wall_compass(wall);
    let len_str = match units {
        LengthUnit::Millimetre => format!("{:.2} m", len / 1000.0),
        LengthUnit::Inch => format_length_with_unit(len, LengthUnit::Inch),
    };
    format!("Wall {} · {compass} · {len_str}", wall + 1)
}

/// Current-unit groups first.
pub fn preset_rows(units: LengthUnit) -> Vec<PresetRowData> {
    let mut metric_rows = Vec::new();
    let mut imperial_rows = Vec::new();
    let mut custom_rows = Vec::new();

    for p in PRESETS.iter() {
        let group_str = match p.group {
            PresetGroup::TableMetric => "Optical tables (metric)",
            PresetGroup::TableImperial => "Optical tables (imperial)",
            PresetGroup::BreadboardMetric => "Breadboards (metric)",
            PresetGroup::BreadboardImperial => "Breadboards (imperial)",
            PresetGroup::SteelBreadboardMetric => "Steel breadboards (metric)",
            PresetGroup::SteelBreadboardImperial => "Steel breadboards (imperial)",
            PresetGroup::Custom => "Custom",
        };
        let row = PresetRowData {
            id: p.id.to_string(),
            group: group_str.to_string(),
            label: p.label.to_string(),
            hint: p.hint.to_string(),
        };
        let raised = p.id == "bb-m-300x300";
        match p.group {
            PresetGroup::TableMetric | PresetGroup::BreadboardMetric | PresetGroup::SteelBreadboardMetric => {
                metric_rows.push(row);
                if raised {
                    metric_rows.push(PresetRowData {
                        id: "raised-bb".into(),
                        group: group_str.into(),
                        label: "Raised breadboard".into(),
                        hint: "300 × 300 mm on 100 mm standoffs, on the selected platform".into(),
                    });
                }
            }
            PresetGroup::TableImperial | PresetGroup::BreadboardImperial | PresetGroup::SteelBreadboardImperial => {
                imperial_rows.push(row);
            }
            PresetGroup::Custom => {
                custom_rows.push(row);
            }
        }
    }

    match units {
        LengthUnit::Millimetre => {
            let mut out = metric_rows;
            out.extend(imperial_rows);
            out.extend(custom_rows);
            out
        }
        LengthUnit::Inch => {
            let mut out = imperial_rows;
            out.extend(metric_rows);
            out.extend(custom_rows);
            out
        }
    }
}

/// "{name} — LabSetup" / "{name} — Edited — LabSetup"
pub fn window_title(name: &str, dirty: bool) -> String {
    if dirty {
        format!("{name} — Edited — LabSetup")
    } else {
        format!("{name} — LabSetup")
    }
}

pub fn project_detail(doc: &Document) -> String {
    let count = |kind| doc.platforms.iter().filter(|p| p.kind == kind).count();
    let platform_str = [
        (count(PlatformKind::OpticalTable), "table", "tables"),
        (count(PlatformKind::Breadboard), "breadboard", "breadboards"),
        (count(PlatformKind::Custom), "custom platform", "custom platforms"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
    .collect::<Vec<_>>()
    .join(", ");
    let platform_str = if platform_str.is_empty() { "0 tables".to_string() } else { platform_str };

    let part_count = doc.nodes.len();
    let part_str = if part_count == 1 {
        "1 part".to_string()
    } else {
        format!("{part_count} parts")
    };

    format!("{platform_str} · {part_str} · offline")
}

pub fn outside_warning(doc: &Document, p: &Platform) -> String {
    let points = p.outline_path().map(|f| f.points).unwrap_or_default();
    let outside_count = doc
        .nodes_on(p.id)
        .filter(|n| !lab_geom::point_in_polygon(&points, n.position.truncate()))
        .count();

    if outside_count == 0 {
        "".into()
    } else if outside_count == 1 {
        "1 part lies outside the outline".into()
    } else {
        format!("{outside_count} parts lie outside the outline")
    }
}

pub fn invalid_row(err: &EditError) -> Option<usize> {
    let EditError::Invalid(msg) = err else { return None; };

    if let Some(pos) = msg.find("edges ") {
        let rest = &msg[pos + 6..];
        if let Some(end) = rest.find(' ') {
            return rest[..end].parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        }
    }

    for prefix in &["Room corner ", "room corner ", "Corner ", "corner ", "edge ", "Edge ", "Wall ", "wall "] {
        if let Some(pos) = msg.find(prefix) {
            let rest = &msg[pos + prefix.len()..];
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num_str.parse::<usize>() {
                return n.checked_sub(1);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::glam::{DQuat, DVec3};
    use lab_core::model::{NodeId, PlatformId, Surface};
    use lab_core::outline::RectCorner;

    fn test_doc() -> Document {
        let mut doc = Document::new("My Project");
        doc.platforms.push(Platform {
            id: PlatformId(1),
            name: "Table 1".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(lab_core::holes::HoleGrid::metric()),
            support: Support::Fixed,
        });
        doc
    }

    // V1: Subtitle, summary and preview for a rectangle and an L.
    #[test]
    fn v1_subtitle_summary_and_preview() {
        let doc = test_doc();
        let p_rect = &doc.platforms[0];
        let holes = lab_core::holes::compute_holes(p_rect).unwrap();
        let view_rect = platform_view(&doc, p_rect, &holes, LengthUnit::Millimetre);

        assert_eq!(view_rect.subtitle, "Optical table · 1200 × 900 mm");
        assert!(view_rect.hole_summary.contains("holes"));
        assert!(!view_rect.preview.outline.is_empty());

        let mut p_l = p_rect.clone();
        p_l.outline = Outline::LShape {
            size: DVec2::new(1200.0, 900.0),
            notch: DVec2::new(400.0, 300.0),
            notch_corner: RectCorner::XPosYPos,
        };
        let holes_l = lab_core::holes::compute_holes(&p_l).unwrap();
        let view_l = platform_view(&doc, &p_l, &holes_l, LengthUnit::Millimetre);
        assert!(!view_l.preview.outline.is_empty());
    }

    // V2: SVG y negation.
    #[test]
    fn v2_svg_y_negation() {
        let points = vec![DVec2::new(10.0, 20.0), DVec2::new(30.0, 40.0)];
        let prev = outline_preview(&points, &[], &[]);
        assert!(prev.outline.contains("-20.00"));
        assert!(prev.outline.contains("-40.00"));
    }

    // V3: Scene rows: Room first, nodes nested.
    #[test]
    fn v3_scene_rows() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            note: String::new(),
            id: NodeId(10),
            name: "Laser · head".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: Shape::Box { size: DVec3::ONE },
            finish: Finish::Stainless,
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let rows = scene_rows(&doc, None, &HashSet::new());
        assert_eq!(rows[0].kind, 2); // Room first
        assert_eq!(rows[0].label, "Room");
        assert_eq!(rows[1].kind, 1); // Platform next
        assert_eq!(rows[1].label, "Table 1");
        assert_eq!(rows[2].kind, 0); // Nested node
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].depth, 2);
    }

    // V4: wall_choice_label compass.
    #[test]
    fn v4_wall_choice_label() {
        let doc = test_doc();
        let label = wall_choice_label(&doc.room, 0, LengthUnit::Millimetre);
        assert!(label.starts_with("Wall 1 · "));
        assert!(label.contains(" · 6.00 m"));

        let label_in = wall_choice_label(&doc.room, 0, LengthUnit::Inch);
        assert!(label_in.contains("in"));
    }

    // V5: window_title.
    #[test]
    fn v5_window_title() {
        assert_eq!(window_title("Project", false), "Project — LabSetup");
        assert_eq!(window_title("Project", true), "Project — Edited — LabSetup");
    }
}
