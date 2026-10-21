//! Top-view labels and dimensions (M3-PLAN POLA-54): maps [`OverlayItem`]s to the Slint overlay,
//! with names from the document and lengths in the display unit.

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::SelectionTarget;
use lab_core::units::format_length_with_unit;
use lab_ui::{AppWindow, OverlayItem as UiItem};
use lab_viewport::OverlayItem;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app_state::AppState;

/// End-tick half length in logical px.
const TICK: f32 = 4.0;

thread_local! {
    /// The last items, so a unit change or rename can re-format them.
    static LAST: RefCell<Vec<OverlayItem>> = const { RefCell::new(Vec::new()) };
}

pub fn on_overlay(ui: &AppWindow, app: &Rc<RefCell<AppState>>, items: Vec<OverlayItem>) {
    LAST.with(|l| *l.borrow_mut() = items);
    refresh(ui, &app.borrow());
}

/// Re-formats the overlay, e.g. after the display unit changed.
pub fn refresh(ui: &AppWindow, app: &AppState) {
    let scale = ui.window().scale_factor();
    let unit = app.units();
    let doc = &app.session.doc;
    let (mut candidates, mut dims) = (Vec::new(), Vec::new());
    LAST.with(|l| {
        for item in l.borrow().iter() {
            match item {
                OverlayItem::Label { target, pos } => {
                    let name = match target {
                        SelectionTarget::Platform(id) => doc.platform(*id).map(|p| p.name.clone()),
                        SelectionTarget::Node(id) => doc.node(*id).map(|n| n.name.clone()),
                        _ => None,
                    };
                    let Some(text) = name else { continue };
                    let rank = if Some(*target) == app.session.selection {
                        0
                    } else if matches!(target, SelectionTarget::Platform(_)) {
                        1
                    } else {
                        2
                    };
                    candidates.push((rank, *pos / scale, text));
                }
                OverlayItem::Dimension { a, b, length_mm } => {
                    let (a, b) = (*a / scale, *b / scale);
                    let n = (b - a).perp().normalize_or_zero() * TICK;
                    let commands = format!(
                        "M {} {} L {} {} M {} {} L {} {} M {} {} L {} {}",
                        a.x, a.y, b.x, b.y,
                        a.x - n.x, a.y - n.y, a.x + n.x, a.y + n.y,
                        b.x - n.x, b.y - n.y, b.x + n.x, b.y + n.y,
                    );
                    let m = (a + b) / 2.0;
                    dims.push(UiItem { x: m.x, y: m.y, text: format_length_with_unit(*length_mm, unit).into(), commands: commands.into() });
                }
            }
        }
    });
    let labels = place_labels(candidates);
    ui.set_overlay_labels(ModelRc::new(VecModel::from(labels)));
    ui.set_overlay_dimensions(ModelRc::new(VecModel::from(dims)));
}

/// Greedy collision avoidance: selected first, then platforms, then parts; a label whose
/// estimated rectangle overlaps one already placed is skipped.
fn place_labels(mut candidates: Vec<(u8, lab_viewport::Vec2, String)>) -> Vec<UiItem> {
    // shortcut: label size estimated from the character count (caption font); measure the text
    // if labels with wide glyphs still overlap.
    const CHAR_W: f32 = 7.0;
    const H: f32 = 18.0;
    candidates.sort_by_key(|c| c.0);
    let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut out = Vec::new();
    for (_, pos, text) in candidates {
        let r = (pos.x, pos.y, pos.x + CHAR_W * text.chars().count() as f32 + 12.0, pos.y + H);
        if placed.iter().any(|p| r.0 < p.2 && p.0 < r.2 && r.1 < p.3 && p.1 < r.3) {
            continue;
        }
        placed.push(r);
        out.push(UiItem { x: pos.x, y: pos.y, text: text.into(), commands: "".into() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_labels_keep_the_higher_rank() {
        let v = lab_viewport::Vec2::new;
        let out = place_labels(vec![
            (2, v(10.0, 10.0), "Beam".into()),
            (0, v(14.0, 12.0), "L1".into()),
            (2, v(200.0, 10.0), "HWP".into()),
        ]);
        let names: Vec<_> = out.iter().map(|i| i.text.to_string()).collect();
        assert_eq!(names, ["L1", "HWP"]);
    }
}
