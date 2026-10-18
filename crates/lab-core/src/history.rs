//! Undo/redo with state ids (M1-SPEC §3.8, D11).
//!
//! Every document state reached through the history has an id. Each entry records the ids
//! before and after it, so undoing back to a saved state makes the document clean again, and
//! trimming or coalescing entries never confuses two different states.

use crate::command::{ChangeSet, Command, CommandError};
use crate::limits::HISTORY_LIMIT;
use crate::model::Document;

#[derive(Debug)]
struct Entry {
    /// The command that moves the document to the other side of this entry.
    command: Command,
    label: String,
    coalesce: Option<String>,
    state_before: u64,
    state_after: u64,
}

/// Undo/redo stacks. Executing a new command clears the redo stack.
#[derive(Debug)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    current: u64,
    next_state: u64,
    open_gesture: Option<String>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(HISTORY_LIMIT)
    }
}

impl History {
    /// An empty history at state 0, keeping at most `limit` undo entries.
    pub fn new(limit: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), limit: limit.max(1), current: 0, next_state: 1, open_gesture: None }
    }

    fn take_state(&mut self) -> u64 {
        let state = self.next_state;
        self.next_state += 1;
        state
    }

    fn push_undo(&mut self, entry: Entry) {
        self.undo.push(entry);
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(..excess);
        }
    }

    /// The failure policy: an inverse failed to apply (the document is unchanged because
    /// commands are atomic). Both stacks are dropped and the state gets a fresh id.
    fn reset_after_failure(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_gesture = None;
        self.current = self.take_state();
    }

    /// Executes `command` with its default label.
    pub fn execute(&mut self, doc: &mut Document, command: Command) -> Result<ChangeSet, CommandError> {
        let label = command.label(doc);
        self.execute_labeled(doc, command, label)
    }

    /// Executes `command` as one undo step named `label`. A command that changes nothing
    /// records nothing and keeps the state id.
    pub fn execute_labeled(
        &mut self,
        doc: &mut Document,
        command: Command,
        label: impl Into<String>,
    ) -> Result<ChangeSet, CommandError> {
        let (inverse, changes) = command.apply(doc)?;
        if changes.is_empty() {
            return Ok(changes);
        }
        let after = self.take_state();
        let entry = Entry {
            command: inverse,
            label: label.into(),
            coalesce: None,
            state_before: self.current,
            state_after: after,
        };
        self.push_undo(entry);
        self.current = after;
        self.redo.clear();
        self.open_gesture = None;
        Ok(changes)
    }

    /// Like [`History::execute_labeled`], but while a gesture with the same `key` is open the
    /// command joins the previous entry, so a whole drag undoes in one step. Each command still
    /// gets a new state id. Reserved for M2 drags.
    pub fn execute_coalescing(
        &mut self,
        doc: &mut Document,
        command: Command,
        label: impl Into<String>,
        key: &str,
    ) -> Result<ChangeSet, CommandError> {
        let (inverse, changes) = command.apply(doc)?;
        if changes.is_empty() {
            return Ok(changes);
        }
        let after = self.take_state();
        let joins = self.open_gesture.as_deref() == Some(key)
            && self.undo.last().is_some_and(|top| top.coalesce.as_deref() == Some(key));
        match self.undo.last_mut() {
            // The open gesture's entry already restores the state before the gesture.
            Some(top) if joins => top.state_after = after,
            _ => {
                let entry = Entry {
                    command: inverse,
                    label: label.into(),
                    coalesce: Some(key.to_string()),
                    state_before: self.current,
                    state_after: after,
                };
                self.push_undo(entry);
                self.open_gesture = Some(key.to_string());
            }
        }
        self.current = after;
        self.redo.clear();
        Ok(changes)
    }

    /// Closes the open gesture: the next coalescing command starts a new entry.
    pub fn end_gesture(&mut self) {
        self.open_gesture = None;
    }

    /// Undoes the most recent entry. Returns `None` when there is nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>> {
        let Entry { command, label, state_before, state_after, .. } = self.undo.pop()?;
        self.open_gesture = None;
        match command.apply(doc) {
            Ok((inverse, changes)) => {
                self.redo.push(Entry { command: inverse, label, coalesce: None, state_before, state_after });
                self.current = state_before;
                Some(Ok(changes))
            }
            Err(e) => {
                self.reset_after_failure();
                Some(Err(e))
            }
        }
    }

    /// Re-applies the most recently undone entry.
    pub fn redo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>> {
        let Entry { command, label, state_before, state_after, .. } = self.redo.pop()?;
        self.open_gesture = None;
        match command.apply(doc) {
            Ok((inverse, changes)) => {
                self.push_undo(Entry { command: inverse, label, coalesce: None, state_before, state_after });
                self.current = state_after;
                Some(Ok(changes))
            }
            Err(e) => {
                self.reset_after_failure();
                Some(Err(e))
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Id of the current document state. Equal ids mean equal documents (ignoring `next_id`).
    pub fn state_id(&self) -> u64 {
        self.current
    }

    /// Drops both stacks; the current state id stays.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_gesture = None;
    }
}

#[cfg(test)]
mod tests {
    use glam::{DQuat, DVec3};

    use super::*;
    use crate::model::NodeId;
    use crate::sample::demo_document;

    fn place(id: NodeId, x: f64) -> Command {
        Command::SetPlacement { id, position: DVec3::new(x, 0.0, 10.0), rotation: DQuat::IDENTITY }
    }

    #[test]
    fn s1_labels() {
        let mut doc = demo_document();
        let mut h = History::default();
        let id = doc.nodes[0].id;
        let old_name = doc.nodes[0].name.clone();
        h.execute(&mut doc, Command::Rename { id, name: "Laser".into() }).unwrap();
        assert_eq!(h.undo_label(), Some(format!("Rename {old_name}").as_str()));
        h.execute_labeled(&mut doc, place(id, 5.0), "Change X").unwrap();
        assert_eq!(h.undo_label(), Some("Change X"));
        assert_eq!(h.redo_label(), None);
        h.undo(&mut doc).unwrap().unwrap();
        assert_eq!(h.redo_label(), Some("Change X"));
        assert_eq!(h.undo_label(), Some(format!("Rename {old_name}").as_str()));
        h.redo(&mut doc).unwrap().unwrap();
        assert_eq!(h.undo_label(), Some("Change X"));
    }

    #[test]
    fn s1_state_ids_follow_undo_and_redo() {
        let mut doc = demo_document();
        let start = doc.clone();
        let mut h = History::default();
        let id = doc.nodes[0].id;
        assert_eq!(h.state_id(), 0);
        h.execute(&mut doc, place(id, 1.0)).unwrap();
        let s1 = h.state_id();
        let d1 = doc.clone();
        h.execute(&mut doc, place(id, 2.0)).unwrap();
        let s2 = h.state_id();
        assert!(s1 != 0 && s2 != s1);
        h.undo(&mut doc).unwrap().unwrap();
        assert_eq!(h.state_id(), s1);
        assert!(doc.content_eq(&d1));
        h.undo(&mut doc).unwrap().unwrap();
        assert_eq!(h.state_id(), 0);
        assert!(doc.content_eq(&start));
        h.redo(&mut doc).unwrap().unwrap();
        assert_eq!(h.state_id(), s1);
        // A new edit after undoing gets a new id, never s2 again.
        h.execute(&mut doc, place(id, 3.0)).unwrap();
        assert!(![0, s1, s2].contains(&h.state_id()));
        assert!(!h.can_redo());
    }

    #[test]
    fn s1_limit_keeps_the_trimmed_state_dirty() {
        let mut doc = demo_document();
        let mut h = History::new(2);
        let id = doc.nodes[0].id;
        for x in [1.0, 2.0, 3.0] {
            h.execute(&mut doc, place(id, x)).unwrap();
        }
        while h.can_undo() {
            h.undo(&mut doc).unwrap().unwrap();
        }
        // The oldest entry was dropped: the document is not back at state 0 (it stays dirty).
        assert_ne!(h.state_id(), 0);
        assert_eq!(doc.node(id).unwrap().position.x, 1.0);
        while h.can_redo() {
            h.redo(&mut doc).unwrap().unwrap();
        }
        assert_eq!(doc.node(id).unwrap().position.x, 3.0);
    }

    #[test]
    fn s1_failed_inverse_resets_the_history() {
        let mut doc = demo_document();
        let mut h = History::default();
        let mut node = doc.nodes[0].clone();
        node.id = NodeId(doc.next_free_id());
        let new_id = node.id;
        h.execute(&mut doc, Command::AddNode { node, index: None }).unwrap();
        let first = doc.nodes[0].id;
        h.execute(&mut doc, place(first, 9.0)).unwrap();
        h.undo(&mut doc).unwrap().unwrap();
        assert!(h.can_redo());
        let before_state = h.state_id();
        // Corrupt the document behind the history's back: the inverse (RemoveNode) cannot apply.
        let index = doc.node_index(new_id).unwrap();
        doc.nodes.remove(index);
        let snapshot = doc.clone();
        let result = h.undo(&mut doc).unwrap();
        assert_eq!(result, Err(CommandError::UnknownNode(new_id)));
        assert_eq!(doc, snapshot);
        assert!(!h.can_undo() && !h.can_redo());
        assert_ne!(h.state_id(), before_state);
        assert_ne!(h.state_id(), 0);
        assert!(h.undo(&mut doc).is_none());
    }

    #[test]
    fn s1_empty_change_set_keeps_the_state() {
        let mut doc = demo_document();
        let mut h = History::default();
        let node = &doc.nodes[0];
        let same = Command::SetPlacement { id: node.id, position: node.position, rotation: node.rotation };
        assert!(h.execute(&mut doc, same.clone()).unwrap().is_empty());
        assert_eq!(h.state_id(), 0);
        assert!(!h.can_undo());
        assert!(h.execute_coalescing(&mut doc, same, "Move", "drag").unwrap().is_empty());
        assert_eq!(h.state_id(), 0);
        assert!(!h.can_undo());
    }

    #[test]
    fn s1_coalescing() {
        let mut doc = demo_document();
        let start = doc.clone();
        let mut h = History::default();
        let id = doc.nodes[0].id;
        let mut states = vec![h.state_id()];
        for x in [1.0, 2.0, 3.0] {
            h.execute_coalescing(&mut doc, place(id, x), "Move Laser", "drag").unwrap();
            states.push(h.state_id());
        }
        states.dedup();
        assert_eq!(states.len(), 4, "each coalesced command changes the state id");
        let end = doc.clone();
        let end_state = h.state_id();
        assert_eq!(h.undo_label(), Some("Move Laser"));
        h.undo(&mut doc).unwrap().unwrap();
        assert!(!h.can_undo(), "three coalesced commands give one undo step");
        assert!(doc.content_eq(&start));
        assert_eq!(h.state_id(), 0);
        h.redo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&end));
        assert_eq!(h.state_id(), end_state);
    }

    #[test]
    fn s1_end_gesture_splits_entries() {
        let mut doc = demo_document();
        let mut h = History::default();
        let id = doc.nodes[0].id;
        h.execute_coalescing(&mut doc, place(id, 1.0), "Move", "drag").unwrap();
        h.execute_coalescing(&mut doc, place(id, 2.0), "Move", "drag").unwrap();
        h.end_gesture();
        h.execute_coalescing(&mut doc, place(id, 3.0), "Move", "drag").unwrap();
        // A different key, or a plain command, also starts a new entry.
        h.execute_coalescing(&mut doc, place(id, 4.0), "Move", "other").unwrap();
        h.execute(&mut doc, place(id, 5.0)).unwrap();
        h.execute_coalescing(&mut doc, place(id, 6.0), "Move", "other").unwrap();
        let mut steps = 0;
        while h.can_undo() {
            h.undo(&mut doc).unwrap().unwrap();
            steps += 1;
        }
        assert_eq!(steps, 5);
        assert_eq!(h.state_id(), 0);
    }

    #[test]
    fn clear_keeps_the_state() {
        let mut doc = demo_document();
        let mut h = History::default();
        let id = doc.nodes[0].id;
        h.execute(&mut doc, place(id, 1.0)).unwrap();
        let s = h.state_id();
        h.clear();
        assert_eq!(h.state_id(), s);
        assert!(!h.can_undo() && !h.can_redo());
    }
}
