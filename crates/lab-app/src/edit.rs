//! Pure edit pipeline: UI field commits, choice selects and actions (M1-SPEC §7.3).

use lab_core::glam::DVec2;
use lab_core::holes::{GridOrigin, HoleGrid, Thread, ZoneShape, lattice_origin};
use lab_core::limits::OPENING_MIN_JAMB;
use lab_core::model::{Document, OpeningId, Platform, PlatformId, PlatformKind, Support, Surface};
use lab_core::outline::{Outline, OutlineCorner, RectCorner};
use lab_core::prefs::WallDisplay;
use lab_core::presets::{PRESETS, apply_preset, platform_from_preset};
use lab_core::room::{Opening, OpeningFix, Room};
use lab_core::selection::SelectionTarget;
use lab_core::units::{
    LengthUnit, ParseError, format_angle_deg, format_length, parse_angle_deg, parse_length,
};
use lab_core::Command;
use lab_core::glam::{DQuat, DVec3, EulerRot};
use lab_core::model::{Attach, Node, Parent};
use lab_core::model::NodeId;
use lab_core::part::{Frame, Geometry, ParamValue, Params};
use lab_catalog::Catalog;

pub struct EditContext<'a> {
    pub doc: &'a Document,
    pub selection: Option<SelectionTarget>,
    pub units: LengthUnit,
}

#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum EditOutcome {
    Nothing, // no-op (unchanged text, same value)
    Command {
        command: Command,
        label: String,
        select: Option<SelectionTarget>,
        fixes: Vec<OpeningFix>,
    },
    Preference(PreferenceChange), // units / walls
    OpenPaste(String /* "platform" | "room" */),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceChange {
    Units(LengthUnit),
    Walls(WallDisplay),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    Parse(ParseError),
    Invalid(String),
    Stale,
    UnknownField(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Parse(e) => write!(f, "{e}"),
            EditError::Invalid(s) => write!(f, "{s}"),
            EditError::Stale => write!(f, "That item no longer exists."),
            EditError::UnknownField(s) => write!(f, "Unknown field {s}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<ParseError> for EditError {
    fn from(e: ParseError) -> Self {
        EditError::Parse(e)
    }
}

pub enum PreviewTarget {
    Platform(Platform),
    Room(Room),
}

fn platform_field_val(p: &Platform, field: &str, units: LengthUnit) -> Option<String> {
    if field == "platform.name" {
        return Some(p.name.clone());
    }
    if field == "platform.x" {
        return Some(format_length(p.position.x, units));
    }
    if field == "platform.y" {
        return Some(format_length(p.position.y, units));
    }
    if field == "platform.rotation" {
        return Some(format_angle_deg(p.rotation_z));
    }
    if field == "platform.top_height" {
        return Some(format_length(p.top_height, units));
    }
    if field == "platform.thickness" {
        return Some(format_length(p.thickness, units));
    }

    if let Some(rest) = field.strip_prefix("platform.outline.") {
        return match rest {
            "width" => match &p.outline {
                Outline::Rectangle { size } | Outline::RoundedRectangle { size, .. } | Outline::LShape { size, .. } => {
                    Some(format_length(size.x, units))
                }
                Outline::Polygon { .. } => None,
            },
            "depth" => match &p.outline {
                Outline::Rectangle { size } | Outline::RoundedRectangle { size, .. } | Outline::LShape { size, .. } => {
                    Some(format_length(size.y, units))
                }
                Outline::Polygon { .. } => None,
            },
            "radius" => match &p.outline {
                Outline::RoundedRectangle { radius, .. } => Some(format_length(*radius, units)),
                _ => None,
            },
            "notch_width" => match &p.outline {
                Outline::LShape { notch, .. } => Some(format_length(notch.x, units)),
                _ => None,
            },
            "notch_depth" => match &p.outline {
                Outline::LShape { notch, .. } => Some(format_length(notch.y, units)),
                _ => None,
            },
            _ => None,
        };
    }

    if let Some(rest) = field.strip_prefix("platform.corner.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next()?.parse().ok()?;
        let axis = parts.next()?;
        if let Outline::Polygon { corners } = &p.outline {
            let c = corners.get(k)?;
            return match axis {
                "x" => Some(format_length(c.point.x, units)),
                "y" => Some(format_length(c.point.y, units)),
                "r" => Some(format_length(c.radius, units)),
                _ => None,
            };
        }
        return None;
    }

    if let Some(rest) = field.strip_prefix("platform.grid.") {
        let g = p.hole_grid.as_ref()?;
        return match rest {
            "pitch_x" => Some(format_length(g.pitch.x, units)),
            "pitch_y" => Some(format_length(g.pitch.y, units)),
            "margin" => Some(format_length(g.margin, units)),
            "origin_x" => match g.origin {
                GridOrigin::At(pt) => Some(format_length(pt.x, units)),
                GridOrigin::Auto => Some(format_length(lattice_origin(g, p.outline.bounds()).x, units)),
            },
            "origin_y" => match g.origin {
                GridOrigin::At(pt) => Some(format_length(pt.y, units)),
                GridOrigin::Auto => Some(format_length(lattice_origin(g, p.outline.bounds()).y, units)),
            },
            _ => None,
        };
    }

    if let Some(rest) = field.strip_prefix("platform.zone.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next()?.parse().ok()?;
        let prop = parts.next()?;
        let g = p.hole_grid.as_ref()?;
        let z = g.exclusions.get(k)?;
        return match prop {
            "name" => Some(z.name.clone()),
            "x" => match z.shape {
                ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => Some(format_length(center.x, units)),
            },
            "y" => match z.shape {
                ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => Some(format_length(center.y, units)),
            },
            "w" => match z.shape {
                ZoneShape::Rect { size, .. } => Some(format_length(size.x, units)),
                ZoneShape::Circle { .. } => None,
            },
            "h" => match z.shape {
                ZoneShape::Rect { size, .. } => Some(format_length(size.y, units)),
                ZoneShape::Circle { .. } => None,
            },
            "r" => match z.shape {
                ZoneShape::Circle { radius, .. } => Some(format_length(radius, units)),
                ZoneShape::Rect { .. } => None,
            },
            _ => None,
        };
    }

    if let Some(rest) = field.strip_prefix("platform.support.") {
        return match (rest, &p.support) {
            ("leg_size", Support::Legs { size, .. } | Support::Isolators { size, .. }) => Some(format_length(*size, units)),
            ("leg_inset", Support::Legs { inset, .. } | Support::Isolators { inset, .. }) => {
                Some(format_length(*inset, units))
            }
            ("standoff", Support::OnPlatform { standoff, .. }) => Some(format_length(*standoff, units)),
            _ => None,
        };
    }

    None
}

fn room_field_val(r: &Room, field: &str, units: LengthUnit) -> Option<String> {
    if field == "room.width" {
        return r.is_centered_rectangle().map(|sz| format_length(sz.x, units));
    }
    if field == "room.depth" {
        return r.is_centered_rectangle().map(|sz| format_length(sz.y, units));
    }
    if field == "room.height" {
        return Some(format_length(r.wall_height, units));
    }
    if field == "room.thickness" {
        return Some(format_length(r.wall_thickness, units));
    }

    if let Some(rest) = field.strip_prefix("room.corner.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next()?.parse().ok()?;
        let axis = parts.next()?;
        let pt = r.outline.get(k)?;
        return match axis {
            "x" => Some(format_length(pt.x, units)),
            "y" => Some(format_length(pt.y, units)),
            _ => None,
        };
    }

    if let Some(rest) = field.strip_prefix("room.door.") {
        let mut parts = rest.split('.');
        let id: u64 = parts.next()?.parse().ok()?;
        let prop = parts.next()?;
        let o = r.openings.iter().find(|o| o.id.0 == id)?;
        return match prop {
            "name" => Some(o.name.clone()),
            "offset" => Some(format_length(o.offset, units)),
            "width" => Some(format_length(o.width, units)),
            "height" => Some(format_length(o.height, units)),
            "sill" => Some(format_length(o.sill, units)),
            _ => None,
        };
    }

    None
}

/// Formatted current value of a field id in `units` (the source of truth for the no-op rule and for views).
pub fn field_value(doc: &Document, field: &str, units: LengthUnit) -> Option<String> {
    if field.starts_with("platform.") {
        doc.platforms.first().and_then(|p| platform_field_val(p, field, units))
    } else if field.starts_with("room.") {
        room_field_val(&doc.room, field, units)
    } else {
        None
    }
}

fn target_platform<'a>(cx: &'a EditContext) -> Result<&'a Platform, EditError> {
    match cx.selection {
        Some(SelectionTarget::Platform(pid)) => cx.doc.platform(pid).ok_or(EditError::Stale),
        Some(SelectionTarget::Node(_)) | Some(SelectionTarget::Room) => Err(EditError::Stale),
        None => cx.doc.platforms.first().ok_or(EditError::Stale),
    }
}

fn target_field_value(cx: &EditContext, field: &str) -> Option<String> {
    if field.starts_with("platform.") {
        let p = target_platform(cx).ok()?;
        platform_field_val(p, field, cx.units)
    } else if field.starts_with("room.") {
        room_field_val(&cx.doc.room, field, cx.units)
    } else {
        None
    }
}

fn is_length_field(field: &str) -> bool {
    !field.ends_with(".name") && field != "platform.rotation"
}

fn pin_grid_if_needed(
    doc: &Document,
    p: &mut Platform,
    old_grid: &Option<HoleGrid>,
    old_bounds: (DVec2, DVec2),
) {
    if doc.nodes_on(p.id).count() == 0 {
        return;
    }
    let Some(og) = old_grid.as_ref() else {
        return;
    };
    let pinned_origin = match og.origin {
        GridOrigin::Auto => lattice_origin(og, old_bounds),
        GridOrigin::At(pt) => pt,
    };
    if let Some(g) = p.hole_grid.as_mut() {
        g.origin = GridOrigin::At(pinned_origin);
    }
}

pub fn field_edit(cx: &EditContext, field: &str, text: &str) -> Result<EditOutcome, EditError> {
    let trimmed = text.trim();

    // 1. No-op rule (mandatory)
    if let Some(cur) = target_field_value(cx, field) {
        if trimmed == cur || trimmed == format!("{cur} {}", cx.units.suffix()) {
            return Ok(EditOutcome::Nothing);
        }
        // Also check if numeric parse equals the same formatted value (length fields only)
        if is_length_field(field) && parse_length(trimmed, cx.units).is_ok_and(|len| format_length(len, cx.units) == cur) {
            return Ok(EditOutcome::Nothing);
        }
    }

    if field.starts_with("platform.") {
        let orig = target_platform(cx)?;
        let old_grid = orig.hole_grid.clone();
        let old_bounds = orig.outline.bounds();
        let mut p = orig.clone();

        let mut outline_or_grid_changed = false;

        if field == "platform.name" {
            p.name = trimmed.to_string();
        } else if field == "platform.x" {
            p.position.x = parse_length(trimmed, cx.units)?;
        } else if field == "platform.y" {
            p.position.y = parse_length(trimmed, cx.units)?;
        } else if field == "platform.rotation" {
            p.rotation_z = parse_angle_deg(trimmed)?;
        } else if field == "platform.top_height" {
            p.top_height = parse_length(trimmed, cx.units)?;
        } else if field == "platform.thickness" {
            p.thickness = parse_length(trimmed, cx.units)?;
        } else if let Some(rest) = field.strip_prefix("platform.outline.") {
            outline_or_grid_changed = true;
            match rest {
                "width" => {
                    let w = parse_length(trimmed, cx.units)?;
                    match &mut p.outline {
                        Outline::Rectangle { size } | Outline::RoundedRectangle { size, .. } | Outline::LShape { size, .. } => {
                            size.x = w;
                        }
                        Outline::Polygon { .. } => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "depth" => {
                    let d = parse_length(trimmed, cx.units)?;
                    match &mut p.outline {
                        Outline::Rectangle { size } | Outline::RoundedRectangle { size, .. } | Outline::LShape { size, .. } => {
                            size.y = d;
                        }
                        Outline::Polygon { .. } => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "radius" => {
                    let r = parse_length(trimmed, cx.units)?;
                    match &mut p.outline {
                        Outline::RoundedRectangle { radius, .. } => *radius = r,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "notch_width" => {
                    let nw = parse_length(trimmed, cx.units)?;
                    match &mut p.outline {
                        Outline::LShape { notch, .. } => notch.x = nw,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "notch_depth" => {
                    let nd = parse_length(trimmed, cx.units)?;
                    match &mut p.outline {
                        Outline::LShape { notch, .. } => notch.y = nd,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                _ => return Err(EditError::UnknownField(field.into())),
            }
        } else if let Some(rest) = field.strip_prefix("platform.corner.") {
            outline_or_grid_changed = true;
            let mut parts = rest.split('.');
            let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?.parse().map_err(|_| EditError::UnknownField(field.into()))?;
            let axis = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?;
            match &mut p.outline {
                Outline::Polygon { corners } => {
                    if k >= corners.len() {
                        return Err(EditError::Stale);
                    }
                    let val = parse_length(trimmed, cx.units)?;
                    match axis {
                        "x" => corners[k].point.x = val,
                        "y" => corners[k].point.y = val,
                        "r" => corners[k].radius = val,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                _ => return Err(EditError::UnknownField(field.into())),
            }
        } else if let Some(rest) = field.strip_prefix("platform.grid.") {
            let g = p.hole_grid.as_mut().ok_or(EditError::Stale)?;
            match rest {
                "pitch_x" => {
                    g.pitch.x = parse_length(trimmed, cx.units)?;
                    outline_or_grid_changed = true;
                }
                "pitch_y" => {
                    g.pitch.y = parse_length(trimmed, cx.units)?;
                    outline_or_grid_changed = true;
                }
                "margin" => {
                    g.margin = parse_length(trimmed, cx.units)?;
                    outline_or_grid_changed = true;
                }
                "origin_x" => {
                    let ox = parse_length(trimmed, cx.units)?;
                    let cur_y = match g.origin {
                        GridOrigin::At(pt) => pt.y,
                        GridOrigin::Auto => lattice_origin(g, p.outline.bounds()).y,
                    };
                    g.origin = GridOrigin::At(DVec2::new(ox, cur_y));
                }
                "origin_y" => {
                    let oy = parse_length(trimmed, cx.units)?;
                    let cur_x = match g.origin {
                        GridOrigin::At(pt) => pt.x,
                        GridOrigin::Auto => lattice_origin(g, p.outline.bounds()).x,
                    };
                    g.origin = GridOrigin::At(DVec2::new(cur_x, oy));
                }
                _ => return Err(EditError::UnknownField(field.into())),
            }
        } else if let Some(rest) = field.strip_prefix("platform.zone.") {
            let mut parts = rest.split('.');
            let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?.parse().map_err(|_| EditError::UnknownField(field.into()))?;
            let prop = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?;
            let g = p.hole_grid.as_mut().ok_or(EditError::Stale)?;
            if k >= g.exclusions.len() {
                return Err(EditError::Stale);
            }
            let z = &mut g.exclusions[k];
            match prop {
                "name" => z.name = trimmed.to_string(),
                "x" => {
                    let x = parse_length(trimmed, cx.units)?;
                    match &mut z.shape {
                        ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => center.x = x,
                    }
                }
                "y" => {
                    let y = parse_length(trimmed, cx.units)?;
                    match &mut z.shape {
                        ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => center.y = y,
                    }
                }
                "w" => {
                    let w = parse_length(trimmed, cx.units)?;
                    match &mut z.shape {
                        ZoneShape::Rect { size, .. } => size.x = w,
                        ZoneShape::Circle { .. } => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "h" => {
                    let h = parse_length(trimmed, cx.units)?;
                    match &mut z.shape {
                        ZoneShape::Rect { size, .. } => size.y = h,
                        ZoneShape::Circle { .. } => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "r" => {
                    let r = parse_length(trimmed, cx.units)?;
                    match &mut z.shape {
                        ZoneShape::Circle { radius, .. } => *radius = r,
                        ZoneShape::Rect { .. } => return Err(EditError::UnknownField(field.into())),
                    }
                }
                _ => return Err(EditError::UnknownField(field.into())),
            }
        } else if let Some(rest) = field.strip_prefix("platform.support.") {
            match rest {
                "leg_size" => {
                    let sz = parse_length(trimmed, cx.units)?;
                    match &mut p.support {
                        Support::Legs { size, .. } | Support::Isolators { size, .. } => *size = sz,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "leg_inset" => {
                    let inset_val = parse_length(trimmed, cx.units)?;
                    match &mut p.support {
                        Support::Legs { inset, .. } | Support::Isolators { inset, .. } => *inset = inset_val,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "standoff" => {
                    let h = parse_length(trimmed, cx.units)?;
                    match &mut p.support {
                        Support::OnPlatform { standoff, .. } => *standoff = h,
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                "on.name" => {
                    let base = cx
                        .doc
                        .platforms
                        .iter()
                        .find(|b| b.id != p.id && b.name == trimmed)
                        .ok_or_else(|| EditError::Invalid(format!("No other platform is named “{trimmed}”")))?;
                    match &mut p.support {
                        Support::OnPlatform { parent, .. } if *parent == base.id => return Ok(EditOutcome::Nothing),
                        Support::OnPlatform { parent, .. } => {
                            *parent = base.id;
                            p.position = DVec2::ZERO;
                            p.rotation_z = 0.0;
                        }
                        _ => return Err(EditError::UnknownField(field.into())),
                    }
                }
                _ => return Err(EditError::UnknownField(field.into())),
            }
        } else {
            return Err(EditError::UnknownField(field.into()));
        }

        // 4. Pinning (D13)
        if outline_or_grid_changed {
            pin_grid_if_needed(cx.doc, &mut p, &old_grid, old_bounds);
        }

        // 3. Validate
        p.validate().map_err(|e| EditError::Invalid(e.to_string()))?;

        Ok(EditOutcome::Command {
            command: Command::UpdatePlatform { platform: Box::new(p) },
            label: label_for(field),
            select: cx.selection,
            fixes: Vec::new(),
        })
    } else if field.starts_with("room.") {
        // Room shape edits
        if field == "room.width" {
            let new_w = parse_length(trimmed, cx.units)?;
            let sz = cx.doc.room.is_centered_rectangle().ok_or(EditError::Stale)?;
            let (new_room, fixes) = cx.doc.room.with_rectangle(DVec2::new(new_w, sz.y)).map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes,
            })
        } else if field == "room.depth" {
            let new_d = parse_length(trimmed, cx.units)?;
            let sz = cx.doc.room.is_centered_rectangle().ok_or(EditError::Stale)?;
            let (new_room, fixes) = cx.doc.room.with_rectangle(DVec2::new(sz.x, new_d)).map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes,
            })
        } else if field == "room.height" {
            let new_h = parse_length(trimmed, cx.units)?;
            let (new_room, fixes) = cx.doc.room.with_dimensions(new_h, cx.doc.room.wall_thickness).map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes,
            })
        } else if field == "room.thickness" {
            let new_t = parse_length(trimmed, cx.units)?;
            let (new_room, fixes) = cx.doc.room.with_dimensions(cx.doc.room.wall_height, new_t).map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes,
            })
        } else if let Some(rest) = field.strip_prefix("room.corner.") {
            let mut parts = rest.split('.');
            let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?.parse().map_err(|_| EditError::UnknownField(field.into()))?;
            let axis = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?;
            if k >= cx.doc.room.outline.len() {
                return Err(EditError::Stale);
            }
            let cur_pt = cx.doc.room.outline[k];
            let val = parse_length(trimmed, cx.units)?;
            let new_pt = match axis {
                "x" => DVec2::new(val, cur_pt.y),
                "y" => DVec2::new(cur_pt.x, val),
                _ => return Err(EditError::UnknownField(field.into())),
            };
            let (new_room, fixes) = cx.doc.room.with_corner_moved(k, new_pt).map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes,
            })
        } else if let Some(rest) = field.strip_prefix("room.door.") {
            // Direct door-field edits do not refit: they validate and fail with the message.
            let mut parts = rest.split('.');
            let id: u64 = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?.parse().map_err(|_| EditError::UnknownField(field.into()))?;
            let prop = parts.next().ok_or_else(|| EditError::UnknownField(field.into()))?;
            let mut new_room = cx.doc.room.clone();
            let o = new_room.openings.iter_mut().find(|o| o.id.0 == id).ok_or(EditError::Stale)?;
            match prop {
                "name" => o.name = trimmed.to_string(),
                "offset" => o.offset = parse_length(trimmed, cx.units)?,
                "width" => o.width = parse_length(trimmed, cx.units)?,
                "height" => o.height = parse_length(trimmed, cx.units)?,
                "sill" => o.sill = parse_length(trimmed, cx.units)?,
                _ => return Err(EditError::UnknownField(field.into())),
            }
            new_room.validate().map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(field),
                select: cx.selection,
                fixes: Vec::new(),
            })
        } else {
            Err(EditError::UnknownField(field.into()))
        }
    } else {
        Err(EditError::UnknownField(field.into()))
    }
}

pub fn choice_edit(cx: &EditContext, choice: &str, index: i32) -> Result<EditOutcome, EditError> {
    if choice == "units" {
        let u = match index {
            0 => LengthUnit::Millimetre,
            1 => LengthUnit::Inch,
            _ => return Err(EditError::Invalid("Invalid units choice".into())),
        };
        return Ok(EditOutcome::Preference(PreferenceChange::Units(u)));
    }
    if choice == "walls" {
        let w = match index {
            0 => WallDisplay::Auto,
            1 => WallDisplay::Always,
            2 => WallDisplay::Hidden,
            _ => return Err(EditError::Invalid("Invalid walls choice".into())),
        };
        return Ok(EditOutcome::Preference(PreferenceChange::Walls(w)));
    }

    if choice.starts_with("platform.") {
        let orig = target_platform(cx)?;
        let old_grid = orig.hole_grid.clone();
        let old_bounds = orig.outline.bounds();
        let mut p = orig.clone();
        let mut outline_or_grid_changed = false;

        match choice {
            "platform.kind" => {
                p.kind = match index {
                    0 => PlatformKind::OpticalTable,
                    1 => PlatformKind::Breadboard,
                    2 => PlatformKind::Custom,
                    _ => return Err(EditError::Invalid("Invalid platform kind".into())),
                };
            }
            "platform.surface" => {
                p.surface = match index {
                    0 => Surface::Stainless,
                    1 => Surface::BlackAnodized,
                    _ => return Err(EditError::Invalid("Invalid surface".into())),
                };
            }
            "platform.outline.kind" => {
                outline_or_grid_changed = true;
                let (lo, hi) = orig.outline.bounds();
                let bbox_size = hi - lo;
                let w = bbox_size.x;
                let d = bbox_size.y;
                let hx = w / 2.0;
                let hy = d / 2.0;

                p.outline = match index {
                    0 => Outline::Rectangle { size: bbox_size },
                    1 => {
                        let rad = (25.0_f64).min(hx).min(hy);
                        Outline::RoundedRectangle { size: bbox_size, radius: rad }
                    }
                    2 => {
                        let nw = (w / 3.0).round();
                        let nd = (d / 3.0).round();
                        Outline::LShape {
                            size: bbox_size,
                            notch: DVec2::new(nw, nd),
                            notch_corner: RectCorner::XPosYPos,
                        }
                    }
                    3 => orig.outline.to_polygon(),
                    _ => return Err(EditError::Invalid("Invalid outline kind".into())),
                };
            }
            "platform.outline.notch_corner" => {
                let corner = RectCorner::from_index(index as usize).ok_or_else(|| EditError::Invalid("Invalid corner".into()))?;
                match &mut p.outline {
                    Outline::LShape { notch_corner, .. } => *notch_corner = corner,
                    _ => return Err(EditError::Invalid("Not an L-shape".into())),
                }
            }
            "platform.grid.standard" => {
                outline_or_grid_changed = true;
                match index {
                    0 => p.hole_grid = None,
                    1 => {
                        let mut g = HoleGrid::metric();
                        if let Some(old) = &orig.hole_grid {
                            g.exclusions = old.exclusions.clone();
                        }
                        p.hole_grid = Some(g);
                    }
                    2 => {
                        let mut g = HoleGrid::imperial();
                        if let Some(old) = &orig.hole_grid {
                            g.exclusions = old.exclusions.clone();
                        }
                        p.hole_grid = Some(g);
                    }
                    3 => {
                        if p.hole_grid.is_none() {
                            p.hole_grid = Some(HoleGrid::metric());
                        }
                    }
                    _ => return Err(EditError::Invalid("Invalid grid standard".into())),
                }
            }
            "platform.grid.thread" => {
                let th = Thread::from_index(index as usize).ok_or_else(|| EditError::Invalid("Invalid thread".into()))?;
                let g = p.hole_grid.as_mut().ok_or(EditError::Stale)?;
                g.thread = th;
                outline_or_grid_changed = true;
            }
            "platform.grid.origin" => {
                let g = p.hole_grid.as_mut().ok_or(EditError::Stale)?;
                match index {
                    0 => g.origin = GridOrigin::Auto,
                    1 => {
                        let cur_o = lattice_origin(g, p.outline.bounds());
                        g.origin = GridOrigin::At(cur_o);
                    }
                    _ => return Err(EditError::Invalid("Invalid origin mode".into())),
                }
            }
            "platform.support" => {
                p.support = match index {
                    0 => Support::Legs { size: 120.0, inset: 150.0 },
                    1 => Support::Isolators { size: 120.0, inset: 150.0 },
                    2 => Support::Fixed,
                    3 => {
                        if matches!(p.support, Support::OnPlatform { .. }) {
                            return Ok(EditOutcome::Nothing);
                        }
                        // The largest other platform (by area) is the likeliest base; centred on it.
                        let base = cx
                            .doc
                            .platforms
                            .iter()
                            .filter(|b| b.id != p.id)
                            .max_by(|a, b| {
                                let area = |p: &Platform| {
                                    let (lo, hi) = p.outline.bounds();
                                    (hi.x - lo.x) * (hi.y - lo.y)
                                };
                                area(a).total_cmp(&area(b))
                            })
                            .ok_or_else(|| EditError::Invalid("There is no other platform to stand on".into()))?;
                        p.position = DVec2::ZERO;
                        p.rotation_z = 0.0;
                        Support::OnPlatform { parent: base.id, standoff: 100.0 }
                    }
                    _ => return Err(EditError::Invalid("Invalid support".into())),
                };
            }
            _ => return Err(EditError::UnknownField(choice.into())),
        }

        // Pinning rule
        if outline_or_grid_changed {
            pin_grid_if_needed(cx.doc, &mut p, &old_grid, old_bounds);
        }

        p.validate().map_err(|e| EditError::Invalid(e.to_string()))?;

        let label = match choice {
            "platform.outline.kind" => match index {
                0 => "Change Outline to Rectangle".to_string(),
                1 => "Change Outline to Rounded Rectangle".to_string(),
                2 => "Change Outline to L-shape".to_string(),
                _ => "Change Outline to Custom Polygon".to_string(),
            },
            "platform.grid.standard" => match index {
                0 => "Remove Hole Grid".to_string(),
                1 => "Use Metric Holes".to_string(),
                2 => "Use Imperial Holes".to_string(),
                _ => "Use Custom Holes".to_string(),
            },
            _ => label_for(choice),
        };

        Ok(EditOutcome::Command {
            command: Command::UpdatePlatform { platform: Box::new(p) },
            label,
            select: cx.selection,
            fixes: Vec::new(),
        })
    } else if let Some(rest) = choice.strip_prefix("room.door.") {
        let mut parts = rest.split('.');
        let id: u64 = parts.next().ok_or_else(|| EditError::UnknownField(choice.into()))?.parse().map_err(|_| EditError::UnknownField(choice.into()))?;
        let prop = parts.next().ok_or_else(|| EditError::UnknownField(choice.into()))?;
        if prop == "wall" {
            let mut new_room = cx.doc.room.clone();
            let o = new_room.openings.iter_mut().find(|o| o.id.0 == id).ok_or(EditError::Stale)?;
            o.wall = index as u32;
            new_room.validate().map_err(|e| EditError::Invalid(e.to_string()))?;
            Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(choice),
                select: cx.selection,
                fixes: Vec::new(),
            })
        } else {
            Err(EditError::UnknownField(choice.into()))
        }
    } else {
        Err(EditError::UnknownField(choice.into()))
    }
}

struct WallGap {
    wall: usize,
    wall_len: f64,
    start: f64,
    #[allow(dead_code)]
    end: f64,
    len: f64,
}

fn find_best_wall_gap(doc: &Document, req_width: f64) -> Option<(usize, f64)> {
    let n = doc.room.outline.len();
    let mut candidate_gaps: Vec<WallGap> = Vec::new();

    for w in 0..n {
        let wall_len = doc.room.wall_length(w);
        let (min_lim, max_lim) = doc.room.opening_limits(w);
        if max_lim.is_nan() || max_lim <= min_lim {
            continue;
        }

        let mut wall_openings: Vec<&Opening> = doc.room.openings.iter().filter(|o| o.wall == w as u32).collect();
        wall_openings.sort_by(|a, b| a.offset.total_cmp(&b.offset));

        let mut gaps = Vec::new();
        if wall_openings.is_empty() {
            let gap_len = max_lim - min_lim;
            if gap_len >= req_width - 1e-6 {
                gaps.push((min_lim, max_lim, gap_len));
            }
        } else {
            // Gap before first opening
            let first_start = min_lim;
            let first_end = wall_openings[0].offset - OPENING_MIN_JAMB;
            if first_end - first_start >= req_width - 1e-6 {
                gaps.push((first_start, first_end, first_end - first_start));
            }

            // Gaps between openings
            for pair in wall_openings.windows(2) {
                let mid_start = pair[0].offset + pair[0].width + OPENING_MIN_JAMB;
                let mid_end = pair[1].offset - OPENING_MIN_JAMB;
                if mid_end - mid_start >= req_width - 1e-6 {
                    gaps.push((mid_start, mid_end, mid_end - mid_start));
                }
            }

            // Gap after last opening
            let last_o = wall_openings.last().unwrap();
            let last_start = last_o.offset + last_o.width + OPENING_MIN_JAMB;
            let last_end = max_lim;
            if last_end - last_start >= req_width - 1e-6 {
                gaps.push((last_start, last_end, last_end - last_start));
            }
        }

        // Pick largest gap on this wall
        if let Some(&(start, end, len)) = gaps.iter().max_by(|a, b| a.2.total_cmp(&b.2)) {
            candidate_gaps.push(WallGap {
                wall: w,
                wall_len,
                start,
                end,
                len,
            });
        }
    }

    // Pick candidate with longest wall_len, break ties by largest gap len, then smaller wall index
    candidate_gaps.sort_by(|a, b| {
        b.wall_len
            .total_cmp(&a.wall_len)
            .then_with(|| b.len.total_cmp(&a.len))
            .then_with(|| a.wall.cmp(&b.wall))
    });

    candidate_gaps.first().map(|g| {
        let offset = g.start + (g.len - req_width) / 2.0;
        (g.wall, offset)
    })
}

pub fn action_edit(cx: &EditContext, action: &str) -> Result<EditOutcome, EditError> {
    if action == "selection.delete" {
        return match cx.selection {
            Some(SelectionTarget::Node(nid)) => Ok(EditOutcome::Command {
                command: Command::RemoveNodes(vec![nid]),
                label: "Delete".into(),
                select: None,
                fixes: Vec::new(),
            }),
            Some(SelectionTarget::Platform(pid)) => Ok(EditOutcome::Command {
                command: Command::RemovePlatform(pid),
                label: "Delete Platform".into(),
                select: None,
                fixes: Vec::new(),
            }),
            Some(SelectionTarget::Room) | None => Ok(EditOutcome::Nothing),
        };
    }
    if action == "platform.delete" {
        let pid = match cx.selection {
            Some(SelectionTarget::Platform(pid)) => pid,
            _ => {
                if let Some(first) = cx.doc.platforms.first() {
                    first.id
                } else {
                    return Err(EditError::Stale);
                }
            }
        };
        return Ok(EditOutcome::Command {
            command: Command::RemovePlatform(pid),
            label: "Delete Platform".into(),
            select: None,
            fixes: Vec::new(),
        });
    }

    if action == "platform.corners.paste" {
        return Ok(EditOutcome::OpenPaste("platform".into()));
    }
    if action == "room.corners.paste" {
        return Ok(EditOutcome::OpenPaste("room".into()));
    }

    if action == "platform.add.raised-bb" {
        let base = match cx.selection {
            Some(SelectionTarget::Platform(pid)) => cx.doc.platform(pid),
            _ => None,
        }
        .ok_or_else(|| EditError::Invalid("Select the platform the breadboard should stand on".into()))?;
        let preset = PRESETS.iter().find(|p| p.id == "bb-m-300x300").ok_or(EditError::Stale)?;
        let new_id = PlatformId(cx.doc.next_free_id());
        let mut p = platform_from_preset(cx.doc, preset, new_id, Some(base.id));
        p.position = DVec2::ZERO;
        p.rotation_z = 0.0;
        p.support = Support::OnPlatform { parent: base.id, standoff: 100.0 };
        return Ok(EditOutcome::Command {
            command: Command::AddPlatform { platform: Box::new(p), index: None },
            label: "Add Raised breadboard".into(),
            select: Some(SelectionTarget::Platform(new_id)),
            fixes: Vec::new(),
        });
    }

    if let Some(preset_id) = action.strip_prefix("platform.add.") {
        let preset = PRESETS.iter().find(|p| p.id == preset_id).ok_or_else(|| EditError::Invalid(format!("Unknown preset {preset_id}")))?;
        let new_id = PlatformId(cx.doc.next_free_id());
        let on = match cx.selection {
            Some(SelectionTarget::Platform(pid)) => Some(pid),
            _ => None,
        };
        let p = platform_from_preset(cx.doc, preset, new_id, on);
        return Ok(EditOutcome::Command {
            command: Command::AddPlatform { platform: Box::new(p), index: None },
            label: format!("Add {}", preset.label),
            select: Some(SelectionTarget::Platform(new_id)),
            fixes: Vec::new(),
        });
    }

    if let Some(preset_id) = action.strip_prefix("platform.preset.") {
        let preset = PRESETS.iter().find(|p| p.id == preset_id).ok_or_else(|| EditError::Invalid(format!("Unknown preset {preset_id}")))?;
        let orig = target_platform(cx)?;
        let mut p = apply_preset(orig, preset);
        pin_grid_if_needed(cx.doc, &mut p, &orig.hole_grid, orig.outline.bounds());
        p.validate().map_err(|e| EditError::Invalid(e.to_string()))?;
        return Ok(EditOutcome::Command {
            command: Command::UpdatePlatform { platform: Box::new(p) },
            label: format!("Apply {}", preset.label),
            select: cx.selection,
            fixes: Vec::new(),
        });
    }

    if action.starts_with("platform.") {
        let orig = target_platform(cx)?;
        let old_grid = orig.hole_grid.clone();
        let old_bounds = orig.outline.bounds();
        let mut p = orig.clone();
        let mut outline_or_grid_changed = false;

        if let Some(rest) = action.strip_prefix("platform.corner.") {
            outline_or_grid_changed = true;
            if rest == "append" {
                let n = match &p.outline {
                    Outline::Polygon { corners } => corners.len(),
                    _ => return Err(EditError::Invalid("Only custom outlines have editable corners".into())),
                };
                if n == 0 {
                    return Err(EditError::Stale);
                }
                p.outline = p.outline.with_corner_inserted(n - 1).map_err(|e| EditError::Invalid(e.to_string()))?;
            } else {
                let mut parts = rest.split('.');
                let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?.parse().map_err(|_| EditError::UnknownField(action.into()))?;
                let op = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?;
                match op {
                    "insert" => {
                        p.outline = p.outline.with_corner_inserted(k).map_err(|e| EditError::Invalid(e.to_string()))?;
                    }
                    "delete" => {
                        p.outline = p.outline.with_corner_removed(k).map_err(|e| EditError::Invalid(e.to_string()))?;
                    }
                    _ => return Err(EditError::UnknownField(action.into())),
                }
            }
        } else if let Some(rest) = action.strip_prefix("platform.zone.") {
            let (lo, hi) = p.outline.bounds();
            let center = (lo + hi) / 2.0;
            let g = p.hole_grid.as_mut().ok_or_else(|| EditError::Invalid("No hole grid on platform".into()))?;

            if rest == "add_rect" {
                let name = format!("Zone {}", g.exclusions.len() + 1);
                g.exclusions.push(lab_core::holes::ExclusionZone {
                    name,
                    shape: ZoneShape::Rect { center, size: DVec2::splat(100.0) },
                });
            } else if rest == "add_circle" {
                let name = format!("Zone {}", g.exclusions.len() + 1);
                g.exclusions.push(lab_core::holes::ExclusionZone {
                    name,
                    shape: ZoneShape::Circle { center, radius: 50.0 },
                });
            } else {
                let mut parts = rest.split('.');
                let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?.parse().map_err(|_| EditError::UnknownField(action.into()))?;
                let op = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?;
                if op == "delete" {
                    if k >= g.exclusions.len() {
                        return Err(EditError::Stale);
                    }
                    g.exclusions.remove(k);
                } else {
                    return Err(EditError::UnknownField(action.into()));
                }
            }
        } else {
            return Err(EditError::UnknownField(action.into()));
        }

        if outline_or_grid_changed {
            pin_grid_if_needed(cx.doc, &mut p, &old_grid, old_bounds);
        }

        p.validate().map_err(|e| EditError::Invalid(e.to_string()))?;

        return Ok(EditOutcome::Command {
            command: Command::UpdatePlatform { platform: Box::new(p) },
            label: label_for(action),
            select: cx.selection,
            fixes: Vec::new(),
        });
    }

    if action.starts_with("room.") {
        if action == "room.make_l" {
            let (new_room, fixes) = cx.doc.room.with_l_shape().map_err(|e| EditError::Invalid(e.to_string()))?;
            return Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(action),
                select: cx.selection,
                fixes,
            });
        }
        if action == "room.make_rect" {
            let (lo, hi) = lab_geom::bounds(&cx.doc.room.outline);
            let (new_room, fixes) = cx.doc.room.with_rectangle(hi - lo).map_err(|e| EditError::Invalid(e.to_string()))?;
            return Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: label_for(action),
                select: cx.selection,
                fixes,
            });
        }
        if let Some(rest) = action.strip_prefix("room.corner.") {
            if rest == "append" {
                let n = cx.doc.room.outline.len();
                let (new_room, fixes) = cx.doc.room.with_corner_inserted(n - 1).map_err(|e| EditError::Invalid(e.to_string()))?;
                return Ok(EditOutcome::Command {
                    command: Command::SetRoom(Box::new(new_room)),
                    label: label_for(action),
                    select: cx.selection,
                    fixes,
                });
            } else {
                let mut parts = rest.split('.');
                let k: usize = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?.parse().map_err(|_| EditError::UnknownField(action.into()))?;
                let op = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?;
                let (new_room, fixes) = match op {
                    "insert" => cx.doc.room.with_corner_inserted(k).map_err(|e| EditError::Invalid(e.to_string()))?,
                    "delete" => cx.doc.room.with_corner_removed(k).map_err(|e| EditError::Invalid(e.to_string()))?,
                    _ => return Err(EditError::UnknownField(action.into())),
                };
                return Ok(EditOutcome::Command {
                    command: Command::SetRoom(Box::new(new_room)),
                    label: label_for(action),
                    select: cx.selection,
                    fixes,
                });
            }
        }
        if action == "room.door.add" {
            let Some((wall, offset)) = find_best_wall_gap(cx.doc, 1000.0) else {
                return Err(EditError::Invalid("No wall is long enough for a 1 m door".into()));
            };
            let mut n = 1;
            while cx.doc.room.openings.iter().any(|o| o.name == format!("Door {n}")) {
                n += 1;
            }
            let name = format!("Door {n}");
            let height = 2100.0f64.min(cx.doc.room.wall_height - 100.0);
            let next_id = cx.doc.next_free_id().max(cx.doc.room.openings.iter().map(|o| o.id.0 + 1).max().unwrap_or(1));
            let opening = Opening {
                id: OpeningId(next_id),
                name,
                wall: wall as u32,
                offset,
                width: 1000.0,
                height,
                sill: 0.0,
            };
            let mut new_room = cx.doc.room.clone();
            new_room.openings.push(opening);
            new_room.validate().map_err(|e| EditError::Invalid(e.to_string()))?;
            return Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: "Add Door".into(),
                select: cx.selection,
                fixes: Vec::new(),
            });
        }
        if action == "room.window.add" {
            let Some((wall, offset)) = find_best_wall_gap(cx.doc, 1200.0) else {
                return Err(EditError::Invalid("No wall is long enough for a 1.2 m window".into()));
            };
            let mut n = 1;
            while cx.doc.room.openings.iter().any(|o| o.name == format!("Window {n}")) {
                n += 1;
            }
            let name = format!("Window {n}");
            let next_id = cx.doc.next_free_id().max(cx.doc.room.openings.iter().map(|o| o.id.0 + 1).max().unwrap_or(1));
            let opening = Opening {
                id: OpeningId(next_id),
                name,
                wall: wall as u32,
                offset,
                width: 1200.0,
                height: 1200.0,
                sill: 900.0,
            };
            let mut new_room = cx.doc.room.clone();
            new_room.openings.push(opening);
            new_room.validate().map_err(|e| EditError::Invalid(e.to_string()))?;
            return Ok(EditOutcome::Command {
                command: Command::SetRoom(Box::new(new_room)),
                label: "Add Window".into(),
                select: cx.selection,
                fixes: Vec::new(),
            });
        }
        if let Some(rest) = action.strip_prefix("room.door.") {
            let mut parts = rest.split('.');
            let id: u64 = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?.parse().map_err(|_| EditError::UnknownField(action.into()))?;
            let op = parts.next().ok_or_else(|| EditError::UnknownField(action.into()))?;
            if op == "delete" {
                let mut new_room = cx.doc.room.clone();
                let idx = new_room.openings.iter().position(|o| o.id.0 == id).ok_or(EditError::Stale)?;
                let is_window = new_room.openings[idx].sill > 1e-6 || new_room.openings[idx].name.starts_with("Window");
                let label = if is_window { "Remove Window" } else { "Remove Door" };
                new_room.openings.remove(idx);
                return Ok(EditOutcome::Command {
                    command: Command::SetRoom(Box::new(new_room)),
                    label: label.into(),
                    select: cx.selection,
                    fixes: Vec::new(),
                });
            }
        }
    }

    Err(EditError::UnknownField(action.into()))
}

/// Live preview of a corner/zone cell: the platform or room with that one cell changed, or the validation error and the row to tint.
pub fn preview_edit(cx: &EditContext, field: &str, text: &str) -> Result<PreviewTarget, (EditError, Option<usize>)> {
    if let Some(rest) = field.strip_prefix("platform.corner.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next().and_then(|s| s.parse().ok()).ok_or((EditError::UnknownField(field.into()), None))?;
        let axis = parts.next().ok_or((EditError::UnknownField(field.into()), Some(k)))?;
        let pid = match cx.selection {
            Some(SelectionTarget::Platform(pid)) => pid,
            Some(SelectionTarget::Node(_)) | Some(SelectionTarget::Room) => return Err((EditError::Stale, Some(k))),
            None => cx.doc.platforms.first().map(|p| p.id).ok_or((EditError::Stale, Some(k)))?,
        };
        let orig = cx.doc.platform(pid).ok_or((EditError::Stale, Some(k)))?;
        let mut p = orig.clone();
        let val = parse_length(text.trim(), cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
        match &mut p.outline {
            Outline::Polygon { corners } => {
                if k >= corners.len() {
                    return Err((EditError::Stale, Some(k)));
                }
                match axis {
                    "x" => corners[k].point.x = val,
                    "y" => corners[k].point.y = val,
                    "r" => corners[k].radius = val,
                    _ => return Err((EditError::UnknownField(field.into()), Some(k))),
                }
            }
            _ => return Err((EditError::Invalid("Only custom outlines have editable corners".into()), Some(k))),
        }
        p.validate().map_err(|e| (EditError::Invalid(e.to_string()), Some(k)))?;
        return Ok(PreviewTarget::Platform(p));
    }

    if let Some(rest) = field.strip_prefix("room.corner.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next().and_then(|s| s.parse().ok()).ok_or((EditError::UnknownField(field.into()), None))?;
        let axis = parts.next().ok_or((EditError::UnknownField(field.into()), Some(k)))?;
        if k >= cx.doc.room.outline.len() {
            return Err((EditError::Stale, Some(k)));
        }
        let cur_pt = cx.doc.room.outline[k];
        let val = parse_length(text.trim(), cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
        let new_pt = match axis {
            "x" => DVec2::new(val, cur_pt.y),
            "y" => DVec2::new(cur_pt.x, val),
            _ => return Err((EditError::UnknownField(field.into()), Some(k))),
        };
        let (new_room, _) = cx.doc.room.with_corner_moved(k, new_pt).map_err(|e| (EditError::Invalid(e.to_string()), Some(k)))?;
        return Ok(PreviewTarget::Room(new_room));
    }

    if let Some(rest) = field.strip_prefix("platform.zone.") {
        let mut parts = rest.split('.');
        let k: usize = parts.next().and_then(|s| s.parse().ok()).ok_or((EditError::UnknownField(field.into()), None))?;
        let prop = parts.next().ok_or((EditError::UnknownField(field.into()), Some(k)))?;
        let pid = match cx.selection {
            Some(SelectionTarget::Platform(pid)) => pid,
            Some(SelectionTarget::Node(_)) | Some(SelectionTarget::Room) => return Err((EditError::Stale, Some(k))),
            None => cx.doc.platforms.first().map(|p| p.id).ok_or((EditError::Stale, Some(k)))?,
        };
        let orig = cx.doc.platform(pid).ok_or((EditError::Stale, Some(k)))?;
        let mut p = orig.clone();
        let g = p.hole_grid.as_mut().ok_or((EditError::Stale, Some(k)))?;
        if k >= g.exclusions.len() {
            return Err((EditError::Stale, Some(k)));
        }
        let z = &mut g.exclusions[k];
        let trimmed = text.trim();
        match prop {
            "name" => z.name = trimmed.to_string(),
            "x" => {
                let x = parse_length(trimmed, cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
                match &mut z.shape {
                    ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => center.x = x,
                }
            }
            "y" => {
                let y = parse_length(trimmed, cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
                match &mut z.shape {
                    ZoneShape::Rect { center, .. } | ZoneShape::Circle { center, .. } => center.y = y,
                }
            }
            "w" => {
                let w = parse_length(trimmed, cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
                match &mut z.shape {
                    ZoneShape::Rect { size, .. } => size.x = w,
                    ZoneShape::Circle { .. } => return Err((EditError::UnknownField(field.into()), Some(k))),
                }
            }
            "h" => {
                let h = parse_length(trimmed, cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
                match &mut z.shape {
                    ZoneShape::Rect { size, .. } => size.y = h,
                    ZoneShape::Circle { .. } => return Err((EditError::UnknownField(field.into()), Some(k))),
                }
            }
            "r" => {
                let r = parse_length(trimmed, cx.units).map_err(|e| (EditError::Parse(e), Some(k)))?;
                match &mut z.shape {
                    ZoneShape::Circle { radius, .. } => *radius = r,
                    ZoneShape::Rect { .. } => return Err((EditError::UnknownField(field.into()), Some(k))),
                }
            }
            _ => return Err((EditError::UnknownField(field.into()), Some(k))),
        }
        p.validate().map_err(|e| (EditError::Invalid(e.to_string()), Some(k)))?;
        return Ok(PreviewTarget::Platform(p));
    }

    Err((EditError::UnknownField(field.into()), None))
}

pub fn parse_pasted_corners(text: &str, units: LengthUnit, with_radius: bool) -> Result<Vec<OutlineCorner>, String> {
    let mut corners = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = if t.contains(',') || t.contains('\t') {
            t.split([',', '\t']).map(str::trim).filter(|s| !s.is_empty()).collect()
        } else {
            t.split_whitespace().collect()
        };
        if tokens.len() < 2 {
            return Err(format!("line {line_no}: expected at least x and y coordinates"));
        }
        let x = parse_length(tokens[0], units).map_err(|e| format!("line {line_no}: {e}"))?;
        let y = parse_length(tokens[1], units).map_err(|e| format!("line {line_no}: {e}"))?;
        let radius = if with_radius && tokens.len() >= 3 {
            parse_length(tokens[2], units).map_err(|e| format!("line {line_no}: {e}"))?
        } else {
            0.0
        };
        corners.push(OutlineCorner { point: DVec2::new(x, y), radius });
    }
    if corners.len() < 3 {
        return Err("An outline needs at least 3 corners".into());
    }
    Ok(corners)
}

pub fn label_for(field_or_choice: &str) -> String {
    if let Some(p) = field_or_choice
        .strip_prefix("platform.preset.")
        .and_then(|id| PRESETS.iter().find(|pr| pr.id == id))
    {
        return format!("Apply {}", p.label);
    }
    if let Some(p) = field_or_choice
        .strip_prefix("platform.add.")
        .and_then(|id| PRESETS.iter().find(|pr| pr.id == id))
    {
        return format!("Add {}", p.label);
    }

    if let Some(rest) = field_or_choice.strip_prefix("platform.corner.") {
        if rest == "append" {
            return "Add Corner".into();
        }
        let mut parts = rest.split('.');
        let parsed = parts.next().and_then(|s| s.parse::<usize>().ok());
        if let (Some(k), action) = (parsed, parts.next()) {
            return match action {
                Some("insert") => "Add Corner".into(),
                Some("delete") => format!("Remove Corner {}", k + 1),
                _ => format!("Move Corner {}", k + 1),
            };
        }
    }
    if let Some(rest) = field_or_choice.strip_prefix("room.corner.") {
        if rest == "append" {
            return "Add Corner".into();
        }
        let mut parts = rest.split('.');
        let parsed = parts.next().and_then(|s| s.parse::<usize>().ok());
        if let (Some(k), action) = (parsed, parts.next()) {
            return match action {
                Some("insert") => "Add Corner".into(),
                Some("delete") => format!("Remove Corner {}", k + 1),
                _ => format!("Move Corner {}", k + 1),
            };
        }
    }

    if let Some(rest) = field_or_choice.strip_prefix("platform.zone.") {
        if rest == "add_rect" || rest == "add_circle" {
            return "Add Zone".into();
        }
        let mut parts = rest.split('.');
        let parsed = parts.next().and_then(|s| s.parse::<usize>().ok());
        if let (Some(k), action) = (parsed, parts.next()) {
            return match action {
                Some("delete") => format!("Remove Zone {}", k + 1),
                Some("name") => format!("Rename Zone {}", k + 1),
                _ => format!("Change Zone {}", k + 1),
            };
        }
    }

    if let Some(rest) = field_or_choice.strip_prefix("room.door.") {
        let mut parts = rest.split('.');
        if let (Some(_id_str), Some(action)) = (parts.next(), parts.next()) {
            return match action {
                "delete" => "Remove Door".into(),
                "name" => "Change Door Name".into(),
                "wall" => "Change Door Wall".into(),
                "offset" => "Change Door Offset".into(),
                "width" => "Change Door Width".into(),
                "height" => "Change Door Height".into(),
                "sill" => "Change Door Sill".into(),
                _ => "Change Door".into(),
            };
        }
    }

    match field_or_choice {
        "platform.outline.width" | "room.width" => "Change Width".into(),
        "platform.outline.depth" | "room.depth" => "Change Depth".into(),
        "platform.outline.radius" => "Change Corner Radius".into(),
        "platform.outline.notch_width" => "Change Notch Width".into(),
        "platform.outline.notch_depth" => "Change Notch Depth".into(),
        "platform.grid.margin" => "Change Edge Margin".into(),
        "platform.grid.pitch_x" => "Change Grid Pitch X".into(),
        "platform.grid.pitch_y" => "Change Grid Pitch Y".into(),
        "platform.grid.origin_x" | "platform.grid.origin_y" => "Change Grid Origin".into(),
        "platform.support.leg_size" => "Change Leg Size".into(),
        "platform.support.leg_inset" => "Change Leg Inset".into(),
        "platform.name" => "Rename Platform".into(),
        "platform.x" | "platform.y" => "Move Platform".into(),
        "platform.rotation" => "Change Rotation".into(),
        "platform.top_height" => "Change Top Height".into(),
        "platform.thickness" => "Change Thickness".into(),
        "room.height" => "Change Height".into(),
        "room.thickness" => "Change Thickness".into(),
        "room.door.add" => "Add Door".into(),
        "room.window.add" => "Add Window".into(),
        "room.make_l" => "Make L-shape".into(),
        "room.make_rect" => "Make Rectangle".into(),
        "platform.delete" => "Delete Platform".into(),
        "selection.delete" => "Delete".into(),
        "units" => "Change Units".into(),
        "walls" => "Change Wall Display".into(),
        _ => {
            let last = field_or_choice.split('.').next_back().unwrap_or(field_or_choice);
            format!("Change {}", last.replace('_', " "))
        }
    }
}

// ── Node (part) fields (M2-PLAN POLA-45) ───────────────────────────────────────────────────

/// A mated node's parent connector frame (in the parent's part frame), its own connector frame,
/// and the slide range. `None` when the node is not mated or a connector is not in the library.
pub fn node_mate(doc: &Document, catalog: &Catalog, node: &Node) -> Option<(Frame, Frame, (f64, f64))> {
    let (Parent::Node(pid), Attach::Mate { self_conn, parent_conn, .. }) = (&node.parent, &node.attach) else {
        return None;
    };
    let parent = doc.node(*pid)?;
    let parent_def = catalog.get(&parent.part)?;
    let range = lab_mates::slide_range(&parent_def.connector(parent_conn)?.kind);
    let pf = lab_mates::connector_frame(parent_def, parent_conn, &parent.params)?;
    let cf = lab_mates::connector_frame(catalog.get(&node.part)?, self_conn, &node.params)?;
    Some((pf, cf, range))
}

/// `SetAttach` commands that re-seat node `id` and its mated children after `id`'s params become
/// `params` (children's poses are parent-relative, so deeper members follow on their own).
fn repose_set(doc: &Document, catalog: &Catalog, id: NodeId, params: &Params) -> Vec<Command> {
    fn params_of<'a>(n: &'a Node, id: NodeId, params: &'a Params) -> &'a Params {
        if n.id == id { params } else { &n.params }
    }
    doc.nodes
        .iter()
        .filter(|n| n.id == id || n.parent == Parent::Node(id))
        .filter_map(|n| {
            let (Parent::Node(pid), Attach::Mate { self_conn, parent_conn, slide, twist }) = (&n.parent, &n.attach) else {
                return None;
            };
            let parent = doc.node(*pid)?;
            let pf = lab_mates::connector_frame(catalog.get(&parent.part)?, parent_conn, params_of(parent, id, params))?;
            let cf = lab_mates::connector_frame(catalog.get(&n.part)?, self_conn, params_of(n, id, params))?;
            let (position, rotation) = lab_mates::mate_pose(&pf, &cf, *slide, *twist);
            (position.distance(n.position) > 1e-9).then(|| Command::SetAttach {
                id: n.id,
                parent: n.parent,
                attach: n.attach.clone(),
                position,
                rotation,
            })
        })
        .collect()
}

/// Rotation of a node about the vertical axis, radians.
pub fn node_yaw(node: &Node) -> f64 {
    node.rotation.to_euler(EulerRot::ZYX).0
}

/// A committed node inspector field: `node.name`, `node.x|y|z`, `node.rotation`, `node.slide`
/// or `node.param.<name>`.
pub fn node_field_edit(cx: &EditContext, catalog: &Catalog, field: &str, text: &str) -> Result<EditOutcome, EditError> {
    let Some(SelectionTarget::Node(id)) = cx.selection else { return Err(EditError::Stale) };
    let node = cx.doc.node(id).ok_or(EditError::Stale)?;
    let t = text.trim();
    let same_length = |a: f64, b: f64| format_length(a, cx.units) == format_length(b, cx.units);
    let set_attach = |attach: Attach, position: DVec3, rotation: DQuat| Command::SetAttach {
        id,
        parent: node.parent,
        attach,
        position,
        rotation,
    };
    let command = match field {
        "node.name" => {
            if t == node.name {
                return Ok(EditOutcome::Nothing);
            }
            if t.is_empty() {
                return Err(EditError::Invalid("A part needs a name.".into()));
            }
            Command::Rename { id, name: t.to_string() }
        }
        "node.x" | "node.y" | "node.z" => {
            if matches!(node.attach, Attach::Mate { .. }) {
                return Err(EditError::Invalid("A mated part moves with its slide and rotation.".into()));
            }
            let value = parse_length(t, cx.units)?;
            let mut position = node.position;
            let axis = match field {
                "node.x" => &mut position.x,
                "node.y" => &mut position.y,
                _ => &mut position.z,
            };
            if same_length(*axis, value) {
                return Ok(EditOutcome::Nothing);
            }
            *axis = value;
            // Typed coordinates leave the hole; height keeps it.
            let attach = match node.attach {
                Attach::Hole { .. } if field != "node.z" => Attach::Free,
                ref attach => attach.clone(),
            };
            set_attach(attach, position, node.rotation)
        }
        "node.rotation" => {
            let angle = parse_angle_deg(t)?;
            match &node.attach {
                Attach::Mate { self_conn, parent_conn, slide, twist } => {
                    if format_angle_deg(*twist) == format_angle_deg(angle) {
                        return Ok(EditOutcome::Nothing);
                    }
                    let (parent, child, _) = node_mate(cx.doc, catalog, node)
                        .ok_or_else(|| EditError::Invalid("This part's connectors are not in the library.".into()))?;
                    let (position, rotation) = lab_mates::mate_pose(&parent, &child, *slide, angle);
                    let attach = Attach::Mate {
                        self_conn: self_conn.clone(),
                        parent_conn: parent_conn.clone(),
                        slide: *slide,
                        twist: angle,
                    };
                    set_attach(attach, position, rotation)
                }
                _ => {
                    let yaw = node_yaw(node);
                    if format_angle_deg(yaw) == format_angle_deg(angle) {
                        return Ok(EditOutcome::Nothing);
                    }
                    let rotation = (DQuat::from_rotation_z(angle - yaw) * node.rotation).normalize();
                    Command::SetPlacement { id, position: node.position, rotation }
                }
            }
        }
        "node.slide" => {
            let Attach::Mate { self_conn, parent_conn, slide, twist } = &node.attach else {
                return Err(EditError::Invalid("Only a mated part has a slide.".into()));
            };
            let value = parse_length(t, cx.units)?;
            if same_length(*slide, value) {
                return Ok(EditOutcome::Nothing);
            }
            let (parent, _, (lo, hi)) = node_mate(cx.doc, catalog, node)
                .ok_or_else(|| EditError::Invalid("This part's connectors are not in the library.".into()))?;
            if value < lo - 1e-9 || value > hi + 1e-9 {
                let suffix = cx.units.suffix();
                return Err(EditError::Invalid(format!(
                    "The slide must be between {} {suffix} and {} {suffix}.",
                    format_length(lo, cx.units),
                    format_length(hi, cx.units)
                )));
            }
            let position = node.position + parent.z.normalize_or_zero() * (value - slide);
            let attach = Attach::Mate {
                self_conn: self_conn.clone(),
                parent_conn: parent_conn.clone(),
                slide: value,
                twist: *twist,
            };
            set_attach(attach, position, node.rotation)
        }
        _ => {
            let Some(name) = field.strip_prefix("node.param.") else {
                return Err(EditError::UnknownField(field.to_string()));
            };
            let value = parse_length(t, cx.units)?;
            let def = catalog
                .get(&node.part)
                .ok_or_else(|| EditError::Invalid(format!("The part {} is not in the library.", node.part)))?;
            let Geometry::Parametric { generator, params } = &def.geometry else {
                return Err(EditError::Invalid("An imported part has no parameters.".into()));
            };
            let current = node.params.get(name).or_else(|| params.get(name)).and_then(ParamValue::as_f64);
            if current.is_some_and(|c| (c - value).abs() < 1e-9) {
                return Ok(EditOutcome::Nothing);
            }
            let mut overrides = node.params.clone();
            overrides.insert(name.to_string(), ParamValue::Number(value));
            let mut all = params.clone();
            all.extend(overrides.clone());
            lab_parts::generate(generator, &all).map_err(|e| EditError::Invalid(e.to_string()))?;
            let repose = repose_set(cx.doc, catalog, id, &overrides);
            let set = Command::SetParams { id, params: overrides };
            if repose.is_empty() { set } else { Command::Batch(std::iter::once(set).chain(repose).collect()) }
        }
    };
    Ok(EditOutcome::Command { command, label: format!("Edit {}", node.name), select: None, fixes: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::invalid_row;
    use lab_core::glam::{DQuat, DVec2, DVec3};
    use lab_core::holes::{compute_holes, ExclusionZone, GridOrigin, HoleGrid, ZoneShape};
    use lab_core::model::{Node, NodeId, Platform, PlatformId, PlatformKind, Support, Surface};
    use lab_core::outline::{Outline, OutlineCorner, RectCorner};
    use lab_core::room::OpeningFix;
    use lab_core::units::LengthUnit;

    fn test_doc() -> Document {
        let mut doc = Document::new("Doc");
        let p = Platform {
            id: PlatformId(1),
            name: "Table 1".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: Some(HoleGrid::metric()),
            support: Support::Legs { size: 120.0, inset: 150.0 },
        };
        doc.platforms.push(p);
        doc
    }

    // E1: field_edit on every platform and room field id.
    // A valid value gives expected command. "abc" gives Parse. Unchanged text gives Nothing.
    // In inches, re-committing '47.244' on a 1200 mm width gives Nothing. Inch input works on mm display.
    #[test]
    fn e1_field_edit_all_fields() {
        let mut doc = test_doc();
        let g = doc.platforms[0].hole_grid.as_mut().unwrap();
        g.exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Rect { center: DVec2::ZERO, size: DVec2::splat(100.0) },
        });
        g.exclusions.push(ExclusionZone {
            name: "Zone 2".into(),
            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 50.0 },
        });

        doc.room.openings.push(Opening {
            id: OpeningId(10),
            name: "Door 1".into(),
            wall: 0,
            offset: 500.0,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        });

        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };

        // Platform fields
        assert!(matches!(field_edit(&cx, "platform.name", "Table X").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.name", "Table 1").unwrap(), EditOutcome::Nothing);

        assert!(matches!(field_edit(&cx, "platform.x", "100").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.x", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.x", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.y", "50").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.y", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.y", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.rotation", "45").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.rotation", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.rotation", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.top_height", "850").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.top_height", "800").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.top_height", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.thickness", "150").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.thickness", "100").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.thickness", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.outline.width", "1500").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.outline.width", "1200").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.outline.width", "abc"), Err(EditError::Parse(_))));

        assert!(matches!(field_edit(&cx, "platform.outline.depth", "1000").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.outline.depth", "900").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.outline.depth", "abc"), Err(EditError::Parse(_))));

        // Outline-specific: rounded
        let mut doc_rounded = doc.clone();
        doc_rounded.platforms[0].outline = Outline::RoundedRectangle { size: DVec2::new(1200.0, 900.0), radius: 25.0 };
        let cx_rounded = EditContext { doc: &doc_rounded, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        assert!(matches!(field_edit(&cx_rounded, "platform.outline.radius", "30").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_rounded, "platform.outline.radius", "25").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_rounded, "platform.outline.radius", "abc"), Err(EditError::Parse(_))));

        // Outline-specific: LShape
        let mut doc_l = doc.clone();
        doc_l.platforms[0].outline = Outline::LShape { size: DVec2::new(1200.0, 900.0), notch: DVec2::new(400.0, 300.0), notch_corner: RectCorner::XPosYPos };
        let cx_l = EditContext { doc: &doc_l, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        assert!(matches!(field_edit(&cx_l, "platform.outline.notch_width", "450").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_l, "platform.outline.notch_width", "400").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_l, "platform.outline.notch_width", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_l, "platform.outline.notch_depth", "350").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_l, "platform.outline.notch_depth", "300").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_l, "platform.outline.notch_depth", "abc"), Err(EditError::Parse(_))));

        // Outline-specific: Polygon corners
        let mut doc_poly = doc.clone();
        doc_poly.platforms[0].outline = Outline::Polygon {
            corners: vec![
                OutlineCorner::sharp(DVec2::new(-600.0, -450.0)),
                OutlineCorner::sharp(DVec2::new(600.0, -450.0)),
                OutlineCorner::sharp(DVec2::new(600.0, 450.0)),
                OutlineCorner::sharp(DVec2::new(-600.0, 450.0)),
            ],
        };
        let cx_poly = EditContext { doc: &doc_poly, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.x", "-550").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_poly, "platform.corner.0.x", "-600").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.x", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.y", "-400").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_poly, "platform.corner.0.y", "-450").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.y", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.r", "10").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_poly, "platform.corner.0.r", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_poly, "platform.corner.0.r", "abc"), Err(EditError::Parse(_))));

        // Grid fields
        assert!(matches!(field_edit(&cx, "platform.grid.pitch_x", "50").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.grid.pitch_x", "25").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.grid.pitch_x", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.grid.pitch_y", "50").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.grid.pitch_y", "25").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.grid.pitch_y", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.grid.margin", "50").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.grid.margin", "12.5").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.grid.margin", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.grid.origin_x", "-500").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.grid.origin_x", "-587.5").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.grid.origin_x", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.grid.origin_y", "-400").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.grid.origin_y", "-437.5").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.grid.origin_y", "abc"), Err(EditError::Parse(_))));

        // Zone fields
        assert!(matches!(field_edit(&cx, "platform.zone.0.name", "ZNew").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.0.name", "Zone 1").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.0.x", "10").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.0.x", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.0.x", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.zone.0.y", "10").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.0.y", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.0.y", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.zone.0.w", "120").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.0.w", "100").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.0.w", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.zone.0.h", "120").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.0.h", "100").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.0.h", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.zone.1.r", "60").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.zone.1.r", "50").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.zone.1.r", "abc"), Err(EditError::Parse(_))));

        // Support fields
        assert!(matches!(field_edit(&cx, "platform.support.leg_size", "140").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.support.leg_size", "120").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.support.leg_size", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx, "platform.support.leg_inset", "160").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx, "platform.support.leg_inset", "150").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx, "platform.support.leg_inset", "abc"), Err(EditError::Parse(_))));

        // Room fields
        let cx_room = EditContext { doc: &doc, selection: Some(SelectionTarget::Room), units: LengthUnit::Millimetre };
        assert!(matches!(field_edit(&cx_room, "room.width", "7000").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.width", "6000").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.width", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.depth", "5500").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.depth", "5000").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.depth", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.height", "3500").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.height", "3000").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.height", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.thickness", "250").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.thickness", "150").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.thickness", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.corner.0.x", "-3200").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.corner.0.x", "-3000").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.corner.0.x", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.corner.0.y", "-2700").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.corner.0.y", "-2500").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.corner.0.y", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.door.10.name", "Side Door").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.door.10.name", "Door 1").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.door.10.offset", "600").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.door.10.offset", "500").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.door.10.offset", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.door.10.width", "950").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.door.10.width", "1000").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.door.10.width", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.door.10.height", "2200").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.door.10.height", "2100").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.door.10.height", "abc"), Err(EditError::Parse(_))));
        assert!(matches!(field_edit(&cx_room, "room.door.10.sill", "50").unwrap(), EditOutcome::Command { .. }));
        assert_eq!(field_edit(&cx_room, "room.door.10.sill", "0").unwrap(), EditOutcome::Nothing);
        assert!(matches!(field_edit(&cx_room, "room.door.10.sill", "abc"), Err(EditError::Parse(_))));

        // In inches, re-committing '47.244' on a 1200 mm width returns Nothing
        let cx_in = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Inch,
        };
        let res = field_edit(&cx_in, "platform.outline.width", "47.244").unwrap();
        assert_eq!(res, EditOutcome::Nothing);

        // Inch input works on mm display
        let res = field_edit(&cx, "platform.outline.width", "10 in").unwrap();
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                match platform.outline {
                    Outline::Rectangle { size } => assert!((size.x - 254.0).abs() < 1e-6),
                    _ => panic!("wrong outline"),
                }
            }
            _ => panic!("expected command"),
        }
    }

    // E2: A corner edit producing a bow-tie gives Invalid containing "cross", and invalid_row points at right row.
    #[test]
    fn e2_bowtie_corner_edit() {
        let mut doc = test_doc();
        // Convert to custom polygon
        doc.platforms[0].outline = Outline::Polygon {
            corners: vec![
                OutlineCorner::sharp(DVec2::new(-500.0, -500.0)),
                OutlineCorner::sharp(DVec2::new(500.0, -500.0)),
                OutlineCorner::sharp(DVec2::new(500.0, 500.0)),
                OutlineCorner::sharp(DVec2::new(-500.0, 500.0)),
            ],
        };
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };

        // Move corner 0 (bottom-left) to (600, 600) causing edges to cross
        let res = field_edit(&cx, "platform.corner.0.x", "600");
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err, EditError::Invalid(ref msg) if msg.contains("cross")));
        let row = invalid_row(&err);
        assert_eq!(row, Some(1));
    }

    // E3: A stale corner index gives Stale.
    #[test]
    fn e3_stale_corner_index() {
        let mut doc = test_doc();
        doc.platforms[0].outline = Outline::Polygon {
            corners: vec![
                OutlineCorner::sharp(DVec2::new(-500.0, -500.0)),
                OutlineCorner::sharp(DVec2::new(500.0, -500.0)),
                OutlineCorner::sharp(DVec2::new(500.0, 500.0)),
            ],
        };
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };
        let res = field_edit(&cx, "platform.corner.99.x", "100");
        assert_eq!(res, Err(EditError::Stale));
    }

    // E4: A room corner move that strands a door gives a Command with a Moved or Removed fix.
    #[test]
    fn e4_room_corner_move_strands_door() {
        let mut doc = test_doc();
        doc.room.openings.push(Opening {
            id: OpeningId(10),
            name: "Door 1".into(),
            wall: 0,
            offset: 600.0,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        });
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Room),
            units: LengthUnit::Millimetre,
        };
        let res = field_edit(&cx, "room.corner.0.x", "2000").unwrap();
        match res {
            EditOutcome::Command { fixes, .. } => {
                assert!(!fixes.is_empty());
                assert!(matches!(fixes[0], OpeningFix::Moved { .. } | OpeningFix::Removed { .. }));
            }
            _ => panic!("expected command with fixes"),
        }
    }

    // E5: Switching the grid standard keeps the zones.
    #[test]
    fn e5_switch_grid_keeps_zones() {
        let mut doc = test_doc();
        doc.platforms[0].hole_grid.as_mut().unwrap().exclusions.push(ExclusionZone {
            name: "Zone 1".into(),
            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 50.0 },
        });
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };

        let res = choice_edit(&cx, "platform.grid.standard", 2).unwrap(); // Imperial
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                let g = platform.hole_grid.unwrap();
                assert_eq!(g.exclusions.len(), 1);
                assert_eq!(g.exclusions[0].name, "Zone 1");
            }
            _ => panic!("expected command"),
        }
    }

    // E6: Outline kind switches as specified.
    #[test]
    fn e6_outline_kind_switches() {
        let doc = test_doc();
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };

        // 1. Rect (1200x900) -> Rounded (radius 25)
        let res = choice_edit(&cx, "platform.outline.kind", 1).unwrap();
        let rounded_p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                match platform.outline {
                    Outline::RoundedRectangle { size, radius } => {
                        assert_eq!(size, DVec2::new(1200.0, 900.0));
                        assert_eq!(radius, 25.0);
                    }
                    _ => panic!("expected rounded"),
                }
                *platform
            }
            _ => panic!("expected command"),
        };

        // Radius clamp to min(hx, hy)
        let mut tiny_doc = doc.clone();
        tiny_doc.platforms[0].outline = Outline::Rectangle { size: DVec2::new(50.0, 50.0) };
        let tiny_cx = EditContext { doc: &tiny_doc, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let tiny_res = choice_edit(&tiny_cx, "platform.outline.kind", 1).unwrap();
        match tiny_res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                match platform.outline {
                    Outline::RoundedRectangle { radius, .. } => assert_eq!(radius, 25.0),
                    _ => panic!("expected rounded"),
                }
            }
            _ => panic!("expected command"),
        }

        // 2. Rect -> L
        let res = choice_edit(&cx, "platform.outline.kind", 2).unwrap();
        let l_p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                match platform.outline {
                    Outline::LShape { size, notch, notch_corner } => {
                        assert_eq!(size, DVec2::new(1200.0, 900.0));
                        assert_eq!(notch, DVec2::new(400.0, 300.0));
                        assert_eq!(notch_corner, RectCorner::XPosYPos);
                    }
                    _ => panic!("expected L shape"),
                }
                *platform
            }
            _ => panic!("expected command"),
        };

        // 3. Rect -> Custom (exact polygon)
        let res = choice_edit(&cx, "platform.outline.kind", 3).unwrap();
        let custom_p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                match &platform.outline {
                    Outline::Polygon { corners } => {
                        let expected = doc.platforms[0].outline.to_polygon();
                        match expected {
                            Outline::Polygon { corners: ec } => assert_eq!(corners, &ec),
                            _ => unreachable!(),
                        }
                    }
                    _ => panic!("expected custom polygon"),
                }
                *platform
            }
            _ => panic!("expected command"),
        };

        // 4. Rounded -> Rect (bbox size, centred)
        let mut doc_r = doc.clone();
        doc_r.platforms[0] = rounded_p;
        let cx_r = EditContext { doc: &doc_r, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = choice_edit(&cx_r, "platform.outline.kind", 0).unwrap();
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                assert_eq!(platform.outline, Outline::Rectangle { size: DVec2::new(1200.0, 900.0) });
            }
            _ => panic!("expected command"),
        }

        // 5. L -> Rect (bbox size, centred)
        let mut doc_l = doc.clone();
        doc_l.platforms[0] = l_p;
        let cx_l = EditContext { doc: &doc_l, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = choice_edit(&cx_l, "platform.outline.kind", 0).unwrap();
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                assert_eq!(platform.outline, Outline::Rectangle { size: DVec2::new(1200.0, 900.0) });
            }
            _ => panic!("expected command"),
        }

        // 6. Custom -> Rect (bbox size, centred)
        let mut doc_c = doc.clone();
        doc_c.platforms[0] = custom_p;
        let cx_c = EditContext { doc: &doc_c, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = choice_edit(&cx_c, "platform.outline.kind", 0).unwrap();
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                assert_eq!(platform.outline, Outline::Rectangle { size: DVec2::new(1200.0, 900.0) });
            }
            _ => panic!("expected command"),
        }
    }

    // E7: Adding a door to a room whose walls are all short gives Invalid.
    #[test]
    fn e7_door_add_all_walls_short() {
        let mut doc = test_doc();
        doc.room.outline = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(800.0, 0.0),
            DVec2::new(800.0, 800.0),
            DVec2::new(0.0, 800.0),
        ];
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Room),
            units: LengthUnit::Millimetre,
        };
        let res = action_edit(&cx, "room.door.add");
        assert!(matches!(res, Err(EditError::Invalid(msg)) if msg.contains("No wall is long enough")));
    }

    // E8: Deleting a platform gives RemovePlatform, node gives RemoveNode, room gives Nothing.
    /// Typing a rotation into the node inspector turns the node about +Z (POLA-45).
    #[test]
    fn node_rotation_edit() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Post".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "t:none".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(100.0, 50.0, 0.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let catalog = Catalog::from_defs(Vec::new());
        let cx = EditContext { doc: &doc, selection: Some(SelectionTarget::Node(NodeId(101))), units: LengthUnit::Millimetre };
        let Ok(EditOutcome::Command { command: Command::SetPlacement { rotation, position, .. }, .. }) =
            node_field_edit(&cx, &catalog, "node.rotation", "45")
        else {
            panic!("expected a SetPlacement");
        };
        assert!((rotation.to_euler(EulerRot::ZYX).0.to_degrees() - 45.0).abs() < 1e-9);
        assert_eq!(position, DVec3::new(100.0, 50.0, 0.0));
        assert_eq!(node_field_edit(&cx, &catalog, "node.rotation", "0"), Ok(EditOutcome::Nothing));
    }

    #[test]
    fn e8_delete_platform_node_and_room() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(100.0, 50.0, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });

        // 1. platform.delete
        let cx_p = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };
        let res = action_edit(&cx_p, "platform.delete").unwrap();
        assert_eq!(res, EditOutcome::Command {
            command: Command::RemovePlatform(PlatformId(1)),
            label: "Delete Platform".into(),
            select: None,
            fixes: Vec::new(),
        });

        // 2. selection.delete on Platform
        let res = action_edit(&cx_p, "selection.delete").unwrap();
        assert_eq!(res, EditOutcome::Command {
            command: Command::RemovePlatform(PlatformId(1)),
            label: "Delete Platform".into(),
            select: None,
            fixes: Vec::new(),
        });

        // 3. selection.delete on Node -> RemoveNode
        let cx_n = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Node(NodeId(101))),
            units: LengthUnit::Millimetre,
        };
        let res = action_edit(&cx_n, "selection.delete").unwrap();
        assert_eq!(res, EditOutcome::Command {
            command: Command::RemoveNodes(vec![NodeId(101)]),
            label: "Delete".into(),
            select: None,
            fixes: Vec::new(),
        });

        // 4. selection.delete on Room -> Nothing
        let cx_r = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Room),
            units: LengthUnit::Millimetre,
        };
        let res = action_edit(&cx_r, "selection.delete").unwrap();
        assert_eq!(res, EditOutcome::Nothing);
    }

    // E9: Pinning: an outline edit on a platform with nodes converts Auto to At,
    // and hole positions are unchanged for every node position.
    #[test]
    fn e9_pinning_rule() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(112.5, 37.5, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });

        assert_eq!(doc.platforms[0].hole_grid.as_ref().unwrap().origin, GridOrigin::Auto);
        let old = compute_holes(&doc.platforms[0]).unwrap();
        let cx = EditContext {
            doc: &doc,
            selection: Some(SelectionTarget::Platform(PlatformId(1))),
            units: LengthUnit::Millimetre,
        };

        for w in ["1400", "1237", "1013"] {
            let res = field_edit(&cx, "platform.outline.width", w).unwrap();
            let p = match res {
                EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => *platform,
                _ => panic!("expected command"),
            };
            assert!(matches!(p.hole_grid.as_ref().unwrap().origin, GridOrigin::At(_)));
            let new = compute_holes(&p).unwrap();
            let pt = doc.nodes[0].position.truncate();
            let a = old.position(old.nearest(pt).unwrap());
            let b = new.position(new.nearest(pt).unwrap());
            assert!((a - b).length() < 1e-9, "width {w}: hole shifted under node: {a:?} -> {b:?}");
        }
    }

    // E10: A units or walls choice gives Preference, never a command.
    #[test]
    fn e10_preference_choices() {
        let doc = test_doc();
        let cx = EditContext {
            doc: &doc,
            selection: None,
            units: LengthUnit::Millimetre,
        };

        let res = choice_edit(&cx, "units", 1).unwrap();
        assert_eq!(res, EditOutcome::Preference(PreferenceChange::Units(LengthUnit::Inch)));

        let res = choice_edit(&cx, "walls", 2).unwrap();
        assert_eq!(res, EditOutcome::Preference(PreferenceChange::Walls(WallDisplay::Hidden)));
    }

    // E11: parse_pasted_corners handles commas, tabs and an error line number.
    #[test]
    fn e11_parse_pasted_corners() {
        let text = "0, 0\n100\t0\n100, 100\n0\t100";
        let corners = parse_pasted_corners(text, LengthUnit::Millimetre, false).unwrap();
        assert_eq!(corners.len(), 4);
        assert_eq!(corners[1].point, DVec2::new(100.0, 0.0));

        let text_with_err = "0, 0\n100, 0\nbad_val, 100\n0, 100";
        let err = parse_pasted_corners(text_with_err, LengthUnit::Millimetre, false).unwrap_err();
        assert!(err.starts_with("line 3:"));
    }

    // Regressions for Major 1-3 and Minor audit findings:

    #[test]
    fn regression_origin_x_edit_preserved() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(112.5, 37.5, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let cx = EditContext { doc: &doc, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = field_edit(&cx, "platform.grid.origin_x", "-500").unwrap();
        let p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => *platform,
            _ => panic!("expected command"),
        };
        let g = p.hole_grid.as_ref().unwrap();
        let o = lattice_origin(g, p.outline.bounds());
        assert_eq!(o.x, -500.0, "typed origin X was discarded");
    }

    #[test]
    fn regression_preset_apply_pins_grid() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(112.5, 37.5, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let before = lattice_origin(doc.platforms[0].hole_grid.as_ref().unwrap(), doc.platforms[0].outline.bounds());
        let cx = EditContext { doc: &doc, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = action_edit(&cx, "platform.preset.table-m-1500x1200").unwrap();
        let p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => *platform,
            _ => panic!("expected command"),
        };
        let after = lattice_origin(p.hole_grid.as_ref().unwrap(), p.outline.bounds());
        let d = after - before;
        let pitch = 25.0;
        let on_lattice = (d.x / pitch).fract().abs() < 1e-9 && (d.y / pitch).fract().abs() < 1e-9;
        assert!(on_lattice, "lattice shifted by {d:?} under placed parts");
    }

    #[test]
    fn regression_corner_delete_pins_grid() {
        let mut doc = test_doc();
        doc.platforms[0].outline = Outline::Polygon {
            corners: vec![
                OutlineCorner::sharp(DVec2::new(-600.0, -450.0)),
                OutlineCorner::sharp(DVec2::new(600.0, -450.0)),
                OutlineCorner::sharp(DVec2::new(600.0, 450.0)),
                OutlineCorner::sharp(DVec2::new(0.0, 470.0)),
                OutlineCorner::sharp(DVec2::new(-600.0, 450.0)),
            ],
        };
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(112.5, 37.5, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let before = lattice_origin(doc.platforms[0].hole_grid.as_ref().unwrap(), doc.platforms[0].outline.bounds());
        let cx = EditContext { doc: &doc, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = action_edit(&cx, "platform.corner.3.delete").unwrap();
        let p = match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => *platform,
            _ => panic!("expected command"),
        };
        let after = lattice_origin(p.hole_grid.as_ref().unwrap(), p.outline.bounds());
        assert_eq!(before, after, "lattice moved under placed parts");
    }

    #[test]
    fn regression_node_selected_platform_edit_returns_stale() {
        let mut doc = test_doc();
        doc.nodes.push(Node {
            id: NodeId(101),
            name: "Laser".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: DVec3::new(112.5, 37.5, 100.0),
            rotation: DQuat::IDENTITY,
            pickable: true,
        });
        let cx_node = EditContext { doc: &doc, selection: Some(SelectionTarget::Node(NodeId(101))), units: LengthUnit::Millimetre };
        assert_eq!(field_edit(&cx_node, "platform.outline.width", "700"), Err(EditError::Stale));
        assert_eq!(choice_edit(&cx_node, "platform.outline.kind", 1), Err(EditError::Stale));
        assert_eq!(action_edit(&cx_node, "platform.preset.table-m-1500x1200"), Err(EditError::Stale));
    }

    #[test]
    fn regression_door_window_add_multiple_and_labels() {
        let doc = lab_core::sample::demo_document();
        let mut current_doc = doc;
        let cx = EditContext { doc: &current_doc, selection: Some(SelectionTarget::Room), units: LengthUnit::Millimetre };
        // Adding first door
        let res = action_edit(&cx, "room.door.add").unwrap();
        match res {
            EditOutcome::Command { command, .. } => {
                command.apply(&mut current_doc).unwrap();
            }
            _ => panic!("expected SetRoom"),
        }

        // Adding second door succeeds without overlap
        let cx2 = EditContext { doc: &current_doc, selection: Some(SelectionTarget::Room), units: LengthUnit::Millimetre };
        let res2 = action_edit(&cx2, "room.door.add").unwrap();
        match res2 {
            EditOutcome::Command { command, .. } => {
                command.apply(&mut current_doc).unwrap();
            }
            _ => panic!("expected SetRoom"),
        }

        // Adding window succeeds without overlap
        let cx3 = EditContext { doc: &current_doc, selection: Some(SelectionTarget::Room), units: LengthUnit::Millimetre };
        let res3 = action_edit(&cx3, "room.window.add").unwrap();
        match res3 {
            EditOutcome::Command { command, .. } => {
                command.apply(&mut current_doc).unwrap();
            }
            _ => panic!("expected SetRoom"),
        }

        // Check window delete label is "Remove Window"
        let win = current_doc.room.openings.iter().find(|o| o.name.starts_with("Window")).unwrap();
        let win_del_action = format!("room.door.{}.delete", win.id.0);
        let cx4 = EditContext { doc: &current_doc, selection: Some(SelectionTarget::Room), units: LengthUnit::Millimetre };
        let del_res = action_edit(&cx4, &win_del_action).unwrap();
        match del_res {
            EditOutcome::Command { label, .. } => assert_eq!(label, "Remove Window"),
            _ => panic!("expected command"),
        }
    }

    #[test]
    fn regression_rename_numeric_text_not_noop() {
        let mut doc = test_doc();
        doc.platforms[0].name = "10".into();
        let cx = EditContext { doc: &doc, selection: Some(SelectionTarget::Platform(PlatformId(1))), units: LengthUnit::Millimetre };
        let res = field_edit(&cx, "platform.name", "10.0").unwrap();
        match res {
            EditOutcome::Command { command: Command::UpdatePlatform { platform }, .. } => {
                assert_eq!(platform.name, "10.0");
            }
            _ => panic!("expected UpdatePlatform, not no-op"),
        }
    }
}
