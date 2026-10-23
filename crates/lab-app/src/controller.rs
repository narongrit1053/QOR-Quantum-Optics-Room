//! Callback wiring, the edit pipeline, undo/redo, selection, toasts and refresh (M1-SPEC §7.5).
//!
//! **Borrow rule.** A callback borrows the state, mutates it, collects follow-ups, drops the
//! borrow, and only then calls into Slint or sends viewport requests. A borrow is never held
//! across `invoke_from_event_loop`, a dialog, or a Slint call that can re-enter.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lab_app::{
    EditContext, EditError, EditOutcome, PreferenceChange, PreviewTarget, Session, corner_labels, edit, node_view,
    outline_preview, platform_view, preset_rows, project_detail, room_view, scene_rows, wall_labels, window_title,
};
use lab_core::glam::DVec2;
use lab_core::{
    ChangeSet, Command, LengthUnit, NodeId, OpeningFix, Outline, OutlineCorner, PlatformId, SelectionTarget,
    WallDisplay,
};
use lab_ui::AppWindow;
use lab_viewport::{DragUnit, FrameTarget, Tool, ViewMode, ViewportRequest};
use slint::{ComponentHandle, SharedString};

use crate::app_state::{AppState, SheetKind};
use crate::files;
use crate::ui_models::{self, UiModels};

/// Wires every AppWindow callback except the viewport input ones (`bridge::connect_input`).
pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    crate::wizard::connect(ui, app, models);
    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_field_committed(move |id, text| {
        let Some(ui) = w.upgrade() else { return false };
        on_field_committed(&ui, &a, &m, &id, &text)
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_field_edited(move |id, text| {
        let Some(ui) = w.upgrade() else { return };
        on_field_edited(&ui, &a, &m, &id, &text);
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_choice_selected(move |id, index| {
        let Some(ui) = w.upgrade() else { return };
        on_choice_selected(&ui, &a, &m, &id, index);
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_action(move |id| {
        let Some(ui) = w.upgrade() else { return };
        run_action(&ui, &a, &m, &id);
    });

    crate::import::connect(ui, app, models);
    crate::palette::connect(ui, app, models);

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_paste_corners(move |target, text| {
        let Some(ui) = w.upgrade() else { return SharedString::new() };
        on_paste_corners(&ui, &a, &m, &target, &text).into()
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_sheet_button(move |button| {
        let Some(ui) = w.upgrade() else { return };
        let kind = a.borrow().sheet;
        match kind {
            Some(SheetKind::Message) | None => close_sheet(&ui, &mut a.borrow_mut()),
            Some(_) => files::handle_sheet_button(&ui, &a, &m, button),
        }
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_toast_action_clicked(move || {
        let Some(ui) = w.upgrade() else { return };
        let undo = std::mem::take(&mut a.borrow_mut().toast_undo);
        hide_toast(&ui);
        if undo {
            undo_redo(&ui, &a, &m, false);
        }
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_scene_row_clicked(move |kind, id| {
        let Some(ui) = w.upgrade() else { return };
        let target = match kind {
            0 if id >= 0 => SelectionTarget::Node(NodeId(id as u64)),
            1 if id >= 0 => SelectionTarget::Platform(PlatformId(id as u64)),
            2 => SelectionTarget::Room,
            _ => return,
        };
        select(&ui, &a, &m, Some(target));
    });

    // M9 E2: typed value during a drag; Enter applies it exactly and ends the drag.
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_drag_value_key(move |key| {
        let Some(ui) = w.upgrade() else { return false };
        let mut text = ui.get_drag_value().to_string();
        match key.as_str() {
            "\n" | "\r" => {
                if let Ok(v) = text.parse::<f64>() {
                    a.borrow().send(ViewportRequest::DragValue(v));
                    ui.set_drag_value_open(false);
                }
                return true;
            }
            "\u{1b}" => text.clear(),
            "\u{8}" => {
                text.pop();
            }
            k if k == "." || k == "-" || (k.len() == 1 && k.chars().all(|c| c.is_ascii_digit())) => text.push_str(k),
            _ => return false,
        }
        ui.set_drag_value(text.into());
        true
    });

    let (w, a, m) = (ui.as_weak(), app.clone(), models.clone());
    ui.on_clear_selection(move || {
        let Some(ui) = w.upgrade() else { return };
        select(&ui, &a, &m, None);
    });

    let a = app.clone();
    ui.on_view_mode_requested(move |mode| {
        let mode = match mode {
            1 => ViewMode::Top,
            2 => ViewMode::Split,
            _ => ViewMode::Perspective,
        };
        a.borrow().send(ViewportRequest::SetViewMode(mode));
    });
    crate::view_cube::wire(ui, app);
    app.borrow().send(ViewportRequest::SetReduceMotion(reduce_motion()));
}

/// Windows "Show animations in Windows" off (`SPI_GETCLIENTAREAANIMATION` false), read once.
#[cfg(windows)]
fn reduce_motion() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, pv: *mut core::ffi::c_void, win_ini: u32) -> i32;
    }
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    let mut on: i32 = 1;
    // SAFETY: the action writes one BOOL into `on`, which outlives the call.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0) };
    ok != 0 && on == 0
}

#[cfg(not(windows))]
fn reduce_motion() -> bool {
    false
}

/// Copies the whole state into the window: title, undo state, preferences, scene, inspector.
pub fn refresh_all(ui: &AppWindow, app: &AppState, models: &UiModels) {
    refresh_document(ui, app, models);
    refresh_inspector(ui, app, models);
    crate::overlay::refresh(ui, app);
    ui.set_busy(app.busy);
}

/// The isolate banner's platform name.
pub fn refresh_isolated(ui: &AppWindow, app: &AppState) {
    let name = app.isolated.and_then(|id| app.session.doc.platform(id)).map(|p| p.name.clone());
    let name = name.or(app.isolated_nodes.then(|| "Selection".to_string()));
    ui.set_isolated_name(name.unwrap_or_default().into());
}

/// Title, dirty state, notes, undo/redo, preferences and scene rows (everything but the inspector).
fn refresh_document(ui: &AppWindow, app: &AppState, models: &UiModels) {
    let session = &app.session;
    let title = session.title();
    let dirty = session.is_dirty();
    ui.set_window_title(window_title(&title, dirty).into());
    ui.set_project_name(title.into());
    ui.set_project_detail(project_detail(&session.doc).into());
    ui.set_dirty(dirty);
    let note = if !app.busy_note.is_empty() {
        app.busy_note.as_str()
    } else if session.recovered {
        "Recovered — not saved yet"
    } else if session.migrated_from.is_some() {
        "Converted from an older format"
    } else {
        ""
    };
    ui.set_document_note(note.into());
    refresh_isolated(ui, app);

    let history = &session.history;
    ui.set_can_undo(history.can_undo());
    ui.set_can_redo(history.can_redo());
    ui.set_undo_label(history.undo_label().map_or("Undo".into(), |l| format!("Undo {l}")).into());
    ui.set_redo_label(history.redo_label().map_or("Redo".into(), |l| format!("Redo {l}")).into());

    ui.set_display_unit(unit_index(app.units()));
    ui.set_wall_display(wall_index(app.settings.wall_display));
    ui.set_show_beams(app.settings.show_beams);
    ui_models::fill_presets(models, preset_rows(app.units()));
    ui_models::fill_scene(models, scene_rows(&session.doc, session.selection, &app.scene_collapsed));
    crate::relations::fill(ui, &session.doc);
}

/// The inspector for the current selection, including every choice property.
fn refresh_inspector(ui: &AppWindow, app: &AppState, models: &UiModels) {
    crate::joint::fill(ui, app, models);
    let doc = &app.session.doc;
    let units = app.units();
    let kind = match app.session.selection {
        Some(SelectionTarget::Node(id)) => match doc.node(id) {
            Some(node) => {
                let holes = doc.root_platform(id).and_then(|p| doc.platform(p)).map(|p| app.holes_for(p));
                ui_models::fill_node(ui, models, node_view(doc, &app.catalog, node, holes.as_deref(), units));
                ui_models::sync(&models.constraint_rows, crate::constrain::rows(node, units));
                crate::beams::fill(ui, app, models, id);
                1
            }
            None => 0,
        },
        Some(SelectionTarget::Platform(id)) => match doc.platform(id) {
            Some(platform) => {
                let holes = app.holes_for(platform);
                ui_models::fill_platform(ui, models, platform_view(doc, platform, &holes, units));
                2
            }
            None => 0,
        },
        Some(SelectionTarget::Room) => {
            ui_models::fill_room(ui, models, room_view(doc, units));
            3
        }
        None => 0,
    };
    ui.set_selection_kind(kind);
}

/// Refreshes everything after a state change (updating the hole cache first).
pub(crate) fn refresh(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels) {
    app.borrow_mut().update_hole_cache();
    crate::beams::retrace(&mut app.borrow_mut());
    refresh_all(ui, &app.borrow(), models);
}

/// Replaces the open document: sends `LoadDocument` + `Frame(All)`, clears the selection, refreshes.
#[allow(dead_code)] // Used by the file flows (T6b).
pub fn replace_document(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, session: Session) {
    let snapshot = {
        let mut a = app.borrow_mut();
        a.session = session;
        a.session.selection = None;
        a.hole_cache.clear();
        a.toast_undo = false;
        a.timers.autosave.stop();
        Box::new(a.session.doc.clone())
    };
    {
        let a = app.borrow();
        a.send(ViewportRequest::LoadDocument(snapshot));
        a.send(ViewportRequest::Select(None));
        a.send(ViewportRequest::Frame(FrameTarget::All));
    }
    ui.set_inspector_error("".into());
    ui.set_paste_target("".into());
    refresh(ui, app, models);
}

/// M9 D. The tool step hint at the bottom centre of the viewport; empty hides it.
pub fn set_hint(ui: &AppWindow, text: &str) {
    ui.set_tool_hint(text.into());
}

/// Shows a single-shot toast: 2.4 s, or 5 s with an Undo button. The caller sets
/// `AppState::toast_undo` to match `undo`.
/// M9 E2. A drag began: open the empty numeric field in the drag's unit.
pub fn on_drag_started(ui: &AppWindow, unit: DragUnit) {
    ui.set_drag_value("".into());
    ui.set_drag_value_unit(match unit {
        DragUnit::Mm => "mm",
        DragUnit::Degrees => "°",
    }
    .into());
    ui.set_drag_value_open(true);
}

pub fn show_toast(ui: &AppWindow, app: &AppState, text: &str, undo: bool) {
    ui.set_toast(text.into());
    ui.set_toast_action(if undo { "Undo" } else { "" }.into());
    let ui_weak = ui.as_weak();
    let duration = Duration::from_millis(if undo { 5000 } else { 2400 });
    app.timers.toast.start(slint::TimerMode::SingleShot, duration, move || {
        if let Some(ui) = ui_weak.upgrade() {
            hide_toast(&ui);
        }
    });
}

fn hide_toast(ui: &AppWindow) {
    ui.set_toast("".into());
    ui.set_toast_action("".into());
}

/// Opens a modal sheet. `default` is the Return button, `destructive` is -1 for none.
#[allow(clippy::too_many_arguments)]
pub fn show_sheet(
    ui: &AppWindow,
    app: &mut AppState,
    kind: SheetKind,
    title: &str,
    message: &str,
    buttons: &[&str],
    default: i32,
    destructive: i32,
) {
    app.sheet = Some(kind);
    match ui_models::vec_model(&ui.get_sheet_buttons()) {
        Some(model) => ui_models::sync(model, buttons.iter().map(|b| SharedString::from(*b))),
        None => ui.set_sheet_buttons(slint::ModelRc::new(slint::VecModel::from(
            buttons.iter().map(|b| SharedString::from(*b)).collect::<Vec<_>>(),
        ))),
    }
    ui.set_sheet_title(title.into());
    ui.set_sheet_message(message.into());
    ui.set_sheet_default(default);
    ui.set_sheet_destructive(destructive);
    ui.set_sheet_open(true);
}

/// Closes the open sheet (any kind).
pub fn close_sheet(ui: &AppWindow, app: &mut AppState) {
    app.sheet = None;
    ui.set_sheet_open(false);
}

// ── Edit pipeline ──────────────────────────────────────────────────────────────────────────

fn on_field_committed(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str, text: &str) -> bool {
    if id.starts_with("joint.") {
        return crate::joint::field_committed(ui, app, models, id, text);
    }
    if id.starts_with("cpanel.") {
        return crate::constrain::panel_field(ui, app, models, id, text);
    }
    let outcome = {
        let a = app.borrow();
        if let Some((node, field)) = set_field(id) {
            let cx = EditContext { selection: Some(SelectionTarget::Node(node)), ..context(&a) };
            edit::node_field_edit(&cx, &a.catalog, field, text)
        } else if id.starts_with("beam.") {
            crate::beams::field_edit(&a, id, text)
        } else if id.starts_with("constraint.") {
            crate::constrain::field_edit(&a, id, text)
        } else if id.starts_with("node.") {
            edit::node_field_edit(&context(&a), &a.catalog, id, text)
        } else {
            edit::field_edit(&context(&a), id, text)
        }
    };
    apply_outcome(ui, app, models, id, outcome)
}

/// `set.<node id>:<node field>` (a Set row field) → (node, field).
fn set_field(id: &str) -> Option<(NodeId, &str)> {
    let (node, field) = id.strip_prefix("set.")?.split_once(':')?;
    Some((NodeId(node.parse().ok()?), field))
}

fn on_choice_selected(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str, index: i32) {
    let outcome = {
        let a = app.borrow();
        edit::choice_edit(&context(&a), id, index)
    };
    if !apply_outcome(ui, app, models, id, outcome) {
        return;
    }
    // Controlled choices (D22): the property must equal the document even when nothing
    // changed, so the thumb never stays where the user clicked unless the value moved there.
    refresh_all(ui, &app.borrow(), models);
}

pub(crate) fn run_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str) {
    match id {
        "edit.undo" => undo_redo(ui, app, models, false),
        "edit.redo" => undo_redo(ui, app, models, true),
        "view.frame_all" => {
            // In Top, F frames the selected platform (POLA-49).
            let a = app.borrow();
            let target = match a.session.selection {
                Some(SelectionTarget::Platform(id)) if ui.get_view_mode() == 1 => FrameTarget::Platform(id),
                _ => FrameTarget::All,
            };
            a.send(ViewportRequest::Frame(target));
        }
        "view.blueprint" => {
            let on = !ui.get_blueprint();
            ui.set_blueprint(on);
            app.borrow().send(ViewportRequest::SetBlueprint(on));
        }
        "view.beams" => {
            let settings = {
                let mut a = app.borrow_mut();
                a.settings.show_beams = !a.settings.show_beams;
                a.send(ViewportRequest::SetBeams(a.settings.show_beams));
                a.settings.clone()
            };
            ui.set_show_beams(settings.show_beams);
            if let Err(error) = lab_file::save_settings(&settings) {
                eprintln!("settings not saved: {error}");
            }
        }
        "beam.toggle" => {
            let outcome = crate::beams::toggle(&app.borrow());
            apply_outcome(ui, app, models, id, outcome);
        }
        "view.frame_selection" => app.borrow().send(ViewportRequest::Frame(FrameTarget::Selection)),
        "view.isolate" => {
            let app = app.borrow();
            let platform = match app.session.selection {
                Some(SelectionTarget::Platform(id)) => Some(id),
                Some(SelectionTarget::Node(id)) => app.session.doc.root_platform(id),
                _ => None,
            };
            if platform.is_some() {
                app.send(ViewportRequest::Isolate(platform));
            }
        }
        "view.theme.dark" | "view.theme.light" | "view.theme.contrast" => {
            let mode = match id {
                "view.theme.light" => lab_core::ThemeMode::Light,
                "view.theme.contrast" => lab_core::ThemeMode::HighContrast,
                _ => lab_core::ThemeMode::Dark,
            };
            crate::apply_theme(ui, mode);
            let settings = {
                let mut a = app.borrow_mut();
                a.settings.theme = mode;
                a.settings.clone()
            };
            if let Err(error) = lab_file::save_settings(&settings) {
                eprintln!("settings not saved: {error}");
            }
        }
        "view.show_all" => {
            let mut a = app.borrow_mut();
            a.hidden.clear();
            a.isolated_nodes = false;
            a.send(ViewportRequest::SetHidden(Vec::new()));
            a.send(ViewportRequest::IsolateNodes(None));
            a.send(ViewportRequest::Isolate(None));
        }
        "view.part_box" => {
            // Opens the part box with the Constrain Components panel (POLA-147).
            run_action(ui, app, models, "tool.constrain");
        }
        "view.hide" => {
            let mut a = app.borrow_mut();
            match a.session.selection {
                Some(SelectionTarget::Node(n)) => {
                    if !a.hidden.remove(&n) {
                        a.hidden.insert(n);
                    }
                }
                _ => a.hidden.clear(),
            }
            let ids = a.hidden.iter().copied().collect();
            a.send(ViewportRequest::SetHidden(ids));
        }
        "view.isolate_selection" => {
            let Some(SelectionTarget::Node(n)) = app.borrow().session.selection else { return };
            {
                let mut a = app.borrow_mut();
                a.isolated_nodes = true;
                a.send(ViewportRequest::IsolateNodes(Some(vec![n])));
            }
            refresh_isolated(ui, &app.borrow());
        }
        "view.walls" => {
            let mut a = app.borrow_mut();
            a.walls_hidden = !a.walls_hidden;
            let display = if a.walls_hidden { WallDisplay::Hidden } else { a.settings.wall_display };
            a.send(ViewportRequest::SetWallDisplay(display));
        }
        "view.context_menu" => {
            let (target, (w, h)) = {
                let a = app.borrow();
                (a.session.selection, a.viewport_size)
            };
            // shortcut: opens at the viewport centre, not the node's projected origin; needs a projection query to the viewport.
            crate::context_menu::open(ui, target, lab_viewport::Vec2::new(w as f32 / 2.0, h as f32 / 2.0));
        }
        "view.top" | "view.3d" => {
            let top = id == "view.top";
            ui.set_view_mode(i32::from(top));
            let mode = if top { ViewMode::Top } else { ViewMode::Perspective };
            app.borrow().send(ViewportRequest::SetViewMode(mode));
        }
        "view.split" => {
            ui.set_view_mode(2);
            app.borrow().send(ViewportRequest::SetViewMode(ViewMode::Split));
        }
        "palette.open" => ui.set_palette_open(true),
        "help.shortcuts" => {
            let rows = crate::actions::ACTIONS
                .iter()
                .filter(|a| !a.shortcut.is_empty())
                .map(|a| lab_ui::PaletteItem { id: a.id.into(), title: a.title.into(), group: a.group.into(), shortcut: a.shortcut.into() })
                .chain(NUDGE_KEYS.iter().map(|(title, keys)| lab_ui::PaletteItem {
                    id: "".into(),
                    title: (*title).into(),
                    group: "Edit".into(),
                    shortcut: (*keys).into(),
                }))
                .collect::<Vec<_>>();
            ui.set_shortcut_items(slint::ModelRc::new(slint::VecModel::from(rows)));
            ui.set_shortcuts_open(true);
        }
        "set.select_root" => {
            let root = match app.borrow().session.selection {
                Some(SelectionTarget::Node(id)) => lab_app::set_root(&app.borrow().session.doc, id),
                _ => return,
            };
            select(ui, app, models, Some(SelectionTarget::Node(root)));
        }
        id if id.starts_with("set.select.") => {
            if let Ok(n) = id["set.select.".len()..].parse() {
                select(ui, app, models, Some(SelectionTarget::Node(NodeId(n))));
            }
        }
        "inspector.dismiss_error" => {
            ui.set_inspector_error("".into());
            // Drops any live-preview state (corner tint, previewed outline).
            refresh_all(ui, &app.borrow(), models);
        }
        "import.step" => crate::import::choose_file(ui, app, models),
        "import.step_insert" => crate::import::insert_dialog(ui, app, models),
        "paste.cancel" => ui.set_paste_target("".into()),
        "tool.select" | "tool.move" | "tool.rotate" | "tool.measure" | "tool.constrain" | "tool.joint" => {
            let (index, tool) = match id {
                "tool.move" => (1, Tool::Move),
                "tool.rotate" => (2, Tool::Rotate),
                "tool.measure" => (3, Tool::Measure),
                "tool.constrain" => (4, Tool::Constrain),
                "tool.joint" => (5, Tool::Joint),
                _ => (0, Tool::Select),
            };
            ui.set_tool(index);
            crate::constrain::reset(ui, app, models, tool == Tool::Constrain);
            crate::joint::reset(ui, app, models, tool == Tool::Joint);
            if tool == Tool::Measure {
                set_hint(ui, "Click the first point");
            }
            app.borrow().send(ViewportRequest::SetTool(tool));
        }
        "edit.copy" | "edit.cut" | "edit.paste" | "edit.duplicate" => {
            let pasted = {
                let mut a = app.borrow_mut();
                let selected = match a.session.selection {
                    Some(SelectionTarget::Node(n)) => vec![n],
                    _ => Vec::new(),
                };
                if id != "edit.paste" && selected.is_empty() {
                    return;
                }
                let copied = crate::clipboard::copy(&a.session.doc, &selected);
                if id == "edit.duplicate" {
                    crate::clipboard::paste(&a.session.doc, &copied)
                } else {
                    if id != "edit.paste" {
                        a.clipboard = copied;
                    }
                    if id == "edit.paste" { crate::clipboard::paste(&a.session.doc, &a.clipboard) } else { None }
                }
            };
            if id == "edit.cut" {
                run_action(ui, app, models, "selection.delete");
            } else if let Some((command, roots)) = pasted {
                let label = if id == "edit.paste" { "Paste" } else { "Duplicate" };
                execute(ui, app, models, id, command, label.into(), roots.first().map(|r| SelectionTarget::Node(*r)), Vec::new());
            }
        }
        _ if id.starts_with("nudge.") || id == "tool.rotate_left" || id == "tool.rotate_right" || id == "edit.rotate_90" => {
            let how = match id {
                "edit.rotate_90" => crate::nudge::Nudge::Turn { degrees: 90.0 },
                "tool.rotate_left" => crate::nudge::Nudge::Turn { degrees: -15.0 },
                "tool.rotate_right" => crate::nudge::Nudge::Turn { degrees: 15.0 },
                _ => {
                    // "nudge.step:dx:dy" (holes) or "nudge.fine:dx:dy" (mm).
                    let mut parts = id["nudge.".len()..].split(':');
                    let kind = parts.next().unwrap_or("");
                    let mut d = || parts.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                    let (dx, dy) = (d(), d());
                    if kind == "fine" {
                        crate::nudge::Nudge::Fine { dx: f64::from(dx), dy: f64::from(dy) }
                    } else {
                        crate::nudge::Nudge::Step { dx, dy }
                    }
                }
            };
            let Some(SelectionTarget::Node(node)) = app.borrow().session.selection else { return };
            let label = if matches!(how, crate::nudge::Nudge::Turn { .. }) { "Rotate" } else { "Nudge" };
            let result = crate::nudge::nudge(&app.borrow().session.doc, &[node], how);
            match result {
                Ok(Some(command)) => {
                    execute(ui, app, models, id, command, label.into(), None, Vec::new());
                }
                Ok(None) => {}
                Err(error) => {
                    let text = if error == crate::nudge::NudgeError::Locked { "Part is locked" } else { "No hole there" };
                    show_toast(ui, &app.borrow(), text, false);
                }
            }
        }
        _ if id.starts_with("file.") => files::handle_file_action(ui, app, models, id),
        "tool.ground" => crate::joint::ground(ui, app, models),
        "tool.rigid_group" => crate::joint::rigid(ui, app, models),
        _ if id.starts_with("joint.") => crate::joint::action(ui, app, models, id),
        _ if id.starts_with("cpanel.") => crate::constrain::panel_action(ui, app, models, id),
        _ if id.starts_with("relation.") => crate::relations::run(ui, app, models, id),
        _ if id.starts_with("constraint.") => {
            let outcome = crate::constrain::action_edit(&app.borrow(), id);
            apply_outcome(ui, app, models, id, outcome);
        }
        _ => {
            let outcome = {
                let a = app.borrow();
                edit::action_edit(&context(&a), id)
            };
            apply_outcome(ui, app, models, id, outcome);
        }
    }
}

fn context(a: &AppState) -> EditContext<'_> {
    EditContext { doc: &a.session.doc, selection: a.session.selection, units: a.units() }
}

/// Applies an edit outcome. Returns `false` when it was refused (the field keeps its text).
fn apply_outcome(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    id: &str,
    outcome: Result<EditOutcome, EditError>,
) -> bool {
    match outcome {
        Err(error) => {
            refuse(ui, app, models, id, &error.to_string(), lab_app::invalid_row(&error));
            false
        }
        Ok(EditOutcome::Nothing) => true,
        Ok(EditOutcome::Preference(change)) => {
            apply_preference(ui, app, models, change);
            true
        }
        Ok(EditOutcome::OpenPaste(target)) => {
            ui.set_paste_target(target.into());
            true
        }
        Ok(EditOutcome::Command { command, label, select, fixes }) => {
            execute(ui, app, models, id, command, label, select, fixes)
        }
    }
}

/// Shows a refusal: the banner, the inspector refreshed to the document (choices included),
/// and the offending corner row tinted for corner edits.
fn refuse(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels, id: &str, message: &str, row: Option<usize>) {
    refresh_all(ui, &app.borrow(), models);
    ui.set_inspector_error(message.into());
    let row = row.map_or(-1, |r| r as i32);
    if id.starts_with("platform.corner") {
        ui.set_platform_corner_invalid(row);
    } else if id.starts_with("room.corner") {
        ui.set_room_corner_invalid(row);
    }
}

/// Arrow-key nudge rows for the shortcut sheet (they are not palette actions).
const NUDGE_KEYS: [(&str, &str); 3] = [
    ("Nudge selection one hole", "Arrows"),
    ("Nudge selection 5 mm", "Shift+Arrows"),
    ("Nudge selection 1 mm", "Alt+Arrows"),
];

fn apply_preference(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels, change: PreferenceChange) {
    let (settings, walls) = {
        let mut a = app.borrow_mut();
        let walls = match change {
            PreferenceChange::Units(unit) => {
                a.settings.display_unit = unit;
                crate::measure::refresh(ui, unit);
                crate::stack::fill(ui, &mut a);
                None
            }
            PreferenceChange::Walls(display) => {
                a.settings.wall_display = display;
                Some(display)
            }
        };
        (a.settings.clone(), walls)
    };
    if let Some(display) = walls {
        app.borrow().send(ViewportRequest::SetWallDisplay(display));
    }
    if let Err(error) = lab_file::save_settings(&settings) {
        eprintln!("settings not saved: {error}");
    }
    refresh_all(ui, &app.borrow(), models);
}

/// Runs a command through the history (edit flow step 5).
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    id: &str,
    command: Command,
    label: String,
    select: Option<SelectionTarget>,
    fixes: Vec<OpeningFix>,
) -> bool {
    let result = {
        let mut a = app.borrow_mut();
        // Captured before the command removes it, for the toast.
        let deleted = match &command {
            Command::RemovePlatform(pid) => {
                a.session.doc.platform(*pid).map(|p| (p.name.clone(), a.session.doc.nodes_on(*pid).count()))
            }
            _ => None,
        };
        let before = a.session.selection;
        match a.session.execute(command, Some(label)) {
            Ok(changes) => {
                if let Some(target) = select.filter(|t| t.exists_in(&a.session.doc)) {
                    a.session.selection = Some(target);
                }
                a.session.fix_selection();
                let selection = a.session.selection;
                let snapshot = (!changes.is_empty()).then(|| Box::new(a.session.doc.clone()));
                let toast = toast_for(&fixes, deleted);
                a.toast_undo = toast.is_some();
                Ok((snapshot, changes, (selection != before || select.is_some()).then_some(selection), toast))
            }
            Err(error) => Err(error),
        }
    };
    match result {
        Ok((snapshot, changes, selection, toast)) => {
            ui.set_inspector_error("".into());
            {
                let a = app.borrow();
                if let Some(snapshot) = snapshot {
                    a.send(ViewportRequest::ApplyChanges { snapshot, changes });
                }
                if let Some(selection) = selection {
                    a.send(ViewportRequest::Select(selection));
                }
            }
            refresh(ui, app, models);
            if let Some(text) = toast {
                show_toast(ui, &app.borrow(), &text, true);
            }
            files::arm_autosave(app);
            true
        }
        Err(error) => {
            refuse(ui, app, models, id, &error.to_string(), None);
            false
        }
    }
}

/// The Undo toast for door refits or a platform deletion, if any.
fn toast_for(fixes: &[OpeningFix], deleted: Option<(String, usize)>) -> Option<String> {
    if let Some((name, parts)) = deleted {
        return Some(match parts {
            0 => format!("Deleted {name}"),
            1 => format!("Deleted {name} and 1 part"),
            n => format!("Deleted {name} and {n} parts"),
        });
    }
    let text = fixes
        .iter()
        .map(|fix| match fix {
            OpeningFix::Moved { name, wall, .. } => format!("Moved {name} to fit Wall {}", wall + 1),
            OpeningFix::Removed { name, .. } => format!("Removed {name}, which no longer fits"),
        })
        .collect::<Vec<_>>();
    (!text.is_empty()).then(|| text.join(" · "))
}

fn undo_redo(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, redo: bool) {
    let outcome = {
        let mut a = app.borrow_mut();
        let history = &a.session.history;
        let label = if redo { history.redo_label() } else { history.undo_label() }.map(str::to_owned);
        let result = if redo { a.session.redo() } else { a.session.undo() };
        match result {
            None => return,
            Some(Ok(changes)) => {
                let before = a.session.selection;
                if let Some(target) = changes.single_target()
                    && Some(target) != a.session.selection
                    && target.exists_in(&a.session.doc)
                {
                    a.session.selection = Some(target);
                }
                a.session.fix_selection();
                let selection = a.session.selection;
                a.toast_undo = false;
                Ok((Box::new(a.session.doc.clone()), changes, (selection != before).then_some(selection), label))
            }
            Some(Err(error)) => {
                // The document is whatever the failed step left; drop the history and resync.
                a.session.history.clear();
                a.session.fix_selection();
                Err((Box::new(a.session.doc.clone()), error))
            }
        }
    };
    match outcome {
        Ok((snapshot, changes, selection, label)) => {
            ui.set_inspector_error("".into());
            {
                let a = app.borrow();
                send_changes(&a, snapshot, changes);
                if let Some(selection) = selection {
                    a.send(ViewportRequest::Select(selection));
                }
            }
            refresh(ui, app, models);
            let text = match (redo, label) {
                (false, Some(l)) => format!("Undid: {l}"),
                (true, Some(l)) => format!("Redid: {l}"),
                (false, None) => "Undid".into(),
                (true, None) => "Redid".into(),
            };
            show_toast(ui, &app.borrow(), &text, false);
            files::arm_autosave(app);
        }
        Err((snapshot, error)) => {
            eprintln!("history failure: {error}");
            app.borrow().send(ViewportRequest::LoadDocument(snapshot));
            refresh(ui, app, models);
            show_sheet(
                ui,
                &mut app.borrow_mut(),
                SheetKind::Message,
                "Undo history was reset",
                "Undo history was reset because of an internal error.",
                &["OK"],
                0,
                -1,
            );
            files::arm_autosave(app);
        }
    }
}

fn send_changes(a: &AppState, snapshot: Box<lab_core::Document>, changes: ChangeSet) {
    if !changes.is_empty() {
        a.send(ViewportRequest::ApplyChanges { snapshot, changes });
    }
}

// ── Live preview and paste ─────────────────────────────────────────────────────────────────

/// Live preview of a corner or zone cell: only the preview, its labels, the row tint and the
/// banner change. No command is issued; the 3D view updates on commit.
fn on_field_edited(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, id: &str, text: &str) {
    if !(id.contains(".corner.") || id.contains(".zone.")) {
        return;
    }
    let preview = {
        let a = app.borrow();
        edit::preview_edit(&context(&a), id, text)
    };
    let room = id.starts_with("room.");
    match preview {
        Ok(PreviewTarget::Platform(platform)) => {
            let flat = platform.outline_path().map(|f| f.points).unwrap_or_default();
            let zones = platform.hole_grid.as_ref().map(|g| g.exclusions.as_slice()).unwrap_or(&[]);
            let preview = outline_preview(&flat, zones, &[]);
            let corners: Vec<DVec2> = platform.outline.corners().iter().map(|c| c.point).collect();
            let labels = corner_labels(&corners, &preview);
            ui_models::fill_platform_preview(ui, models, preview, labels);
            ui.set_platform_corner_invalid(-1);
            ui.set_inspector_error("".into());
        }
        Ok(PreviewTarget::Room(new_room)) => {
            let preview = outline_preview(&new_room.outline, &[], &[]);
            let mut labels = corner_labels(&new_room.outline, &preview);
            labels.extend(wall_labels(&new_room, &preview));
            ui_models::fill_room_preview(ui, models, preview, labels);
            ui.set_room_corner_invalid(-1);
            ui.set_inspector_error("".into());
        }
        Err((error, row)) => {
            ui.set_inspector_error(error.to_string().into());
            if id.contains(".corner.") {
                let row = row.map_or(-1, |r| r as i32);
                if room {
                    ui.set_room_corner_invalid(row);
                } else {
                    ui.set_platform_corner_invalid(row);
                }
            }
        }
    }
}

/// Applies pasted corners to the selected platform (converted to Custom) or the room.
/// Returns "" when applied (the sheet closes) or the error text (the sheet stays open).
fn on_paste_corners(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, target: &str, text: &str) -> String {
    let built = {
        let a = app.borrow();
        build_paste(&a, target, text)
    };
    let (command, label, fixes) = match built {
        Ok(built) => built,
        Err(message) => return message,
    };
    if execute(ui, app, models, &format!("{target}.corners.paste"), command, label, None, fixes) {
        ui.set_paste_target("".into());
        String::new()
    } else {
        // `refuse` showed the reason in the inspector banner; repeat it in the sheet.
        ui.get_inspector_error().to_string()
    }
}

fn build_paste(a: &AppState, target: &str, text: &str) -> Result<(Command, String, Vec<OpeningFix>), String> {
    let doc = &a.session.doc;
    let units: LengthUnit = a.units();
    match target {
        "platform" => {
            let Some(SelectionTarget::Platform(id)) = a.session.selection else {
                return Err(EditError::Stale.to_string());
            };
            let platform = doc.platform(id).ok_or_else(|| EditError::Stale.to_string())?;
            let corners: Vec<OutlineCorner> = edit::parse_pasted_corners(text, units, true)?;
            let mut new = platform.clone();
            new.outline = Outline::Polygon { corners };
            new.validate().map_err(|e| e.to_string())?;
            Ok((Command::UpdatePlatform { platform: Box::new(new) }, "Paste Corners".into(), Vec::new()))
        }
        "room" => {
            let corners = edit::parse_pasted_corners(text, units, false)?;
            let points: Vec<DVec2> = corners.iter().map(|c| c.point).collect();
            let mut room = doc.room.clone();
            room.outline = points;
            let fixes = room.refit_openings();
            room.validate().map_err(|e| e.to_string())?;
            Ok((Command::SetRoom(Box::new(room)), "Paste Corners".into(), fixes))
        }
        _ => Err(format!("Unknown paste target {target}")),
    }
}

// ── Selection ──────────────────────────────────────────────────────────────────────────────

/// Selection set from the UI (scene row click, clear): stored locally, sent, refreshed.
fn select(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels, target: Option<SelectionTarget>) {
    let changed = {
        let mut a = app.borrow_mut();
        let target = target.filter(|t| t.exists_in(&a.session.doc));
        let changed = a.session.selection != target;
        a.session.selection = target;
        a.send(ViewportRequest::Select(target));
        changed
    };
    if changed {
        ui.set_inspector_error("".into());
    }
    refresh(ui, app, models);
}

/// `ViewportEvent::SelectionChanged`: validated against the document, then stored.
pub fn on_viewport_selection(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &UiModels,
    target: Option<SelectionTarget>,
) {
    let changed = {
        let mut a = app.borrow_mut();
        if let Some(t) = target
            && !t.exists_in(&a.session.doc)
        {
            a.send(ViewportRequest::Select(None));
            return;
        }
        let changed = a.session.selection != target;
        a.session.selection = target;
        changed
    };
    if !changed {
        return;
    }
    ui.set_inspector_error("".into());
    refresh(ui, app, models);
}

fn unit_index(unit: LengthUnit) -> i32 {
    match unit {
        LengthUnit::Millimetre => 0,
        LengthUnit::Inch => 1,
    }
}

fn wall_index(display: WallDisplay) -> i32 {
    match display {
        WallDisplay::Auto => 0,
        WallDisplay::Always => 1,
        WallDisplay::Hidden => 2,
    }
}
