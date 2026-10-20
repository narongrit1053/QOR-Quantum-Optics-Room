//! UI-thread application state (M1-SPEC §7.5).
//!
//! The document lives here, inside the [`Session`]; the viewport only ever gets snapshots.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use lab_app::Session;
use lab_catalog::Catalog;
use lab_core::{HoleSet, LengthUnit, Platform, PlatformId, SelectionTarget, compute_holes};
use lab_file::{IoWorker, ProjectLocation, RecoveryCandidate, Settings};
use lab_viewport::ViewportRequest;

/// The app's only timers. All are single-shot, so an idle app has no wakeups.
#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
#[derive(Default)]
pub struct Timers {
    pub autosave: slint::Timer,
    pub toast: slint::Timer,
    pub status: slint::Timer,
}

/// A save submitted to the I/O worker and not yet answered.
#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
pub struct PendingSave {
    pub ticket: u64,
    /// History state id at submission: the state that is clean once the save succeeds.
    pub state: u64,
    pub location: ProjectLocation,
    pub then: AfterSave,
}

/// What a "Save changes?" sheet interrupted, resumed once the document is saved or discarded.
#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    New,
    Open,
    OpenDemo,
    Close,
}

/// What happens after a save succeeds.
#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterSave {
    Nothing,
    Continue(PendingAction),
    Quit,
}

/// Which sheet is open, and so what its buttons mean.
#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetKind {
    /// Information or an error with a single OK button (closing it has no other effect).
    Message,
    /// "Save changes to …?" [Save] [Don't Save] [Cancel] for `AppState::pending_action`.
    SaveChanges,
    /// "Couldn't save …" [OK] [Save As…].
    SaveFailed,
    /// "Recover unsaved changes?" for `AppState::recovery[0]`: [Restore] [Discard] [Later],
    /// or only [Later] for a newer format.
    Recovery,
}

#[allow(dead_code)] // Parts are used only by the file flows (files.rs, T6b).
pub struct AppState {
    pub session: Session,
    pub settings: Settings,
    pub hole_cache: BTreeMap<PlatformId, (Platform, Rc<HoleSet>)>,
    pub requests: Option<async_channel::Sender<ViewportRequest>>,
    pub viewport_size: (u32, u32),
    pub io: Option<IoWorker>,
    pub autosave_available: bool,
    pub next_ticket: u64,
    pub pending_save: Option<PendingSave>,
    pub pending_action: Option<PendingAction>,
    pub sheet: Option<SheetKind>,
    pub recovery: Vec<RecoveryCandidate>,
    pub busy: bool,
    /// Document note while `busy` ("Saving…", "Opening…"); shown instead of the session's note.
    pub busy_note: String,
    pub toast_undo: bool,
    pub timers: Timers,
    pub catalog: Arc<Catalog>,
    pub library: crate::library::LibraryState,
    pub stack: crate::stack::StackState,
}

impl AppState {
    pub fn new(session: Session, settings: Settings) -> Self {
        Self {
            session,
            settings,
            hole_cache: BTreeMap::new(),
            requests: None,
            viewport_size: (0, 0),
            io: None,
            autosave_available: false,
            next_ticket: 1,
            pending_save: None,
            pending_action: None,
            sheet: None,
            recovery: Vec::new(),
            busy: false,
            busy_note: String::new(),
            toast_undo: false,
            timers: Timers::default(),
            catalog: Arc::new(Catalog::default()),
            library: Default::default(),
            stack: Default::default(),
        }
    }

    /// Sends a request to the viewport. Unbounded: never blocks the UI thread; fails only
    /// before the viewport starts or during shutdown.
    pub fn send(&self, request: ViewportRequest) {
        if let Some(requests) = &self.requests {
            requests.try_send(request).ok();
        }
    }

    pub fn units(&self) -> LengthUnit {
        self.settings.display_unit
    }

    /// Returns a fresh ticket for an I/O job.
    #[allow(dead_code)] // Used by the file flows (T6b).
    pub fn ticket(&mut self) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        ticket
    }

    /// The hole set of `platform`: cached when the cached platform is identical, else computed
    /// (without caching; call [`AppState::update_hole_cache`] first to keep it).
    pub fn holes_for(&self, platform: &Platform) -> Rc<HoleSet> {
        match self.hole_cache.get(&platform.id) {
            Some((cached, holes)) if cached == platform => holes.clone(),
            _ => Rc::new(compute_holes(platform).unwrap_or_else(|_| HoleSet::empty())),
        }
    }

    /// Drops entries of removed platforms and recomputes the entry of the platform the
    /// inspector shows (the selected platform, or the selected node's platform) if it changed.
    pub fn update_hole_cache(&mut self) {
        let doc = &self.session.doc;
        self.hole_cache.retain(|id, _| doc.platform(*id).is_some());
        let shown = match self.session.selection {
            Some(SelectionTarget::Platform(id)) => Some(id),
            Some(SelectionTarget::Node(id)) => doc.root_platform(id),
            _ => None,
        };
        let Some(platform) = shown.and_then(|id| doc.platform(id)) else { return };
        if self.hole_cache.get(&platform.id).is_some_and(|(cached, _)| cached == platform) {
            return;
        }
        let holes = Rc::new(compute_holes(platform).unwrap_or_else(|_| HoleSet::empty()));
        self.hole_cache.insert(platform.id, (platform.clone(), holes));
    }
}
