//! What the user has selected. Selection is UI state: it is not saved and not undoable
//! (M1-SPEC D9).

use crate::model::{Document, NodeId, PlatformId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectionTarget {
    Node(NodeId),
    Platform(PlatformId),
    Room,
}

impl SelectionTarget {
    /// Whether the selected item is still in `doc` (the room always is).
    pub fn exists_in(self, doc: &Document) -> bool {
        match self {
            SelectionTarget::Node(id) => doc.node(id).is_some(),
            SelectionTarget::Platform(id) => doc.platform(id).is_some(),
            SelectionTarget::Room => true,
        }
    }
}
