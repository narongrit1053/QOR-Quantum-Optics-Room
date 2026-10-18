//! Edits to a [`Document`], each returning its inverse so it can be undone.

use glam::{DQuat, DVec3};

use crate::model::{Document, Node, NodeId};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Adds `node`. With `index`, inserts at that position (used when undoing a removal so
    /// the document's order is restored exactly).
    AddNode { node: Node, index: Option<usize> },
    RemoveNode(NodeId),
    SetPlacement { id: NodeId, position: DVec3, rotation: DQuat },
    Rename { id: NodeId, name: String },
}

/// What a command touched, so projections (viewport, scene tree) can update incrementally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    pub added: Vec<NodeId>,
    pub removed: Vec<NodeId>,
    pub updated: Vec<NodeId>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.updated.is_empty()
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error("no node with id {0:?}")]
    UnknownNode(NodeId),
    #[error("a node with id {0:?} already exists")]
    DuplicateNode(NodeId),
    #[error("node {0:?} refers to a platform that does not exist")]
    UnknownPlatform(NodeId),
}

impl Command {
    /// Applies the command to `doc`. Returns the inverse command and the resulting change set.
    /// On error the document is left unchanged.
    pub fn apply(self, doc: &mut Document) -> Result<(Command, ChangeSet), CommandError> {
        match self {
            Command::AddNode { node, index } => {
                if doc.node(node.id).is_some() {
                    return Err(CommandError::DuplicateNode(node.id));
                }
                if doc.platform(node.platform).is_none() {
                    return Err(CommandError::UnknownPlatform(node.id));
                }
                let id = node.id;
                let index = index.unwrap_or(doc.nodes.len()).min(doc.nodes.len());
                doc.nodes.insert(index, node);
                Ok((Command::RemoveNode(id), ChangeSet { added: vec![id], ..Default::default() }))
            }
            Command::RemoveNode(id) => {
                let index = doc.node_index(id).ok_or(CommandError::UnknownNode(id))?;
                let node = doc.nodes.remove(index);
                Ok((
                    Command::AddNode { node, index: Some(index) },
                    ChangeSet { removed: vec![id], ..Default::default() },
                ))
            }
            Command::SetPlacement { id, position, rotation } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let inverse = Command::SetPlacement { id, position: node.position, rotation: node.rotation };
                node.position = position;
                node.rotation = rotation;
                Ok((inverse, ChangeSet { updated: vec![id], ..Default::default() }))
            }
            Command::Rename { id, name } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let old = std::mem::replace(&mut node.name, name);
                Ok((Command::Rename { id, name: old }, ChangeSet { updated: vec![id], ..Default::default() }))
            }
        }
    }
}

/// Undo/redo stacks. Executing a new command clears the redo stack.
#[derive(Debug)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(500)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), limit: limit.max(1) }
    }

    pub fn execute(&mut self, doc: &mut Document, command: Command) -> Result<ChangeSet, CommandError> {
        let (inverse, changes) = command.apply(doc)?;
        self.undo.push(inverse);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(changes)
    }

    /// Undoes the most recent command. Returns `None` when there is nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>> {
        let command = self.undo.pop()?;
        Some(command.apply(doc).map(|(inverse, changes)| {
            self.redo.push(inverse);
            changes
        }))
    }

    /// Re-applies the most recently undone command.
    pub fn redo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>> {
        let command = self.redo.pop()?;
        Some(command.apply(doc).map(|(inverse, changes)| {
            self.undo.push(inverse);
            changes
        }))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::demo_document;

    fn some_node(doc: &Document) -> NodeId {
        doc.nodes[3].id
    }

    #[test]
    fn remove_then_undo_restores_document_exactly() {
        let mut doc = demo_document();
        let before = doc.clone();
        let mut history = History::default();
        let id = some_node(&doc);

        let changes = history.execute(&mut doc, Command::RemoveNode(id)).unwrap();
        assert_eq!(changes.removed, vec![id]);
        assert!(doc.node(id).is_none());

        history.undo(&mut doc).unwrap().unwrap();
        assert_eq!(doc, before);

        history.redo(&mut doc).unwrap().unwrap();
        assert!(doc.node(id).is_none());
    }

    #[test]
    fn placement_and_rename_round_trip() {
        let mut doc = demo_document();
        let before = doc.clone();
        let mut history = History::default();
        let id = some_node(&doc);

        history
            .execute(&mut doc, Command::SetPlacement { id, position: DVec3::new(1.0, 2.0, 3.0), rotation: DQuat::IDENTITY })
            .unwrap();
        history.execute(&mut doc, Command::Rename { id, name: "M9".into() }).unwrap();
        assert_eq!(doc.node(id).unwrap().name, "M9");

        history.undo(&mut doc).unwrap().unwrap();
        history.undo(&mut doc).unwrap().unwrap();
        assert_eq!(doc, before);
        assert!(!history.can_undo());
        assert!(history.can_redo());
    }

    #[test]
    fn new_command_clears_redo() {
        let mut doc = demo_document();
        let mut history = History::default();
        let id = some_node(&doc);
        history.execute(&mut doc, Command::Rename { id, name: "a".into() }).unwrap();
        history.undo(&mut doc).unwrap().unwrap();
        history.execute(&mut doc, Command::Rename { id, name: "b".into() }).unwrap();
        assert!(!history.can_redo());
    }

    #[test]
    fn errors_leave_document_unchanged() {
        let mut doc = demo_document();
        let before = doc.clone();
        let mut history = History::default();
        let err = history.execute(&mut doc, Command::RemoveNode(NodeId(999_999))).unwrap_err();
        assert_eq!(err, CommandError::UnknownNode(NodeId(999_999)));
        let duplicate = doc.nodes[0].clone();
        assert!(history.execute(&mut doc, Command::AddNode { node: duplicate, index: None }).is_err());
        assert_eq!(doc, before);
        assert!(!history.can_undo());
    }

    #[test]
    fn history_limit_drops_oldest() {
        let mut doc = demo_document();
        let mut history = History::new(2);
        let id = some_node(&doc);
        for name in ["a", "b", "c"] {
            history.execute(&mut doc, Command::Rename { id, name: name.into() }).unwrap();
        }
        assert!(history.undo(&mut doc).is_some());
        assert!(history.undo(&mut doc).is_some());
        assert!(history.undo(&mut doc).is_none());
        assert_eq!(doc.node(id).unwrap().name, "a");
    }
}
