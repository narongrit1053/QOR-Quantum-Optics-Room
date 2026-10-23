//! M9 F. Floating part box (docs/M9-PLAN.md F): one node drawn by its own viewport camera in a
//! draggable panel. Picks in it arrive as `FeaturePoint`, like picks in the main view.

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::NodeId;
use lab_ui::AppWindow;
use lab_viewport::{Modifiers, PartBoxSpec, PointerButton, Vec2, ViewportInput, ViewportRequest, wgpu};
use slint::ComponentHandle;

use crate::app_state::AppState;

/// Logical size of the box's image (`part_box.slint`).
const IMAGE_PX: f32 = 320.0;

pub fn open(ui: &AppWindow, app: &Rc<RefCell<AppState>>, node: NodeId) {
    let a = app.borrow();
    let Some(name) = a.session.doc.node(node).map(|n| n.name.clone()) else { return };
    let px = (IMAGE_PX * ui.window().scale_factor()).round() as u32;
    a.send(ViewportRequest::PartBox(Some(PartBoxSpec { node, width: px, height: px })));
    ui.set_part_box_title(name.into());
    ui.set_part_box_open(true);
}

pub fn close(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    if !ui.get_part_box_open() {
        return;
    }
    ui.set_part_box_open(false);
    ui.set_part_box_texture(slint::Image::default());
    app.borrow().send(ViewportRequest::PartBox(None));
    crate::constrain::part_box_closed(ui, app);
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_part_box_pointer(move |kind, button, x, y, shift, ctrl, alt| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let scale = ui.window().scale_factor();
        let pos = Vec2::new(x * scale, y * scale);
        let mods = Modifiers { shift, ctrl, alt };
        let button = match button {
            0 => PointerButton::Left,
            1 => PointerButton::Right,
            2 => PointerButton::Middle,
            _ => PointerButton::Other,
        };
        let input = match kind {
            0 => ViewportInput::PointerDown { button, pos, mods },
            1 => ViewportInput::PointerUp { button, pos, mods },
            2 => ViewportInput::PointerMove { pos, mods },
            _ => ViewportInput::PointerLeave,
        };
        a.borrow().send(ViewportRequest::PartBoxInput(input));
    });

    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_part_box_scroll(move |dx, dy, x, y| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let scale = ui.window().scale_factor();
        a.borrow().send(ViewportRequest::PartBoxInput(ViewportInput::Scroll {
            delta: Vec2::new(dx * scale, dy * scale),
            pos: Vec2::new(x * scale, y * scale),
            mods: Modifiers::default(),
        }));
    });

    let ui_weak = ui.as_weak();
    let a = app.clone();
    ui.on_part_box_close(move || {
        if let Some(ui) = ui_weak.upgrade() {
            close(&ui, &a);
        }
    });
}

/// Shows the newest box texture and hands every older one back (as `bridge::show_newest_frame`).
pub fn show_newest_frame(ui: &AppWindow, app: &Rc<RefCell<AppState>>, textures: &async_channel::Receiver<wgpu::Texture>) {
    let a = app.borrow();
    let release = |texture| a.send(ViewportRequest::ReleasePartBoxTexture(texture));
    let mut newest = None;
    while let Ok(texture) = textures.try_recv() {
        if let Some(older) = newest.replace(texture) {
            release(older);
        }
    }
    let Some(texture) = newest else { return };
    if !ui.get_part_box_open() {
        return release(texture);
    }
    let previous = ui.get_part_box_texture().to_wgpu_30_texture();
    match slint::Image::try_from(texture.clone()) {
        Ok(image) => {
            ui.set_part_box_texture(image);
            if let Some(previous) = previous {
                release(previous);
            }
        }
        Err(error) => {
            eprintln!("part box frame could not be shown: {error:?}");
            release(texture);
        }
    }
}
