//! The measure readout (M3-PLAN POLA-53): formats [`ViewportEvent::Measured`] in the display unit.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use lab_core::LengthUnit;
use lab_core::units::{format_length, format_length_with_unit};
use lab_ui::AppWindow;
use lab_viewport::Measurement;

use crate::app_state::AppState;

thread_local! {
    /// The last measurement, so a unit change can re-format it.
    static LAST: Cell<Option<Measurement>> = const { Cell::new(None) };
}

pub fn on_measured(ui: &AppWindow, app: &Rc<RefCell<AppState>>, m: Option<Measurement>) {
    LAST.with(|l| l.set(m));
    refresh(ui, app.borrow().units());
}

/// Re-formats the readout, e.g. after the display unit changed.
pub fn refresh(ui: &AppWindow, unit: LengthUnit) {
    ui.set_measure_text(LAST.with(Cell::get).map(|m| readout(m, unit)).unwrap_or_default().into());
}

fn readout(m: Measurement, unit: LengthUnit) -> String {
    let Some(b) = m.b else { return "Click the second point".into() };
    let d = b - m.a;
    let f = |v: f64| format_length(v, unit);
    format!("{}  (Δx {}, Δy {}, Δz {})", format_length_with_unit(d.length(), unit), f(d.x), f(d.y), f(d.z))
}
