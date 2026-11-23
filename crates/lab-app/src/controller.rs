//! Callback wiring, the edit pipeline, undo/redo, selection, toasts and refresh (M1-SPEC §7.5).
//!
//! **Borrow rule.** A callback borrows the state, mutates it, collects follow-ups, drops the
//! borrow, and only then calls into Slint or sends viewport requests. A borrow is never held
//! across `invoke_from_event_loop`, a dialog, or a Slint call that can re-enter.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use lab_app::multi_select::{self, Sel};
use lab_app::{
    EditContext, EditError, EditOutcome, PreferenceChange, PreviewTarget, Session, corner_labels, edit, node_view,
    outline_preview, platform_view, preset_rows, project_detail, room_view, scene_rows, wall_labels, window_title,
};
use lab_core::glam::DVec2;
use lab_core::{
    ChangeSet, Command, CommandError, GroupId, LengthUnit, NodeId, OpeningFix, Outline, OutlineCorner, PlatformId,
    SelectionTarget, WallDisplay, remove_nodes_command,
};
use lab_ui::AppWindow;
use lab_viewport::{DragUnit, FrameTarget, Tool, ViewMode, ViewportRequest};
use slint::{ComponentHandle, Model, SharedString};

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
    crate::catalog_book::connect(ui, app);
    crate::catalog_book::startup_sync(ui);
    crate::palette::connect(ui, app, models);
    crate::settings_ui::connect(ui, app, models);

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
            Some(SheetKind::DeleteBundle) => {
                let mut a = a.borrow_mut();
                close_sheet(&ui, &mut a);
                if button == 0 {
                    crate::library::delete_bundle(&ui, &mut a, &m);
                }
            }
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
    ui.on_scene_row_clicked(move |kind, id, ctrl, shift| {
        let Some(ui) = w.upgrade() else { return };
        let next = {
            let a = a.borrow();
            let doc = &a.session.doc;
            let prev = current_sel(&a.session);
            match kind {
                3 if id >= 0 => {
                    let Some(group) = doc.group(GroupId(id as u64)) else { return };
                    Sel::from_nodes(&group.members)
                }
                0 if id >= 0 => {
                    let nid = NodeId(id as u64);
                    if ctrl {
                        multi_select::toggle(&prev, nid)
                    } else if shift {
                        let rows: Vec<(i32, i32)> =
                            (0..m.scene.row_count()).filter_map(|i| m.scene.row_data(i)).map(|r| (r.kind, r.id)).collect();
                        multi_select::range(&rows, &prev, nid)
                    } else if prev.primary == Some(SelectionTarget::Node(nid)) && prev.extra.is_empty() {
                        // Clicking the sole selected row again clears the selection.
                        Sel::default()
                    } else {
                        multi_select::click(doc, &prev, Some(SelectionTarget::Node(nid)))
                    }
                }
                1 if id >= 0 => {
                    let target = SelectionTarget::Platform(PlatformId(id as u64));
                    let hit = (prev.primary != Some(target) || !prev.extra.is_empty()).then_some(target);
                    multi_select::click(doc, &prev, hit)
                }
                2 => {
                    let target = SelectionTarget::Room;
                    let hit = (prev.primary != Some(target) || !prev.extra.is_empty()).then_some(target);
                    multi_select::click(doc, &prev, hit)
                }
                _ => return,
            }
        };
        select_sel(&ui, &a, &m, next);
    });

    // M9 E2: typed value during a drag; Enter applies it exactly and ends the drag.
    let (w, a) = (ui.as_weak(), app.clone());
    ui.on_drag_value_key(move |key| {
        let Some(ui) = w.upgrade() else { return false };
        let mut text = ui.get_drag_value().to_string();
        match key.as_str() {
            "\n" | "\r" => {
                // Lengths are typed in the display unit; the drag takes mm.
                let value = if ui.get_drag_value_unit() == "°" {
                    text.parse::<f64>().ok()
                } else {
                    let units = a.borrow().units();
                    lab_core::units::parse_length(&text, units).ok()
                };
                if let Some(v) = value {
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
    crate::probe::refresh(ui, app);
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
    ui.set_show_builtin_parts(app.settings.show_builtin_parts);
    ui_models::fill_presets(models, preset_rows(app.units()));
    ui_models::fill_scene(models, scene_rows(&session.doc, session.selection, &session.extra, &app.scene_collapsed));
    crate::relations::fill(ui, &session.doc, session.selection);
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
        a.doc_token += 1;
        a.dragging = false;
        a.session.selection = None;
        a.session.extra.clear();
        a.hole_cache.clear();
        a.toast_undo = false;
        a.timers.autosave.stop();
        Box::new(a.session.doc.clone())
    };
    {
        let a = app.borrow();
        a.send(ViewportRequest::LoadDocument(snapshot));
        a.send(ViewportRequest::Select(None));
        a.send(ViewportRequest::SelectExtra(Vec::new()));
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
        DragUnit::Mm if ui.get_display_unit() == unit_index(LengthUnit::Inch) => "in",
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
        "library.show_builtin" => {
            let settings = {
                let mut a = app.borrow_mut();
                a.settings.show_builtin_parts = !a.settings.show_builtin_parts;
                crate::library::fill(&a, models);
                a.settings.clone()
            };
            ui.set_show_builtin_parts(settings.show_builtin_parts);
            if let Err(error) = lab_file::save_settings(&settings) {
                eprintln!("settings not saved: {error}");
            }
        }
        "library.open" => ui.set_library_open(!ui.get_library_open()),
        "library.save_bundle" => {
            let a = app.borrow();
            let selected = a.session.selected_nodes();
            crate::library::save_bundle_prompt(ui, &a, &selected);
        }
        "library.find" => {
            ui.set_library_open(true);
            ui.invoke_find_part();
        }
        "selection.delete" => {
            let outcome = {
                let a = app.borrow();
                let nodes = a.session.selected_nodes();
                if !nodes.is_empty() {
                    let command = remove_nodes_command(&a.session.doc, nodes);
                    let label = command.label(&a.session.doc);
                    Ok(EditOutcome::Command { command, label, select: None, fixes: Vec::new() })
                } else {
                    edit::action_edit(&context(&a), id)
                }
            };
            apply_outcome(ui, app, models, id, outcome);
        }
        "selection.lock" | "selection.unlock" => {
            let outcome = {
                let a = app.borrow();
                let on = id == "selection.lock";
                let nodes = a.session.selected_nodes();
                if !nodes.is_empty() {
                    let count = nodes.len();
                    let command = if count == 1 {
                        Command::SetSelectLock { target: SelectionTarget::Node(nodes[0]), on }
                    } else {
                        Command::Batch(
                            nodes
                                .into_iter()
                                .map(|n| Command::SetSelectLock { target: SelectionTarget::Node(n), on })
                                .collect(),
                        )
                    };
                    let label = if count == 1 {
                        command.label(&a.session.doc)
                    } else {
                        format!("{} {count} parts", if on { "Lock" } else { "Unlock" })
                    };
                    Ok(EditOutcome::Command { command, label, select: None, fixes: Vec::new() })
                } else if let Some(t @ SelectionTarget::Platform(_)) = a.session.selection {
                    let command = Command::SetSelectLock { target: t, on };
                    let label = command.label(&a.session.doc);
                    Ok(EditOutcome::Command { command, label, select: None, fixes: Vec::new() })
                } else {
                    Ok(EditOutcome::Nothing)
                }
            };
            apply_outcome(ui, app, models, id, outcome);
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
            let nodes = a.session.selected_nodes();
            if nodes.is_empty() {
                a.hidden.clear();
            } else {
                let hide = nodes.iter().any(|n| !a.hidden.contains(n));
                for n in nodes {
                    if hide {
                        a.hidden.insert(n);
                    } else {
                        a.hidden.remove(&n);
                    }
                }
            }
            let ids = a.hidden.iter().copied().collect();
            a.send(ViewportRequest::SetHidden(ids));
        }
        "view.isolate_selection" => {
            let nodes = app.borrow().session.selected_nodes();
            if nodes.is_empty() {
                return;
            }
            {
                let mut a = app.borrow_mut();
                a.isolated_nodes = true;
                a.send(ViewportRequest::IsolateNodes(Some(nodes)));
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
            let a = app.borrow();
            let (target, (w, h)) = (a.session.selection, a.viewport_size);
            // shortcut: opens at the viewport centre, not the node's projected origin; needs a projection query to the viewport.
            crate::context_menu::open(ui, &a, target, lab_viewport::Vec2::new(w as f32 / 2.0, h as f32 / 2.0));
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
        "app.settings" => crate::settings_ui::open(ui, &app.borrow()),
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
        "import.url" => crate::import::choose_url(ui),
        "catalog.open" => crate::catalog_book::open(ui, &app.borrow()),
        id if id.starts_with("open.url:") => crate::catalog_book::open_url(&id["open.url:".len()..]),
        "help.changelog" => crate::update::show_whats_new(ui),
        "app.check_updates" => crate::update::check_now(ui, app, true),
        "app.auto_updates" => {
            let on = crate::update::toggle_auto(app);
            show_toast(ui, &app.borrow(), if on { "Daily update checks on" } else { "Daily update checks off" }, false);
        }
        "update.show" => crate::update::show_update(ui),
        "update.install" => crate::update::install(ui),
        "bug.report" => crate::catalog_book::open_url(&format!(
            "https://github.com/narongrit1053/QOR-Quantum-Optics-Room/issues/new?body=LabSetup%20{}%0A%0AWhat%20happened%3A%0A",
            env!("CARGO_PKG_VERSION")
        )),
        "library.import_folder" => crate::import::choose_folder(ui),
        "import.step_insert" => crate::import::insert_dialog(ui, app, models),
        "paste.cancel" => ui.set_paste_target("".into()),
        "probe.clear" => app.borrow().send(ViewportRequest::ClearProbes(None)),
        id if id.starts_with("probe.close.") => {
            if let Ok(i) = id["probe.close.".len()..].parse() {
                app.borrow().send(ViewportRequest::ClearProbes(Some(i)));
            }
        }
        "tool.select" | "tool.move" | "tool.rotate" | "tool.measure" | "tool.constrain" | "tool.joint" | "tool.probe" => {
            let (index, tool) = match id {
                "tool.move" => (1, Tool::Move),
                "tool.rotate" => (2, Tool::Rotate),
                "tool.measure" => (3, Tool::Measure),
                "tool.constrain" => (4, Tool::Constrain),
                "tool.joint" => (5, Tool::Joint),
                "tool.probe" => (6, Tool::Probe),
                _ => (0, Tool::Select),
            };
            ui.set_tool(index);
            // A tool's own hint is set below or by its reset.
            set_hint(ui, "");
            crate::constrain::reset(ui, app, models, tool == Tool::Constrain);
            crate::joint::reset(ui, app, models, tool == Tool::Joint);
            match tool {
                Tool::Measure => set_hint(ui, "Click the first point"),
                // Cards follow committed edits; during a gizmo drag they update on release.
                Tool::Probe => set_hint(ui, "Click a beam to pin a probe"),
                _ => {}
            }
            app.borrow().send(ViewportRequest::SetTool(tool));
        }
        "edit.group" => {
            let (command, label, sel) = {
                let a = app.borrow();
                let nodes = a.session.selected_nodes();
                let (command, group) = multi_select::make_group_command(&a.session.doc, &nodes);
                let label = command.label(&a.session.doc);
                let sel = Sel::from_nodes(&group.members);
                (command, label, sel)
            };
            if let Err(error) = execute_sel_raw(ui, app, models, command, label, Some(sel), Vec::new()) {
                show_toast(ui, &app.borrow(), &error.to_string(), false);
            }
        }
        "edit.ungroup" => {
            let found = {
                let a = app.borrow();
                let doc = &a.session.doc;
                a.session
                    .selected_nodes()
                    .into_iter()
                    .find_map(|nid| doc.group_of(doc.chain_root(nid)).map(|g| (g.id, format!("Ungroup {}", g.name))))
            };
            if let Some((gid, label)) = found {
                execute(ui, app, models, id, Command::RemoveGroup(gid), label, None, Vec::new());
            }
        }
        "edit.copy" | "edit.cut" | "edit.paste" | "edit.duplicate" => {
            let (cut_cmd, pasted) = {
                let mut a = app.borrow_mut();
                let selected = a.session.selected_nodes();
                if id != "edit.paste" && selected.is_empty() {
                    return;
                }
                let copied = crate::clipboard::copy(&a.session.doc, &selected);
                if id == "edit.duplicate" {
                    (None, crate::clipboard::paste(&a.session.doc, &copied))
                } else if id == "edit.cut" {
                    let cut_roots = multi_select::chain_roots(&a.session.doc, &selected);
                    let cmd = remove_nodes_command(&a.session.doc, cut_roots);
                    a.clipboard = copied;
                    (Some(cmd), None)
                } else {
                    if id == "edit.copy" {
                        a.clipboard = copied;
                    }
                    (None, if id == "edit.paste" { crate::clipboard::paste(&a.session.doc, &a.clipboard) } else { None })
                }
            };
            if let Some(command) = cut_cmd {
                execute(ui, app, models, id, command, "Cut".into(), None, Vec::new());
            } else if let Some((command, roots)) = pasted {
                let label = if id == "edit.paste" { "Paste" } else { "Duplicate" };
                let sel = (!roots.is_empty()).then(|| Sel::from_nodes(&roots));
                execute_sel(ui, app, models, id, command, label.into(), sel, Vec::new());
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
            let nodes = if matches!(how, crate::nudge::Nudge::Turn { .. }) {
                match app.borrow().session.selection {
                    Some(SelectionTarget::Node(node)) => vec![node],
                    _ => return,
                }
            } else {
                let nodes = app.borrow().session.selected_nodes();
                if nodes.is_empty() {
                    return;
                }
                nodes
            };
            let label = if matches!(how, crate::nudge::Nudge::Turn { .. }) { "Rotate" } else { "Nudge" };
            let result = crate::nudge::nudge(&app.borrow().session.doc, &nodes, how);
            match result {
                Ok(Some(command)) => {
                    execute(ui, app, models, id, command, label.into(), None, Vec::new());
                }
                Ok(None) => {}
                Err(error) => {
                    let text = match error {
                        crate::nudge::NudgeError::Locked => "Part is locked",
                        crate::nudge::NudgeError::NoHole => "No hole there",
                        crate::nudge::NudgeError::MixedParents => "Can't move parts on different platforms together",
                    };
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
    let sel = select.map(|t| Sel { primary: Some(t), extra: Vec::new() });
    execute_sel(ui, app, models, id, command, label, sel, fixes)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_sel(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    id: &str,
    command: Command,
    label: String,
    select: Option<Sel>,
    fixes: Vec<OpeningFix>,
) -> bool {
    match execute_sel_raw(ui, app, models, command, label, select, fixes) {
        Ok(()) => true,
        Err(error) => {
            refuse(ui, app, models, id, &error.to_string(), None);
            false
        }
    }
}

fn execute_sel_raw(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    command: Command,
    label: String,
    select: Option<Sel>,
    fixes: Vec<OpeningFix>,
) -> Result<(), CommandError> {
    let result = {
        let mut a = app.borrow_mut();
        // Captured before the command removes it, for the toast.
        let removed_platform = match &command {
            Command::RemovePlatform(pid) => Some(*pid),
            Command::Batch(cmds) => cmds.iter().find_map(|c| match c {
                Command::RemovePlatform(pid) => Some(*pid),
                _ => None,
            }),
            _ => None,
        };
        let deleted = removed_platform.and_then(|pid| {
            a.session.doc.platform(pid).map(|p| (p.name.clone(), a.session.doc.nodes_on(pid).count()))
        });
        let before = current_sel(&a.session);
        let select_requested = select.is_some();
        match a.session.execute(command, Some(label)) {
            Ok(changes) => {
                if let Some(sel) = select {
                    let norm = normalize_sel(&a.session.doc, sel);
                    if norm.primary.is_some() {
                        a.session.selection = norm.primary;
                        a.session.extra = norm.extra;
                    }
                }
                a.session.fix_selection();
                let after = current_sel(&a.session);
                let snapshot = (!changes.is_empty()).then(|| Box::new(a.session.doc.clone()));
                let toast = toast_for(&fixes, deleted);
                a.toast_undo = toast.is_some();
                Ok((snapshot, changes, (after != before || select_requested).then_some(after), toast))
            }
            Err(error) => Err(error),
        }
    };
    let (snapshot, changes, selection, toast) = result?;
    ui.set_inspector_error("".into());
    {
        let a = app.borrow();
        if let Some(snapshot) = snapshot {
            a.send(ViewportRequest::ApplyChanges { snapshot, changes });
        }
        // Send selection after ApplyChanges so the viewport filters extras against the new snapshot.
        if let Some(sel) = selection {
            a.send(ViewportRequest::Select(sel.primary));
            a.send(ViewportRequest::SelectExtra(sel.extra));
        }
    }
    refresh(ui, app, models);
    if let Some(text) = toast {
        show_toast(ui, &app.borrow(), &text, true);
    }
    files::arm_autosave(app);
    Ok(())
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

pub(crate) fn undo_redo(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, redo: bool) {
    let outcome = {
        let mut a = app.borrow_mut();
        let history = &a.session.history;
        let label = if redo { history.redo_label() } else { history.undo_label() }.map(str::to_owned);
        let before = current_sel(&a.session);
        let result = if redo { a.session.redo() } else { a.session.undo() };
        match result {
            None => return,
            Some(Ok(changes)) => {
                if let Some(target) = changes.single_target()
                    && Some(target) != a.session.selection
                    && target.exists_in(&a.session.doc)
                {
                    a.session.selection = Some(target);
                    a.session.extra.clear();
                }
                a.session.fix_selection();
                let after = current_sel(&a.session);
                a.toast_undo = false;
                Ok((Box::new(a.session.doc.clone()), changes, (after != before).then_some(after), label))
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
                if let Some(sel) = selection {
                    a.send(ViewportRequest::Select(sel.primary));
                    a.send(ViewportRequest::SelectExtra(sel.extra));
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

fn current_sel(session: &Session) -> Sel {
    Sel { primary: session.selection, extra: session.extra.clone() }
}

fn normalize_sel(doc: &lab_core::Document, sel: Sel) -> Sel {
    let primary = sel.primary.filter(|t| t.exists_in(doc));
    let Some(SelectionTarget::Node(pid)) = primary else {
        return Sel { primary, extra: Vec::new() };
    };
    let mut extra = Vec::new();
    for id in sel.extra {
        if id != pid && doc.node(id).is_some() && !extra.contains(&id) {
            extra.push(id);
        }
    }
    Sel { primary, extra }
}

/// Stores `sel` (clearing extras whenever the primary is not a node) and syncs both primary and
/// extras to the viewport. Returns true if the stored selection changed.
pub(crate) fn store_sel(a: &mut AppState, sel: Sel) -> bool {
    let before = current_sel(&a.session);
    let next = normalize_sel(&a.session.doc, sel);
    a.session.selection = next.primary;
    a.session.extra = next.extra.clone();
    a.send(ViewportRequest::Select(next.primary));
    a.send(ViewportRequest::SelectExtra(next.extra));
    before != current_sel(&a.session)
}

/// Selection set from the UI (scene row click, clear, folder actions): stored locally, sent, refreshed.
pub(crate) fn select_sel(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels, sel: Sel) {
    let changed = store_sel(&mut app.borrow_mut(), sel);
    if changed {
        ui.set_inspector_error("".into());
    }
    refresh(ui, app, models);
}

fn select(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &UiModels, target: Option<SelectionTarget>) {
    select_sel(ui, app, models, Sel { primary: target, extra: Vec::new() });
}

/// `ViewportEvent::SelectionChanged`: validated against the document, passed through group click rules.
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
            let cur = current_sel(&a.session);
            store_sel(&mut a, cur);
            return;
        }
        let prev = current_sel(&a.session);
        let next = multi_select::click(&a.session.doc, &prev, target);
        store_sel(&mut a, next)
    };
    if !changed {
        return;
    }
    ui.set_inspector_error("".into());
    refresh(ui, app, models);
}

/// `ViewportEvent::SelectionToggled`: Ctrl+click in the viewport.
pub fn on_viewport_toggle(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &UiModels,
    target: SelectionTarget,
) {
    let changed = {
        let mut a = app.borrow_mut();
        if !target.exists_in(&a.session.doc) {
            return;
        }
        let prev = current_sel(&a.session);
        let next = match target {
            SelectionTarget::Node(id) => multi_select::toggle(&prev, id),
            _ => Sel { primary: Some(target), extra: Vec::new() },
        };
        store_sel(&mut a, next)
    };
    if !changed {
        return;
    }
    ui.set_inspector_error("".into());
    refresh(ui, app, models);
}

/// `ViewportEvent::ContextMenu`: right-click on a selected node keeps the multi-selection;
/// right-click outside clears stale extras before opening the menu.
pub fn on_viewport_context_menu(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &UiModels,
    target: Option<SelectionTarget>,
    pos: lab_viewport::Vec2,
) {
    let (valid, changed) = {
        let mut a = app.borrow_mut();
        let valid = target.filter(|t| t.exists_in(&a.session.doc));
        let prev = current_sel(&a.session);
        let next = multi_select::right_click(&a.session.doc, &prev, valid);
        (valid, store_sel(&mut a, next))
    };
    if changed {
        ui.set_inspector_error("".into());
        refresh(ui, app, models);
    }
    crate::context_menu::open(ui, &app.borrow(), valid, pos);
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
