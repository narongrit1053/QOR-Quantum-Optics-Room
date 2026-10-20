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
    let (mut labels, mut dims) = (Vec::new(), Vec::new());
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
                    labels.push(UiItem { x: pos.x / scale, y: pos.y / scale, text: text.into(), commands: "".into() });
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
    ui.set_overlay_labels(ModelRc::new(VecModel::from(labels)));
    ui.set_overlay_dimensions(ModelRc::new(VecModel::from(dims)));
}
