//! Explorer file drop onto the window (M9-PLAN B): a STEP file is inserted where it lands.
//! Slint does not forward winit's `DroppedFile`, so this hooks the winit window directly.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use lab_core::part::{Feature, PartDef};
use lab_core::{Command, NodeId};
use lab_ui::AppWindow;
use lab_viewport::Vec2;
use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

thread_local! {
    /// STEP files of one Explorer drop still to insert; `None` while idle.
    static PENDING: RefCell<Option<VecDeque<PathBuf>>> = const { RefCell::new(None) };
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.window().on_winit_window_event(move |_, event| {
        if let WindowEvent::DroppedFile(path) = event {
            let (w, a, m, path) = (w.clone(), a.clone(), m.clone(), path.clone());
            // Out of the winit callback, so no app borrow can be live.
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = w.upgrade() else { return };
                let step = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp"));
                if !step {
                    controller::show_toast(&ui, &a.borrow(), "Only STEP files can be dropped", false);
                    return;
                }
                // Files of one drop arrive back to back: the first starts the chain, the rest queue.
                let first = PENDING.with(|p| match &mut *p.borrow_mut() {
                    Some(queue) => {
                        queue.push_back(path.clone());
                        false
                    }
                    none => {
                        *none = Some(VecDeque::from([path.clone()]));
                        true
                    }
                });
                if first {
                    next(&ui, &a, &m, cursor(&ui), None);
                }
            });
        }
        EventResult::Propagate
    });
}

/// Inserts the next queued file at `pos`; `right` is the world X of the previous part's far edge,
/// so this part starts 20 mm beyond it.
fn next(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, pos: Option<Vec2>, right: Option<f64>) {
    let Some(path) = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let path = p.as_mut().and_then(VecDeque::pop_front);
        if path.is_none() {
            *p = None;
        }
        path
    }) else {
        return;
    };
    let before = app.borrow().session.doc.next_free_id();
    crate::import::quick_insert(ui, app, models, path, pos);
    wait_placed(ui.as_weak(), app.clone(), models.clone(), pos, right, before, 0);
}

/// Polls for the new node (a conversion runs on a thread); then offsets it and goes on.
fn wait_placed(w: slint::Weak<AppWindow>, app: Rc<RefCell<AppState>>, models: Rc<UiModels>, pos: Option<Vec2>, right: Option<f64>, before: u64, tries: u32) {
    slint::Timer::single_shot(std::time::Duration::from_millis(250), move || {
        let Some(ui) = w.upgrade() else { return };
        if app.borrow().session.doc.next_free_id() <= before {
            // shortcut: gives up after 30 s (a failed conversion never places); the chain moves on.
            if tries < 120 {
                return wait_placed(w, app, models, pos, right, before, tries + 1);
            }
            return next(&ui, &app, &models, pos, right);
        }
        let placed = {
            let a = app.borrow();
            a.session.doc.node(NodeId(before)).map(|n| {
                let (min, max) = a.catalog.get(&n.part).map_or((0.0, 0.0), x_extent);
                (n.id, n.position, n.rotation, min, max)
            })
        };
        let mut right = right;
        if let Some((id, mut position, rotation, min, max)) = placed {
            if let Some(r) = right {
                // shortcut: assumes the dropped parts are unrotated, which a drop onto the table is.
                position.x = r + 20.0 - min;
                let label = "Space dropped parts".to_string();
                controller::execute(&ui, &app, &models, "drop-offset", Command::SetPlacement { id, position, rotation }, label, None, Vec::new());
            }
            right = Some(position.x + max);
        }
        next(&ui, &app, &models, pos, right);
    });
}

/// The part's X extent in its own frame, from its detected side faces (0, 0 when it has none).
fn x_extent(def: &PartDef) -> (f64, f64) {
    let xs: Vec<f64> = def
        .features
        .iter()
        .filter_map(|f| match f {
            Feature::Plane { origin, normal } if normal.x.abs() > 0.99 => Some(origin.x),
            _ => None,
        })
        .collect();
    let min = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if xs.is_empty() { (0.0, 0.0) } else { (min, max) }
}

/// The cursor in viewport px when it is over the viewport (winit's `DroppedFile` carries no
/// position).
#[cfg(windows)]
fn cursor(ui: &AppWindow) -> Option<Vec2> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
        fn ScreenToClient(hwnd: isize, p: *mut Point) -> i32;
    }
    let mut p = Point { x: 0, y: 0 };
    let hwnd = crate::dialogs::owner_hwnd(ui.window());
    // SAFETY: `p` is a valid POINT; `hwnd` is this window's handle (0 makes both calls fail).
    if hwnd == 0 || unsafe { GetCursorPos(&mut p) == 0 || ScreenToClient(hwnd, &mut p) == 0 } {
        return None;
    }
    let scale = ui.window().scale_factor();
    let (x, y) = (p.x as f32 / scale, p.y as f32 / scale);
    let vx = ui.get_viewport_x();
    let inside =
        x > ui.get_sidebar_right() && x < vx + ui.get_viewport_width() && y >= 0.0 && y < ui.get_viewport_height();
    inside.then(|| Vec2::new((x - vx) * scale, y * scale))
}

#[cfg(not(windows))]
fn cursor(_: &AppWindow) -> Option<Vec2> {
    None
}
