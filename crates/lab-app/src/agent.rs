//! Live control in the running app: pipe requests are handed to the UI thread, reads answered
//! at once, writes paced through a queue and applied like hand edits.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use lab_app::live::features::FeatureLookup;
use lab_app::live::protocol::{
    Envelope, ErrorCode, LiveError, ReadRequest, Request, WriteRequest, err_line, ok_line, parse_line,
};
use lab_app::live::read::{StatusInfo, read};
use lab_app::live::resolve::{Ctx, quat_to_euler_deg, resolve};
use lab_core::Node;
use lab_core::part::{Feature, PartDef};
use lab_ui::AppWindow;
use serde_json::{Value, json};
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::ui_models::UiModels;

/// Writes are applied no faster than one per `PACE`.
const PACE: Duration = Duration::from_millis(300);
/// The badge shows while a client was seen this recently.
const BADGE_WINDOW: Duration = Duration::from_secs(10);

/// Live control while it is switched on (`AppState::agent`).
pub struct Agent {
    #[cfg(windows)]
    server: lab_app::live::pipe::Server,
    queue: VecDeque<Queued>,
    pacer: Pacer,
    /// Single-shot: armed only while writes wait, so an idle app has no wakeups.
    timer: slint::Timer,
    /// Label of the agent's last applied write (for the badge).
    pub last_label: Option<String>,
    /// Repeating 1 s check that hides the badge; runs only while the badge is shown.
    badge_timer: slint::Timer,
}

impl Agent {
    fn last_seen(&self) -> Option<Instant> {
        #[cfg(windows)]
        return self.server.last_seen();
        #[cfg(not(windows))]
        None
    }
}

struct Queued {
    doc: Option<String>,
    request: WriteRequest,
    reply: Sender<String>,
}

type Context = (slint::Weak<AppWindow>, Rc<RefCell<AppState>>, Rc<UiModels>);

thread_local! {
    /// The UI-thread objects the pipe threads' requests are run against.
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    /// Detected features by `(part id, node params as JSON)`.
    static FEATURES: RefCell<HashMap<(String, String), Vec<Feature>>> = RefCell::new(HashMap::new());
}

fn context() -> Option<(AppWindow, Rc<RefCell<AppState>>, Rc<UiModels>)> {
    CONTEXT.with(|c| {
        let (ui, app, models) = c.borrow().clone()?;
        Some((ui.upgrade()?, app, models))
    })
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    CONTEXT.with(|c| *c.borrow_mut() = Some((ui.as_weak(), app.clone(), models.clone())));
    ui.on_agent_badge_clicked({
        let ui = ui.as_weak();
        let app = app.clone();
        move || {
            if let Some(ui) = ui.upgrade() {
                // Same path as the menu, so the sheet shows the current values.
                crate::settings_ui::open(&ui, &app.borrow());
            }
        }
    });
    if app.borrow().settings.allow_agents {
        set_enabled(ui, app, models, true);
    }
}

/// Starts or stops the pipe server (the Settings toggle).
pub fn set_enabled(ui: &AppWindow, app: &Rc<RefCell<AppState>>, _models: &Rc<UiModels>, on: bool) {
    if !on {
        stop(ui, app, disabled());
        return;
    }
    if app.borrow().agent.is_some() {
        return;
    }
    match start_server() {
        Ok(_server) => {
            app.borrow_mut().agent = Some(Agent {
                #[cfg(windows)]
                server: _server,
                queue: VecDeque::new(),
                pacer: Pacer::default(),
                timer: slint::Timer::default(),
                last_label: None,
                badge_timer: slint::Timer::default(),
            });
        }
        Err(error) => {
            eprintln!("live control: {error}");
            let text = format!("AI agent control couldn't start: {error}");
            controller::show_toast(ui, &app.borrow(), &text, false);
        }
    }
}

/// Stops the server on app exit; queued writes are told the app is closing.
pub fn shut_down(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    stop(ui, app, closing());
}

/// Stops the server, answering every queued write with `why`.
fn stop(ui: &AppWindow, app: &Rc<RefCell<AppState>>, why: LiveError) {
    let Some(agent) = app.borrow_mut().agent.take() else { return };
    let token = app.borrow().doc_token.to_string();
    for q in agent.queue {
        let _ = q.reply.send(err_line(&token, &why));
    }
    #[cfg(windows)]
    agent.server.stop();
    ui.set_agent_status("".into());
}

fn closing() -> LiveError {
    LiveError::new(ErrorCode::Disabled, "LabSetup is closing")
}

#[cfg(windows)]
fn start_server() -> std::io::Result<lab_app::live::pipe::Server> {
    use lab_app::live::pipe::{Server, pipe_name};
    Server::start(&pipe_name(std::process::id()), |line| {
        let (tx, rx): (Sender<String>, Receiver<String>) = mpsc::channel();
        let fallback = tx.clone();
        if slint::invoke_from_event_loop(move || on_line(line, tx)).is_err() {
            let _ = fallback.send(err_line("", &closing()));
        }
        rx
    })
}

#[cfg(not(windows))]
fn start_server() -> std::io::Result<()> {
    Err(std::io::Error::other("live control needs Windows"))
}

/// A write is refused while the user is in the middle of something (spec "Concurrency").
/// `ui_busy`: a Slint-side sheet or dialog is open, or the start page hides the document.
pub fn is_busy(a: &AppState, ui_busy: bool) -> bool {
    ui_busy
        || a.busy
        || a.sheet.is_some()
        || a.pending_action.is_some()
        || a.dragging
        || crate::constrain::is_open()
        || crate::joint::panel_open()
        || crate::wizard::is_open()
}

fn ui_busy(ui: &AppWindow) -> bool {
    ui.get_modal() || ui.get_start_page_visible()
}

fn disabled() -> LiveError {
    LiveError::new(ErrorCode::Disabled, "AI agent control is switched off in LabSetup's Settings")
}

fn admit(env: &Envelope, token: u64, busy: bool) -> Result<(), LiveError> {
    if matches!(env.request, Request::Read(_)) {
        return Ok(());
    }
    if env.doc.as_ref().is_some_and(|d| *d != token.to_string()) {
        return Err(LiveError::new(ErrorCode::DocChanged, "another project was opened; read it again"));
    }
    if busy {
        return Err(LiveError::new(ErrorCode::Busy, "LabSetup is in the middle of an edit; retry shortly"));
    }
    Ok(())
}

#[derive(Default)]
struct Pacer {
    last: Option<Instant>,
}

impl Pacer {
    /// True (and counted as a write) when `PACE` has passed since the last one.
    fn ready(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|last| now.duration_since(last) < PACE) {
            return false;
        }
        self.last = Some(now);
        true
    }

    fn wait(&self, now: Instant) -> Duration {
        self.last.map_or(Duration::ZERO, |last| PACE.saturating_sub(now.duration_since(last)))
    }
}

/// Runs on the UI thread for each request line; sends exactly one reply line.
fn on_line(line: String, reply: Sender<String>) {
    let Some((ui, app, models)) = context() else {
        let _ = reply.send(err_line("", &disabled()));
        return;
    };
    let token = app.borrow().doc_token;
    let fail = |e: LiveError| {
        let _ = reply.send(err_line(&token.to_string(), &e));
    };
    if app.borrow().agent.is_none() {
        return fail(disabled()); // raced a disable
    }
    let env = match parse_line(&line) {
        Ok(env) => env,
        Err(e) => return fail(e),
    };
    refresh_badge(&ui, &app, true);
    if let Err(e) = admit(&env, token, is_busy(&app.borrow(), ui_busy(&ui))) {
        return fail(e);
    }
    match env.request {
        Request::Read(ReadRequest::Snapshot { path, width, height }) => snapshot(&app, path, width, height, reply),
        Request::Read(r) => {
            let a = app.borrow();
            let status = StatusInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                title: a.session.title(),
                path: a.session.location.as_ref().map(|l| l.path.clone()),
                dirty: a.session.is_dirty(),
                undo_label: a.session.history.undo_label().map(str::to_owned),
            };
            let _ = reply.send(answer(&a, with_ctx(&a, |ctx| read(&r, ctx, &status))));
        }
        Request::App(_) => fail(LiveError::bad("not supported yet")),
        Request::Write(request) => {
            if let Some(agent) = app.borrow_mut().agent.as_mut() {
                agent.queue.push_back(Queued { doc: env.doc, request, reply });
            }
            drain(&ui, &app, &models);
        }
    }
}

fn answer(a: &AppState, result: Result<Value, LiveError>) -> String {
    let token = a.doc_token.to_string();
    match result {
        Ok(v) => ok_line(&token, v),
        Err(e) => err_line(&token, &e),
    }
}

struct Features;

impl FeatureLookup for Features {
    fn detected(&self, node: &Node, def: &PartDef) -> Vec<Feature> {
        if !def.features.is_empty() {
            return def.features.clone();
        }
        let key = (def.id.clone(), serde_json::to_string(&node.params).unwrap_or_default());
        if let Some(found) = FEATURES.with(|f| f.borrow().get(&key).cloned()) {
            return found;
        }
        let found = crate::constrain::part_mesh(def, node).map(|m| lab_import::features::detect(&m)).unwrap_or_default();
        FEATURES.with(|f| f.borrow_mut().insert(key, found.clone()));
        found
    }
}

fn with_ctx<R>(a: &AppState, f: impl FnOnce(&Ctx) -> R) -> R {
    let holes = |id| a.session.doc.platform(id).map(|p| a.holes_for(p));
    f(&Ctx { doc: &a.session.doc, catalog: &a.catalog, holes: &holes, features: &Features, trace: &a.beams })
}

fn snapshot(app: &Rc<RefCell<AppState>>, path: Option<PathBuf>, width: Option<u32>, height: Option<u32>, reply: Sender<String>) {
    let path = path.unwrap_or_else(|| {
        let mut a = app.borrow_mut();
        a.agent_snapshots += 1;
        std::env::temp_dir().join(format!("labsetup-agent-{}-{}.png", std::process::id(), a.agent_snapshots))
    });
    let weak = Rc::downgrade(app);
    let shown = path.clone();
    let done = move |ok: bool| {
        let Some(app) = weak.upgrade() else { return };
        let result = if ok {
            Ok(json!({ "path": shown }))
        } else {
            Err(LiveError::new(ErrorCode::Refused, "the view could not be rendered"))
        };
        let _ = reply.send(answer(&app.borrow(), result));
    };
    crate::export_png::render_to(app, path, width.unwrap_or(1280), height.unwrap_or(800), Box::new(done));
}

/// "Agent connected" until a write, then "Agent: <step>" (labels already carry the prefix).
fn badge_text(label: Option<&str>) -> String {
    match label {
        Some(l) => format!("Agent: {}", l.strip_prefix("Agent: ").unwrap_or(l)),
        None => "Agent connected".into(),
    }
}

fn badge_visible(last_seen: Option<Instant>, now: Instant) -> bool {
    last_seen.is_some_and(|t| now.saturating_duration_since(t) < BADGE_WINDOW)
}

/// Shows or hides the badge. `seen` marks a request just arrived. The 1 s timer runs only
/// while the badge is up, so an idle app has no wakeups.
fn refresh_badge(ui: &AppWindow, app: &Rc<RefCell<AppState>>, seen: bool) {
    let mut a = app.borrow_mut();
    let Some(agent) = a.agent.as_mut() else { return };
    if seen || badge_visible(agent.last_seen(), Instant::now()) {
        ui.set_agent_status(badge_text(agent.last_label.as_deref()).into());
        if !agent.badge_timer.running() {
            agent.badge_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                if let Some((ui, app, _)) = context() {
                    refresh_badge(&ui, &app, false);
                }
            });
        }
    } else {
        ui.set_agent_status("".into());
        agent.badge_timer.stop();
    }
}

/// Applies the next queued write if the pacer allows, then arms the timer for the rest.
fn drain(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) {
    let job = {
        let mut a = app.borrow_mut();
        let Some(agent) = a.agent.as_mut() else { return };
        if !agent.queue.is_empty() && agent.pacer.ready(Instant::now()) { agent.queue.pop_front() } else { None }
    };
    if let Some(job) = job {
        let line = apply(ui, app, models, job.doc, job.request);
        let _ = job.reply.send(line);
    }
    let mut a = app.borrow_mut();
    let Some(agent) = a.agent.as_mut() else { return };
    if agent.queue.is_empty() {
        agent.timer.stop();
    } else if !agent.timer.running() {
        agent.timer.start(slint::TimerMode::SingleShot, agent.pacer.wait(Instant::now()), || {
            if let Some((ui, app, models)) = context() {
                drain(&ui, &app, &models);
            }
        });
    }
}

/// Applies one write through the history, exactly like a hand edit; returns the reply line.
fn apply(
    ui: &AppWindow,
    app: &Rc<RefCell<AppState>>,
    models: &Rc<UiModels>,
    doc: Option<String>,
    request: WriteRequest,
) -> String {
    // Checked again: the project or the user's state may have changed while it waited.
    let admitted = {
        let a = app.borrow();
        // Any write stands in for this one: admit only looks at its kind.
        let env = Envelope { v: 1, doc, request: Request::Write(WriteRequest::Undo) };
        admit(&env, a.doc_token, is_busy(&a, ui_busy(ui)))
    };
    if let Err(e) = admitted {
        return answer(&app.borrow(), Err(e));
    }
    let before = app.borrow().session.doc.nodes.clone();
    let label = match request {
        WriteRequest::Undo | WriteRequest::Redo => {
            let redo = matches!(request, WriteRequest::Redo);
            let has = {
                let history = &app.borrow().session.history;
                if redo { history.can_redo() } else { history.can_undo() }
            };
            if !has {
                let what = if redo { "nothing to redo" } else { "nothing to undo" };
                return answer(&app.borrow(), Err(LiveError::new(ErrorCode::Refused, what)));
            }
            controller::undo_redo(ui, app, models, redo);
            if redo { "Agent: redo" } else { "Agent: undo" }.to_string()
        }
        request => {
            let resolved = {
                let a = app.borrow();
                with_ctx(&a, |ctx| resolve(&request, ctx))
            };
            let resolved = match resolved {
                Ok(r) => r,
                Err(e) => return answer(&app.borrow(), Err(e)),
            };
            let label = resolved.label.clone();
            if !controller::execute(ui, app, models, "live", resolved.command, label.clone(), resolved.select, vec![]) {
                let shown = ui.get_inspector_error().to_string();
                let message = if shown.is_empty() { "LabSetup could not apply this change".into() } else { shown };
                return answer(&app.borrow(), Err(LiveError::new(ErrorCode::Refused, message)));
            }
            label
        }
    };
    if let Some(agent) = app.borrow_mut().agent.as_mut() {
        agent.last_label = Some(label);
    }
    refresh_badge(ui, app, true);
    let a = app.borrow();
    let changed: Vec<&Node> = a.session.doc.nodes.iter().filter(|n| !before.contains(n)).collect();
    let result = json!({
        "changed": changed.iter().map(|n| n.id.0).collect::<Vec<_>>(),
        "poses": changed.iter().map(|n| json!({
            "id": n.id.0,
            "position": [n.position.x, n.position.y, n.position.z],
            "rotation": quat_to_euler_deg(n.rotation),
        })).collect::<Vec<_>>(),
        "undo": a.session.history.undo_label(),
    });
    answer(&a, Ok(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_app::live::protocol::{parse_line, ErrorCode};

    fn env(line: &str) -> Envelope {
        parse_line(line).unwrap()
    }

    #[test]
    fn write_with_old_token_is_doc_changed() {
        let e = env(r#"{"v":1,"cmd":"rename","args":{"node":1,"name":"M1"},"doc":"1"}"#);
        assert_eq!(admit(&e, 2, false).unwrap_err().code, ErrorCode::DocChanged);
        assert!(admit(&e, 1, false).is_ok());
    }

    #[test]
    fn write_while_busy_is_busy() {
        let e = env(r#"{"v":1,"cmd":"undo"}"#);
        assert_eq!(admit(&e, 1, true).unwrap_err().code, ErrorCode::Busy);
    }

    #[test]
    fn ui_side_sheet_makes_busy() {
        let a = AppState::new(
            lab_app::Session::new_clean(lab_core::presets::default_document(), None),
            lab_file::Settings::default(),
        );
        assert!(!is_busy(&a, false));
        assert!(is_busy(&a, true));
    }

    #[test]
    fn read_ignores_busy() {
        let e = env(r#"{"v":1,"cmd":"status","doc":"1"}"#);
        assert!(admit(&e, 1, true).is_ok());
    }

    #[test]
    fn write_without_token_is_admitted() {
        let e = env(r#"{"v":1,"cmd":"rename","args":{"node":1,"name":"M1"}}"#);
        assert!(admit(&e, 7, false).is_ok());
    }

    #[test]
    fn badge_text_strips_prefix() {
        assert_eq!(badge_text(None), "Agent connected");
        assert_eq!(badge_text(Some("Agent: place KM100")), "Agent: place KM100");
        assert_eq!(badge_text(Some("place KM100")), "Agent: place KM100");
    }

    #[test]
    fn badge_hides_after_ten_seconds() {
        let t = Instant::now();
        assert!(badge_visible(Some(t), t + Duration::from_secs(9)));
        assert!(!badge_visible(Some(t), t + Duration::from_secs(10)));
        assert!(!badge_visible(None, t));
    }

    #[test]
    fn pacer_spaces_writes_300ms() {
        let mut p = Pacer::default();
        let t = Instant::now();
        assert!(p.ready(t));
        assert!(!p.ready(t + Duration::from_millis(299)));
        assert!(p.ready(t + PACE));
        assert!(!p.ready(t + PACE + Duration::from_millis(100)));
        assert!(p.ready(t + PACE * 2));
        assert_eq!(p.wait(t + PACE * 2 + Duration::from_millis(100)), Duration::from_millis(200));
    }
}
