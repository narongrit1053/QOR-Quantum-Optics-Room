//! Explorer file drop onto the window (M9-PLAN B): a STEP file is inserted where it lands.
//! Slint does not forward winit's `DroppedFile`, so this hooks the winit window directly.

use std::cell::RefCell;
use std::rc::Rc;

use lab_ui::AppWindow;
use lab_viewport::Vec2;
use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.window().on_winit_window_event(move |_, event| {
        if let WindowEvent::DroppedFile(path) = event {
            let (w, a, m, path) = (w.clone(), a.clone(), m.clone(), path.clone());
            // Out of the winit callback, so no app borrow can be live.
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = w.upgrade() else { return };
                let step = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp"));
                if step {
                    let pos = cursor(&ui);
                    crate::import::quick_insert(&ui, &a, &m, path, pos);
                } else {
                    controller::show_toast(&ui, &a.borrow(), "Only STEP files can be dropped", false);
                }
            });
        }
        EventResult::Propagate
    });
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
