//! Application session: document, history, location, dirty state and autosave tracking (M1-SPEC §7.2).

use lab_core::{ChangeSet, Command, CommandError, Document, History, SelectionTarget};
use lab_file::{LoadedProject, ProjectLocation};

pub struct Session {
    pub doc: Document,
    pub history: History,
    pub location: Option<ProjectLocation>,
    pub saved_state: Option<u64>, // None: never saved and edited, or recovered, or repaired on load
    pub autosaved_state: Option<u64>,
    pub migrated_from: Option<u32>, // older schema loaded and not yet saved → SaveOptions.backup_schema
    pub recovered: bool,
    pub selection: Option<SelectionTarget>,
}

impl Session {
    pub fn new_clean(doc: Document, location: Option<ProjectLocation>) -> Self {
        Self {
            doc,
            history: History::default(),
            location,
            saved_state: Some(0),
            autosaved_state: None,
            migrated_from: None,
            recovered: false,
            selection: None,
        }
    }

    pub fn from_loaded(p: LoadedProject) -> Self {
        let saved_state = if p.report.repairs.is_empty() { Some(0) } else { None };
        Self {
            doc: p.document,
            history: History::default(),
            location: Some(p.location),
            saved_state,
            autosaved_state: None,
            migrated_from: p.report.migrated_from,
            recovered: false,
            selection: None,
        }
    }

    pub fn from_recovered(p: LoadedProject) -> Self {
        Self {
            doc: p.document,
            history: History::default(),
            location: Some(p.location),
            saved_state: None,
            autosaved_state: None,
            migrated_from: p.report.migrated_from,
            recovered: true,
            selection: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.history.state_id())
    }

    pub fn needs_autosave(&self) -> bool {
        self.is_dirty() && self.autosaved_state != Some(self.history.state_id())
    }

    pub fn execute(&mut self, cmd: Command, label: Option<String>) -> Result<ChangeSet, CommandError> {
        let res = match label {
            Some(l) => self.history.execute_labeled(&mut self.doc, cmd, l),
            None => self.history.execute(&mut self.doc, cmd),
        };
        self.fix_selection();
        res
    }

    pub fn undo(&mut self) -> Option<Result<ChangeSet, CommandError>> {
        let res = self.history.undo(&mut self.doc);
        self.fix_selection();
        res
    }

    pub fn redo(&mut self) -> Option<Result<ChangeSet, CommandError>> {
        let res = self.history.redo(&mut self.doc);
        self.fix_selection();
        res
    }

    pub fn mark_saved(&mut self, state: u64, location: ProjectLocation) {
        self.saved_state = Some(state);
        self.location = Some(location);
        self.migrated_from = None;
        self.recovered = false;
    }

    pub fn title(&self) -> String {
        self.location.as_ref().map_or_else(|| self.doc.name.clone(), |loc| loc.display_name())
    }

    pub fn fix_selection(&mut self) -> bool {
        if self.selection.is_some_and(|target| !target.exists_in(&self.doc)) {
            self.selection = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::glam::DVec2;
    use lab_core::{LoadReport, NodeId, Platform, PlatformId, PlatformKind, SelectionTarget, Support, Surface};
    use lab_file::ProjectFormat;
    use std::path::PathBuf;

    fn sample_doc() -> Document {
        let mut doc = Document::new("Test Doc");
        doc.platforms.push(Platform {
            id: PlatformId(1),
            name: "Table 1".into(),
            kind: PlatformKind::OpticalTable,
            surface: Surface::Stainless,
            outline: lab_core::Outline::Rectangle { size: DVec2::new(1200.0, 900.0) },
            thickness: 100.0,
            top_height: 800.0,
            position: DVec2::ZERO,
            rotation_z: 0.0,
            hole_grid: None,
            support: Support::Fixed,
        });
        doc.nodes.push(lab_core::Node {
            id: NodeId(1),
            name: "Initial".into(),
            parent: lab_core::Parent::Platform(PlatformId(1)),
            attach: lab_core::Attach::Free,
            params: Default::default(),
            locked: false,
            part: "Part A".into(),
            shape: lab_core::model::Shape::Box { size: lab_core::glam::DVec3::ONE },
            finish: lab_core::model::Finish::Stainless,
            position: lab_core::glam::DVec3::ZERO,
            rotation: lab_core::glam::DQuat::IDENTITY,
            pickable: true,
        });
        doc
    }

    // F1: Session dirty logic: edit → dirty; undo → clean; save at state 3, undo to 2 → dirty,
    // redo → clean; a new edit after undoing past the save stays dirty; from_recovered is dirty.
    #[test]
    fn f1_session_dirty_logic() {
        let mut session = Session::new_clean(sample_doc(), None);
        assert!(!session.is_dirty());

        // edit -> dirty
        let cmd1 = Command::Rename { id: NodeId(1), name: "N1".into() };
        let _ = session.execute(cmd1, Some("Edit 1".into()));
        assert!(session.is_dirty());

        // undo -> clean
        let _ = session.undo();
        assert!(!session.is_dirty());

        // 3 edits
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N1".into() }, Some("1".into()));
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N2".into() }, Some("2".into()));
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N3".into() }, Some("3".into()));
        let state3 = session.history.state_id();

        // save at state 3
        let loc = ProjectLocation { path: PathBuf::from("test.labsetup"), format: ProjectFormat::Archive };
        session.mark_saved(state3, loc.clone());
        assert!(!session.is_dirty());

        // undo to 2 -> dirty
        let _ = session.undo();
        assert!(session.is_dirty());

        // redo -> clean
        let _ = session.redo();
        assert!(!session.is_dirty());

        // undo past the save (to state 2)
        let _ = session.undo();
        assert!(session.is_dirty());

        // new edit after undoing past the save -> stays dirty
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N4".into() }, Some("4".into()));
        assert!(session.is_dirty());

        // from_recovered is dirty
        let loaded = LoadedProject {
            document: sample_doc(),
            location: loc,
            report: LoadReport::default(),
        };
        let recovered = Session::from_recovered(loaded);
        assert!(recovered.is_dirty());
        assert!(recovered.recovered);
    }

    // F2: needs_autosave
    #[test]
    fn f2_needs_autosave() {
        let mut session = Session::new_clean(sample_doc(), None);
        assert!(!session.needs_autosave());

        // edit makes it dirty -> needs autosave
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N1".into() }, Some("1".into()));
        assert!(session.needs_autosave());

        // after autosave is recorded at current state
        session.autosaved_state = Some(session.history.state_id());
        assert!(!session.needs_autosave());

        // next edit needs autosave again
        let _ = session.execute(Command::Rename { id: NodeId(1), name: "N2".into() }, Some("2".into()));
        assert!(session.needs_autosave());

        // undo to clean -> does not need autosave
        let _ = session.undo();
        let _ = session.undo();
        assert!(!session.is_dirty());
        assert!(!session.needs_autosave());
    }

    #[test]
    fn session_selection_and_title() {
        let mut session = Session::new_clean(sample_doc(), None);
        assert_eq!(session.title(), "Test Doc");

        let loc = ProjectLocation { path: PathBuf::from("my_project.labsetup"), format: ProjectFormat::Archive };
        session.location = Some(loc);
        assert_eq!(session.title(), "my_project");

        session.selection = Some(SelectionTarget::Platform(PlatformId(1)));
        assert!(!session.fix_selection());
        assert_eq!(session.selection, Some(SelectionTarget::Platform(PlatformId(1))));

        // Remove platform
        session.doc.platforms.clear();
        assert!(session.fix_selection());
        assert_eq!(session.selection, None);
    }
}
