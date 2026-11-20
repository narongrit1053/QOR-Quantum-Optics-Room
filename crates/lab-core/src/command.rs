//! Edits to a [`Document`], each returning its inverse so it can be undone (M1-SPEC §3.7).
//!
//! Every command validates first and then mutates, so on `Err` the document is bit-identical to
//! before. Platform and room edits are whole-value replacements; [`PlatformAspects`] says which
//! parts of a platform changed so projections can update just those.

use glam::{DQuat, DVec3};

use crate::limits::MAX_ID;
use crate::model::{Attach, Document, Node, NodeId, Parent, Platform, PlatformId, Support};
use crate::part::Params;
use crate::room::Room;
use crate::selection::SelectionTarget;
use crate::units::Mm;
use crate::validate::{ParentProblem, PlatformError, RoomError};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Adds `node`. With `index`, inserts at that position (used when undoing a removal so
    /// the document's order is restored exactly).
    AddNode {
        node: Node,
        index: Option<usize>,
    },
    /// Removes the nodes and all their descendants.
    RemoveNodes(Vec<NodeId>),
    /// Inverse of `RemoveNodes`: each `(index, node)` in ascending index order.
    RestoreNodes(Vec<(usize, Node)>),
    /// Adds the nodes in order (parents may be earlier nodes of the list) as one step.
    PlaceAssembly {
        nodes: Vec<Node>,
    },
    /// §4.3's SetTransform.
    SetPlacement {
        id: NodeId,
        position: DVec3,
        rotation: DQuat,
    },
    Rename {
        id: NodeId,
        name: String,
    },
    SetAttach {
        id: NodeId,
        parent: Parent,
        attach: Attach,
        position: DVec3,
        rotation: DQuat,
    },
    SetParams {
        id: NodeId,
        params: Params,
    },
    SetLocked {
        id: NodeId,
        locked: bool,
    },
    SetNote {
        id: NodeId,
        note: String,
    },
    /// Selection lock: plain clicks pass through the target.
    SetSelectLock {
        target: SelectionTarget,
        on: bool,
    },
    AddPlatform {
        platform: Box<Platform>,
        index: Option<usize>,
    },
    /// Removes the platform and every node rooted on it. Refused while platforms sit on it.
    RemovePlatform(PlatformId),
    /// Inverse of `RemovePlatform`: the platform at `index`, then each `(index, node)` in
    /// ascending index order.
    RestorePlatform {
        platform: Box<Platform>,
        index: usize,
        nodes: Vec<(usize, Node)>,
    },
    /// Replaces the platform with the same id.
    UpdatePlatform {
        platform: Box<Platform>,
    },
    SetRoom(Box<Room>),
    /// Applies the commands in order as one step (M6 set re-pose); all or nothing.
    Batch(Vec<Command>),
}

/// Which parts of a platform an update changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformAspects {
    pub name: bool,
    pub kind: bool,
    pub surface: bool,
    /// position, rotation_z or top_height.
    pub placement: bool,
    /// outline or thickness.
    pub geometry: bool,
    /// hole_grid or outline.
    pub holes: bool,
    /// support or geometry; also top_height on a platform with legs (the legs reach the floor).
    pub support: bool,
}

impl PlatformAspects {
    pub fn between(old: &Platform, new: &Platform) -> Self {
        let top_height = old.top_height != new.top_height;
        let geometry = old.outline != new.outline || old.thickness != new.thickness;
        let legs = matches!(new.support, Support::Legs { .. } | Support::Isolators { .. });
        Self {
            name: old.name != new.name,
            kind: old.kind != new.kind,
            surface: old.surface != new.surface,
            placement: old.position != new.position || old.rotation_z != new.rotation_z || top_height,
            geometry,
            holes: old.hole_grid != new.hole_grid || old.outline != new.outline,
            support: old.support != new.support || geometry || (top_height && legs),
        }
    }

    pub fn any(&self) -> bool {
        self.name || self.kind || self.surface || self.placement || self.geometry || self.holes || self.support
    }

    /// The platform's meshes, holes or legs must be rebuilt (not just re-placed).
    pub fn needs_rebuild(&self) -> bool {
        self.geometry || self.holes || self.support || self.surface
    }

    fn or(self, o: PlatformAspects) -> PlatformAspects {
        PlatformAspects {
            name: self.name || o.name,
            kind: self.kind || o.kind,
            surface: self.surface || o.surface,
            placement: self.placement || o.placement,
            geometry: self.geometry || o.geometry,
            holes: self.holes || o.holes,
            support: self.support || o.support,
        }
    }
}

/// What a command touched, so projections (viewport, scene tree) can update incrementally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    pub nodes_added: Vec<NodeId>,
    pub nodes_removed: Vec<NodeId>,
    pub nodes_updated: Vec<NodeId>,
    pub platforms_added: Vec<PlatformId>,
    pub platforms_removed: Vec<PlatformId>,
    pub platforms_updated: Vec<(PlatformId, PlatformAspects)>,
    pub room_changed: bool,
}

fn union<T: PartialEq>(into: &mut Vec<T>, from: Vec<T>) {
    for x in from {
        if !into.contains(&x) {
            into.push(x);
        }
    }
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.nodes_updated.is_empty()
            && self.platforms_added.is_empty()
            && self.platforms_removed.is_empty()
            && self.platforms_updated.is_empty()
            && !self.room_changed
    }

    /// Union of both change sets, deduplicated by id; aspects of the same platform are OR-ed.
    pub fn merge(&mut self, other: ChangeSet) {
        union(&mut self.nodes_added, other.nodes_added);
        union(&mut self.nodes_removed, other.nodes_removed);
        union(&mut self.nodes_updated, other.nodes_updated);
        union(&mut self.platforms_added, other.platforms_added);
        union(&mut self.platforms_removed, other.platforms_removed);
        for (id, aspects) in other.platforms_updated {
            match self.platforms_updated.iter_mut().find(|(p, _)| *p == id) {
                Some((_, a)) => *a = a.or(aspects),
                None => self.platforms_updated.push((id, aspects)),
            }
        }
        self.room_changed |= other.room_changed;
    }

    pub fn touches(&self, t: SelectionTarget) -> bool {
        match t {
            SelectionTarget::Node(id) => {
                self.nodes_added.contains(&id) || self.nodes_removed.contains(&id) || self.nodes_updated.contains(&id)
            }
            SelectionTarget::Platform(id) => {
                self.platforms_added.contains(&id)
                    || self.platforms_removed.contains(&id)
                    || self.platforms_updated.iter().any(|(p, _)| *p == id)
            }
            SelectionTarget::Room => self.room_changed,
        }
    }

    /// `Some` when the change touches exactly one platform (added or updated, possibly with its
    /// nodes) and nothing else, or only the room.
    pub fn single_target(&self) -> Option<SelectionTarget> {
        let mut platforms: Vec<PlatformId> = self.platforms_added.clone();
        union(&mut platforms, self.platforms_updated.iter().map(|(p, _)| *p).collect());
        let no_nodes = self.nodes_added.is_empty() && self.nodes_removed.is_empty() && self.nodes_updated.is_empty();
        if !self.platforms_removed.is_empty() {
            return None;
        }
        match (platforms.as_slice(), self.room_changed) {
            ([id], false) => Some(SelectionTarget::Platform(*id)),
            ([], true) if no_nodes => Some(SelectionTarget::Room),
            _ => None,
        }
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
    #[error("no platform with id {0:?}")]
    NoSuchPlatform(PlatformId),
    #[error("id {0} is already in use")]
    DuplicateId(u64),
    #[error("{0}")]
    InvalidPlatform(PlatformError),
    #[error("{0}")]
    InvalidRoom(RoomError),
    #[error("internal: bad restore index")]
    BadIndex,
    /// Not in M1-SPEC §3.7: an id above MAX_ID (ids cross into Slint `int`).
    #[error("id {0} is too large")]
    IdTooLarge(u64),
    #[error("node {0:?} is attached to a part that does not exist")]
    UnknownParent(NodeId),
    /// A node or platform (raw id) would end up attached to itself.
    #[error("item {0} would be attached to itself")]
    ParentCycle(u64),
    #[error("other platforms sit on platform {0:?}")]
    PlatformHasChildren(PlatformId),
}

/// Checks the parent chain of each of `nodes` (which may refer to each other).
fn check_parents(doc: &Document, nodes: &[Node], platform: Option<PlatformId>) -> Result<(), CommandError> {
    for n in nodes {
        match doc.parent_problem(nodes, platform, n) {
            None => {}
            Some(ParentProblem::UnknownPlatform) => return Err(CommandError::UnknownPlatform(n.id)),
            Some(ParentProblem::UnknownNode) => return Err(CommandError::UnknownParent(n.id)),
            Some(ParentProblem::Cycle) => return Err(CommandError::ParentCycle(n.id.0)),
        }
    }
    Ok(())
}

/// New nodes: unused, unique ids within MAX_ID.
fn check_new_nodes<'a>(doc: &Document, nodes: impl IntoIterator<Item = &'a Node>) -> Result<(), CommandError> {
    let mut ids = Vec::new();
    for n in nodes {
        if doc.node(n.id).is_some() {
            return Err(CommandError::DuplicateNode(n.id));
        }
        if ids.contains(&n.id) {
            return Err(CommandError::DuplicateId(n.id.0));
        }
        check_new_id(doc, n.id.0)?;
        ids.push(n.id);
    }
    Ok(())
}

/// An `OnPlatform` support must name an existing platform without forming a loop.
fn check_stacking(doc: &Document, platform: &Platform) -> Result<(), CommandError> {
    if let Support::OnPlatform { parent, .. } = platform.support {
        if parent != platform.id && doc.platform(parent).is_none() {
            return Err(CommandError::NoSuchPlatform(parent));
        }
        if doc.platform_cycle(platform) {
            return Err(CommandError::ParentCycle(platform.id.0));
        }
    }
    Ok(())
}

/// A new object's id must be unused and at most MAX_ID.
fn check_new_id(doc: &Document, raw: u64) -> Result<(), CommandError> {
    if doc.contains_id(raw) {
        return Err(CommandError::DuplicateId(raw));
    }
    if raw > MAX_ID {
        return Err(CommandError::IdTooLarge(raw));
    }
    Ok(())
}

impl Command {
    /// Applies the command to `doc`. Returns the inverse command and the resulting change set.
    /// On error the document is left unchanged.
    pub fn apply(self, doc: &mut Document) -> Result<(Command, ChangeSet), CommandError> {
        match self {
            Command::Batch(commands) => {
                let mut inverses = Vec::new();
                let mut changes = ChangeSet::default();
                for command in commands {
                    match command.apply(doc) {
                        Ok((inverse, c)) => {
                            inverses.push(inverse);
                            changes.merge(c);
                        }
                        Err(e) => {
                            for inverse in inverses.into_iter().rev() {
                                let _ = inverse.apply(doc);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok((Command::Batch(inverses), changes))
            }
            Command::AddNode { node, index } => {
                check_new_nodes(doc, [&node])?;
                check_parents(doc, std::slice::from_ref(&node), None)?;
                let id = node.id;
                let index = index.unwrap_or(doc.nodes.len()).min(doc.nodes.len());
                doc.nodes.insert(index, node);
                doc.bump_next_id(id.0);
                Ok((Command::RemoveNodes(vec![id]), ChangeSet { nodes_added: vec![id], ..Default::default() }))
            }
            Command::RemoveNodes(ids) => {
                let mut all = Vec::new();
                for id in ids {
                    if doc.node(id).is_none() {
                        return Err(CommandError::UnknownNode(id));
                    }
                    for x in std::iter::once(id).chain(doc.descendants(id)) {
                        if !all.contains(&x) {
                            all.push(x);
                        }
                    }
                }
                let mut removed = Vec::new();
                let mut kept = Vec::with_capacity(doc.nodes.len());
                for (i, node) in std::mem::take(&mut doc.nodes).into_iter().enumerate() {
                    if all.contains(&node.id) { removed.push((i, node)) } else { kept.push(node) }
                }
                doc.nodes = kept;
                let changes = ChangeSet { nodes_removed: removed.iter().map(|(_, n)| n.id).collect(), ..Default::default() };
                Ok((Command::RestoreNodes(removed), changes))
            }
            Command::RestoreNodes(mut nodes) => {
                nodes.sort_by_key(|(i, _)| *i);
                check_new_nodes(doc, nodes.iter().map(|(_, n)| n))?;
                for (len, (i, _)) in (doc.nodes.len()..).zip(&nodes) {
                    if *i > len {
                        return Err(CommandError::BadIndex);
                    }
                }
                let list: Vec<Node> = nodes.iter().map(|(_, n)| n.clone()).collect();
                check_parents(doc, &list, None)?;
                let mut added = Vec::with_capacity(nodes.len());
                for (i, node) in nodes {
                    added.push(node.id);
                    doc.bump_next_id(node.id.0);
                    doc.nodes.insert(i, node);
                }
                Ok((Command::RemoveNodes(added.clone()), ChangeSet { nodes_added: added, ..Default::default() }))
            }
            Command::PlaceAssembly { nodes } => {
                check_new_nodes(doc, &nodes)?;
                check_parents(doc, &nodes, None)?;
                let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
                for node in nodes {
                    doc.bump_next_id(node.id.0);
                    doc.nodes.push(node);
                }
                Ok((Command::RemoveNodes(ids.clone()), ChangeSet { nodes_added: ids, ..Default::default() }))
            }
            Command::SetPlacement { id, position, rotation } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let inverse = Command::SetPlacement { id, position: node.position, rotation: node.rotation };
                let changed = node.position != position || node.rotation != rotation;
                node.position = position;
                node.rotation = rotation;
                Ok((inverse, node_update(id, changed)))
            }
            Command::Rename { id, name } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let changed = node.name != name;
                let old = std::mem::replace(&mut node.name, name);
                Ok((Command::Rename { id, name: old }, node_update(id, changed)))
            }
            Command::SetAttach { id, parent, attach, position, rotation } => {
                let node = doc.node(id).ok_or(CommandError::UnknownNode(id))?;
                match parent {
                    Parent::Room => {}
                    Parent::Platform(p) if doc.platform(p).is_none() => return Err(CommandError::UnknownPlatform(id)),
                    Parent::Platform(_) => {}
                    Parent::Node(p) if doc.node(p).is_none() => return Err(CommandError::UnknownParent(id)),
                    Parent::Node(p) if p == id || doc.descendants(id).contains(&p) => {
                        return Err(CommandError::ParentCycle(id.0));
                    }
                    Parent::Node(_) => {}
                }
                let inverse = Command::SetAttach {
                    id,
                    parent: node.parent,
                    attach: node.attach.clone(),
                    position: node.position,
                    rotation: node.rotation,
                };
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let changed = node.parent != parent
                    || node.attach != attach
                    || node.position != position
                    || node.rotation != rotation;
                node.parent = parent;
                node.attach = attach;
                node.position = position;
                node.rotation = rotation;
                Ok((inverse, node_update(id, changed)))
            }
            Command::SetParams { id, params } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let changed = node.params != params;
                let old = std::mem::replace(&mut node.params, params);
                Ok((Command::SetParams { id, params: old }, node_update(id, changed)))
            }
            Command::SetLocked { id, locked } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let changed = node.locked != locked;
                let old = std::mem::replace(&mut node.locked, locked);
                Ok((Command::SetLocked { id, locked: old }, node_update(id, changed)))
            }
            Command::SetSelectLock { target, on } => {
                let (slot, changes) = match target {
                    SelectionTarget::Node(id) => {
                        let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                        let changed = node.select_lock != on;
                        (&mut node.select_lock, node_update(id, changed))
                    }
                    SelectionTarget::Platform(id) => {
                        let index = doc.platform_index(id).ok_or(CommandError::NoSuchPlatform(id))?;
                        let p = &mut doc.platforms[index];
                        let changed = p.select_lock != on;
                        let changes = if changed {
                            ChangeSet { platforms_updated: vec![(id, PlatformAspects::default())], ..Default::default() }
                        } else {
                            ChangeSet::default()
                        };
                        (&mut p.select_lock, changes)
                    }
                    SelectionTarget::Room => return Ok((Command::SetSelectLock { target, on }, ChangeSet::default())),
                };
                let old = std::mem::replace(slot, on);
                Ok((Command::SetSelectLock { target, on: old }, changes))
            }
            Command::SetNote { id, note } => {
                let node = doc.node_mut(id).ok_or(CommandError::UnknownNode(id))?;
                let changed = node.note != note;
                let old = std::mem::replace(&mut node.note, note);
                Ok((Command::SetNote { id, note: old }, node_update(id, changed)))
            }
            Command::AddPlatform { platform, index } => {
                let id = platform.id;
                check_new_id(doc, id.0)?;
                platform.validate().map_err(CommandError::InvalidPlatform)?;
                check_stacking(doc, &platform)?;
                let index = index.unwrap_or(doc.platforms.len()).min(doc.platforms.len());
                doc.platforms.insert(index, *platform);
                doc.bump_next_id(id.0);
                Ok((Command::RemovePlatform(id), ChangeSet { platforms_added: vec![id], ..Default::default() }))
            }
            Command::RemovePlatform(id) => {
                let index = doc.platform_index(id).ok_or(CommandError::NoSuchPlatform(id))?;
                if doc.platforms.iter().any(|p| matches!(p.support, Support::OnPlatform { parent, .. } if parent == id)) {
                    return Err(CommandError::PlatformHasChildren(id));
                }
                let on: Vec<NodeId> = doc.nodes_on(id).map(|n| n.id).collect();
                let platform = doc.platforms.remove(index);
                let mut nodes = Vec::new();
                let mut kept = Vec::with_capacity(doc.nodes.len());
                for (i, node) in std::mem::take(&mut doc.nodes).into_iter().enumerate() {
                    if on.contains(&node.id) {
                        nodes.push((i, node));
                    } else {
                        kept.push(node);
                    }
                }
                doc.nodes = kept;
                let changes = ChangeSet {
                    platforms_removed: vec![id],
                    nodes_removed: nodes.iter().map(|(_, n)| n.id).collect(),
                    ..Default::default()
                };
                Ok((Command::RestorePlatform { platform: Box::new(platform), index, nodes }, changes))
            }
            Command::RestorePlatform { platform, index, mut nodes } => {
                let id = platform.id;
                nodes.sort_by_key(|(i, _)| *i);
                let mut ids = vec![id.0];
                for (_, n) in &nodes {
                    if ids.contains(&n.id.0) {
                        return Err(CommandError::DuplicateId(n.id.0));
                    }
                    ids.push(n.id.0);
                }
                for &raw in &ids {
                    check_new_id(doc, raw)?;
                }
                if index > doc.platforms.len() {
                    return Err(CommandError::BadIndex);
                }
                for (len, (i, _)) in (doc.nodes.len()..).zip(&nodes) {
                    if *i > len {
                        return Err(CommandError::BadIndex);
                    }
                }
                let list: Vec<Node> = nodes.iter().map(|(_, n)| n.clone()).collect();
                check_parents(doc, &list, Some(id))?;
                platform.validate().map_err(CommandError::InvalidPlatform)?;
                check_stacking(doc, &platform)?;
                doc.platforms.insert(index, *platform);
                let mut added = Vec::with_capacity(nodes.len());
                for (i, node) in nodes {
                    added.push(node.id);
                    doc.nodes.insert(i, node);
                }
                if let Some(&max) = ids.iter().max() {
                    doc.bump_next_id(max);
                }
                Ok((
                    Command::RemovePlatform(id),
                    ChangeSet { platforms_added: vec![id], nodes_added: added, ..Default::default() },
                ))
            }
            Command::UpdatePlatform { platform } => {
                let id = platform.id;
                let index = doc.platform_index(id).ok_or(CommandError::NoSuchPlatform(id))?;
                platform.validate().map_err(CommandError::InvalidPlatform)?;
                check_stacking(doc, &platform)?;
                let old = std::mem::replace(&mut doc.platforms[index], *platform);
                let new = &doc.platforms[index];
                let changes = if old == *new {
                    ChangeSet::default()
                } else {
                    ChangeSet {
                        platforms_updated: vec![(id, PlatformAspects::between(&old, new))],
                        ..Default::default()
                    }
                };
                Ok((Command::UpdatePlatform { platform: Box::new(old) }, changes))
            }
            Command::SetRoom(room) => {
                room.validate().map_err(CommandError::InvalidRoom)?;
                for o in &room.openings {
                    let raw = o.id.0;
                    if doc.nodes.iter().any(|n| n.id.0 == raw) || doc.platforms.iter().any(|p| p.id.0 == raw) {
                        return Err(CommandError::DuplicateId(raw));
                    }
                    if raw > MAX_ID {
                        return Err(CommandError::IdTooLarge(raw));
                    }
                }
                let old = std::mem::replace(&mut doc.room, *room);
                if let Some(max) = doc.room.openings.iter().map(|o| o.id.0).max() {
                    doc.bump_next_id(max);
                }
                let room_changed = old != doc.room;
                Ok((Command::SetRoom(Box::new(old)), ChangeSet { room_changed, ..Default::default() }))
            }
        }
    }

    /// Mates `id` to node `parent` (connector `self_conn` of `id` to `parent_conn`); `position`
    /// and `rotation` are the resulting pose in the parent's frame.
    #[allow(clippy::too_many_arguments)]
    pub fn mate(
        id: NodeId,
        parent: NodeId,
        self_conn: impl Into<String>,
        parent_conn: impl Into<String>,
        slide: Mm,
        twist: f64,
        position: DVec3,
        rotation: DQuat,
    ) -> Command {
        let attach = Attach::Mate { self_conn: self_conn.into(), parent_conn: parent_conn.into(), slide, twist };
        Command::SetAttach { id, parent: Parent::Node(parent), attach, position, rotation }
    }

    /// Detaches `id` to its root platform (or the room) as `Attach::Free`; `position` and
    /// `rotation` are in that frame. Deviation from docs/M2-PLAN.md: takes `doc` to find the root.
    pub fn unmate(doc: &Document, id: NodeId, position: DVec3, rotation: DQuat) -> Command {
        let parent = doc.root_platform(id).map_or(Parent::Room, Parent::Platform);
        Command::SetAttach { id, parent, attach: Attach::Free, position, rotation }
    }

    /// The undo label, computed before the command is applied ("Add Table B", "Edit Room").
    pub fn label(&self, doc: &Document) -> String {
        let node_name = |id: &NodeId| doc.node(*id).map_or_else(|| "Part".to_string(), |n| n.name.clone());
        match self {
            Command::AddNode { node, .. } => format!("Add {}", node.name),
            Command::RemoveNodes(ids) => match ids.as_slice() {
                [id] => format!("Delete {}", node_name(id)),
                _ => format!("Delete {} parts", ids.len()),
            },
            Command::RestoreNodes(nodes) => match nodes.as_slice() {
                [(_, n)] => format!("Restore {}", n.name),
                _ => format!("Restore {} parts", nodes.len()),
            },
            Command::PlaceAssembly { nodes } => match nodes.first() {
                Some(n) if nodes.len() == 1 => format!("Add {}", n.name),
                _ => format!("Add {} parts", nodes.len()),
            },
            Command::SetAttach { id, attach, .. } => {
                let was_mated = doc.node(*id).is_some_and(|n| matches!(n.attach, Attach::Mate { .. }));
                match attach {
                    Attach::Mate { .. } => format!("Mate {}", node_name(id)),
                    Attach::Joint(j) => format!("Joint {:?}", j.kind),
                    _ if was_mated => format!("Unmate {}", node_name(id)),
                    _ => format!("Move {}", node_name(id)),
                }
            }
            Command::SetParams { id, .. } => format!("Edit {}", node_name(id)),
            Command::SetLocked { id, locked: true } => format!("Lock {}", node_name(id)),
            Command::SetLocked { id, locked: false } => format!("Unlock {}", node_name(id)),
            Command::SetPlacement { id, .. } => format!("Move {}", node_name(id)),
            Command::Rename { id, .. } => format!("Rename {}", node_name(id)),
            Command::SetNote { .. } => "Edit Note".into(),
            Command::SetSelectLock { target, on } => {
                let name = match target {
                    SelectionTarget::Node(id) => node_name(id),
                    SelectionTarget::Platform(id) => doc.platform(*id).map_or_else(|| "Platform".into(), |p| p.name.clone()),
                    SelectionTarget::Room => "Room".into(),
                };
                format!("{} {name}", if *on { "Lock" } else { "Unlock" })
            }
            Command::AddPlatform { platform, .. } => format!("Add {}", platform.name),
            Command::RemovePlatform(id) => {
                format!("Delete {}", doc.platform(*id).map_or("Platform", |p| p.name.as_str()))
            }
            Command::RestorePlatform { platform, .. } => format!("Restore {}", platform.name),
            Command::UpdatePlatform { platform } => {
                format!("Edit {}", doc.platform(platform.id).map_or(platform.name.as_str(), |p| p.name.as_str()))
            }
            Command::SetRoom(_) => "Edit Room".into(),
            Command::Batch(commands) => commands.first().map_or_else(String::new, |c| c.label(doc)),
        }
    }
}

fn node_update(id: NodeId, changed: bool) -> ChangeSet {
    if changed { ChangeSet { nodes_updated: vec![id], ..Default::default() } } else { ChangeSet::default() }
}

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use super::*;
    use crate::history::History;
    use crate::holes::{ExclusionZone, GridOrigin, Thread, ZoneShape};
    use crate::model::{OpeningId, PlatformKind, Surface};
    use crate::outline::Outline;
    use crate::presets::{platform_from_preset, preset};
    use crate::room::Opening;
    use crate::sample::demo_document;

    fn some_node(doc: &Document) -> NodeId {
        doc.nodes[3].id
    }

    // ---- Ported M0 tests ----

    #[test]
    fn remove_then_undo_restores_document_exactly() {
        let mut doc = demo_document();
        let before = doc.clone();
        let mut history = History::default();
        let id = some_node(&doc);

        let changes = history.execute(&mut doc, Command::RemoveNodes(vec![id])).unwrap();
        assert_eq!(changes.nodes_removed, vec![id]);
        assert!(doc.node(id).is_none());

        history.undo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&before));

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
            .execute(
                &mut doc,
                Command::SetPlacement { id, position: DVec3::new(1.0, 2.0, 3.0), rotation: DQuat::IDENTITY },
            )
            .unwrap();
        history.execute(&mut doc, Command::Rename { id, name: "M9".into() }).unwrap();
        assert_eq!(doc.node(id).unwrap().name, "M9");

        history.undo(&mut doc).unwrap().unwrap();
        history.undo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&before));
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
        let err = history.execute(&mut doc, Command::RemoveNodes(vec![NodeId(999_999)])).unwrap_err();
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

    // ---- C1: every command round-trips through its inverse ----

    fn breadboard(doc: &Document, on: Option<PlatformId>) -> Platform {
        platform_from_preset(doc, preset("bb-m-600x600").unwrap(), PlatformId(doc.next_free_id()), on)
    }

    /// Applies `command`, then its inverse, and checks the round trip and the inverse's inverse.
    fn round_trip(doc: &Document, command: Command) -> ChangeSet {
        let mut d = doc.clone();
        let (inverse, changes) = command.clone().apply(&mut d).unwrap();
        assert!(!d.content_eq(doc) || changes.is_empty(), "{command:?} changed nothing but reported {changes:?}");
        let (again, _) = inverse.apply(&mut d).unwrap();
        assert!(d.content_eq(doc), "{command:?} did not round-trip");
        assert_eq!(d.validate(), Ok(()));
        // The inverse of the inverse redoes the command.
        let mut redo = doc.clone();
        command.apply(&mut redo).unwrap();
        let mut d2 = d.clone();
        again.apply(&mut d2).unwrap();
        assert!(d2.content_eq(&redo));
        changes
    }

    fn assert_rejected(doc: &Document, command: Command, expected: impl Fn(&CommandError) -> bool) {
        let mut d = doc.clone();
        let err = command.apply(&mut d).unwrap_err();
        assert!(expected(&err), "{err:?}");
        assert_eq!(&d, doc, "a failed command must leave the document bit-identical");
    }

    #[test]
    fn select_lock_round_trips_and_serializes() {
        let doc = demo_document();
        let table = doc.platforms[0].id;
        let node = doc.nodes[0].id;
        round_trip(&doc, Command::SetSelectLock { target: SelectionTarget::Platform(table), on: true });
        round_trip(&doc, Command::SetSelectLock { target: SelectionTarget::Node(node), on: true });
        let mut d = doc.clone();
        Command::SetSelectLock { target: SelectionTarget::Platform(table), on: true }.apply(&mut d).unwrap();
        Command::SetSelectLock { target: SelectionTarget::Node(node), on: true }.apply(&mut d).unwrap();
        let back: Document = serde_json::from_str(&d.to_json().unwrap()).unwrap();
        assert!(back.platforms[0].select_lock && back.nodes[0].select_lock);
        assert!(!doc.to_json().unwrap().contains("select_lock"), "unlocked docs serialize unchanged");
    }

    #[test]
    fn c1_every_command_round_trips() {
        let doc = demo_document();
        let node = doc.nodes[5].clone();
        let id = node.id;
        let table = doc.platforms[0].id;

        let mut new_node = node.clone();
        new_node.id = NodeId(doc.next_free_id());
        let changes = round_trip(&doc, Command::AddNode { node: new_node.clone(), index: Some(2) });
        assert_eq!(changes.nodes_added, vec![new_node.id]);
        round_trip(&doc, Command::RemoveNodes(vec![id]));
        round_trip(&doc, Command::SetPlacement { id, position: DVec3::ONE, rotation: DQuat::from_rotation_z(1.0) });
        round_trip(&doc, Command::Rename { id, name: "Renamed".into() });

        let bb = breadboard(&doc, Some(table));
        let changes = round_trip(&doc, Command::AddPlatform { platform: Box::new(bb.clone()), index: Some(0) });
        assert_eq!(changes.platforms_added, vec![bb.id]);
        let changes = round_trip(&doc, Command::RemovePlatform(table));
        assert_eq!(changes.platforms_removed, vec![table]);
        assert_eq!(changes.nodes_removed.len(), doc.nodes.len());

        let mut edited = doc.platforms[0].clone();
        edited.outline = Outline::RoundedRectangle { size: DVec2::new(1200.0, 900.0), radius: 50.0 };
        edited.name = "Main table".into();
        let changes = round_trip(&doc, Command::UpdatePlatform { platform: Box::new(edited) });
        let aspects = changes.platforms_updated[0].1;
        assert!(aspects.name && aspects.geometry && aspects.holes && aspects.support && !aspects.placement);

        let mut room = doc.room.clone();
        room.openings.push(Opening {
            id: OpeningId(doc.next_free_id()),
            name: "Door 1".into(),
            wall: 0,
            offset: 600.0,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        });
        let changes = round_trip(&doc, Command::SetRoom(Box::new(room.clone())));
        assert!(changes.room_changed);

        // RestorePlatform (the inverse of RemovePlatform) as a command of its own.
        let mut removed = doc.clone();
        let (restore, _) = Command::RemovePlatform(table).apply(&mut removed).unwrap();
        round_trip(&removed, restore);

        // Ids only grow: adding then undoing keeps next_id raised.
        let mut d = doc.clone();
        let (inverse, _) = Command::SetRoom(Box::new(room)).apply(&mut d).unwrap();
        inverse.apply(&mut d).unwrap();
        assert_eq!(d.next_free_id(), doc.next_free_id() + 1);
    }

    #[test]
    fn c1_errors_leave_the_document_bit_identical() {
        let doc = demo_document();
        let table = doc.platforms[0].clone();
        let node = doc.nodes[0].clone();
        assert_rejected(&doc, Command::RemoveNodes(vec![NodeId(9999)]), |e| *e == CommandError::UnknownNode(NodeId(9999)));
        assert_rejected(&doc, Command::AddNode { node: node.clone(), index: None }, |e| {
            *e == CommandError::DuplicateNode(node.id)
        });
        let mut on_table_id = node.clone();
        on_table_id.id = NodeId(table.id.0);
        assert_rejected(&doc, Command::AddNode { node: on_table_id, index: None }, |e| {
            *e == CommandError::DuplicateId(table.id.0)
        });
        let mut orphan = node.clone();
        orphan.id = NodeId(doc.next_free_id());
        orphan.parent = Parent::Platform(PlatformId(4242));
        assert_rejected(&doc, Command::AddNode { node: orphan.clone(), index: None }, |e| {
            *e == CommandError::UnknownPlatform(orphan.id)
        });
        let mut huge = node.clone();
        huge.id = NodeId(MAX_ID + 1);
        assert_rejected(&doc, Command::AddNode { node: huge, index: None }, |e| {
            *e == CommandError::IdTooLarge(MAX_ID + 1)
        });
        assert_rejected(
            &doc,
            Command::SetPlacement { id: NodeId(9999), position: DVec3::ZERO, rotation: DQuat::IDENTITY },
            |e| *e == CommandError::UnknownNode(NodeId(9999)),
        );
        assert_rejected(&doc, Command::Rename { id: NodeId(9999), name: "x".into() }, |e| {
            *e == CommandError::UnknownNode(NodeId(9999))
        });

        assert_rejected(&doc, Command::AddPlatform { platform: Box::new(table.clone()), index: None }, |e| {
            *e == CommandError::DuplicateId(table.id.0)
        });
        let mut bad = breadboard(&doc, None);
        bad.thickness = 0.0;
        assert_rejected(&doc, Command::AddPlatform { platform: Box::new(bad.clone()), index: None }, |e| {
            *e == CommandError::InvalidPlatform(PlatformError::Thickness)
        });
        let mut huge = breadboard(&doc, None);
        huge.id = PlatformId(MAX_ID + 7);
        assert_rejected(&doc, Command::AddPlatform { platform: Box::new(huge), index: None }, |e| {
            *e == CommandError::IdTooLarge(MAX_ID + 7)
        });
        assert_rejected(&doc, Command::RemovePlatform(PlatformId(4242)), |e| {
            *e == CommandError::NoSuchPlatform(PlatformId(4242))
        });
        bad.id = table.id;
        assert_rejected(&doc, Command::UpdatePlatform { platform: Box::new(bad.clone()) }, |e| {
            matches!(e, CommandError::InvalidPlatform(PlatformError::Thickness))
        });
        bad.id = PlatformId(4242);
        assert_rejected(&doc, Command::UpdatePlatform { platform: Box::new(bad) }, |e| {
            *e == CommandError::NoSuchPlatform(PlatformId(4242))
        });

        let mut room = doc.room.clone();
        room.wall_height = 10.0;
        assert_rejected(&doc, Command::SetRoom(Box::new(room)), |e| *e == CommandError::InvalidRoom(RoomError::Height));
        let mut room = doc.room.clone();
        room.openings.push(Opening {
            id: OpeningId(node.id.0),
            name: "Door 1".into(),
            wall: 0,
            offset: 600.0,
            width: 1000.0,
            height: 2100.0,
            sill: 0.0,
        });
        assert_rejected(&doc, Command::SetRoom(Box::new(room.clone())), |e| *e == CommandError::DuplicateId(node.id.0));
        room.openings[0].id = OpeningId(MAX_ID + 1);
        assert_rejected(&doc, Command::SetRoom(Box::new(room)), |e| *e == CommandError::IdTooLarge(MAX_ID + 1));

        // Restore with clashing ids or bad indices.
        let mut removed = doc.clone();
        let (restore, _) = Command::RemovePlatform(table.id).apply(&mut removed).unwrap();
        let Command::RestorePlatform { platform, index, nodes } = restore else { unreachable!() };
        assert_rejected(
            &doc,
            Command::RestorePlatform { platform: platform.clone(), index, nodes: nodes.clone() },
            |e| *e == CommandError::DuplicateId(table.id.0),
        );
        assert_rejected(
            &removed,
            Command::RestorePlatform { platform: platform.clone(), index: 3, nodes: nodes.clone() },
            |e| *e == CommandError::BadIndex,
        );
        let mut gap = nodes.clone();
        gap[0].0 = 500;
        assert_rejected(&removed, Command::RestorePlatform { platform: platform.clone(), index, nodes: gap }, |e| {
            *e == CommandError::BadIndex
        });
        let mut twice = nodes.clone();
        twice[1].1.id = twice[0].1.id;
        assert_rejected(&removed, Command::RestorePlatform { platform, index, nodes: twice }, |e| {
            matches!(e, CommandError::DuplicateId(_))
        });
    }

    #[test]
    fn c1_no_op_update_records_nothing() {
        let mut doc = demo_document();
        let mut history = History::default();
        let same = doc.platforms[0].clone();
        let changes = history.execute(&mut doc, Command::UpdatePlatform { platform: Box::new(same) }).unwrap();
        assert!(changes.is_empty());
        assert!(!history.can_undo());
        assert_eq!(history.state_id(), 0);
        let room = doc.room.clone();
        assert!(history.execute(&mut doc, Command::SetRoom(Box::new(room))).unwrap().is_empty());
        let id = doc.nodes[0].id;
        let name = doc.nodes[0].name.clone();
        assert!(history.execute(&mut doc, Command::Rename { id, name }).unwrap().is_empty());
        assert!(!history.can_undo());
    }

    #[test]
    fn c1_remove_platform_then_undo_restores_node_order() {
        let mut doc = demo_document();
        let table = doc.platforms[0].id;
        let mut history = History::default();
        // A second platform with nodes interleaved between the table's nodes.
        let bb = breadboard(&doc, None);
        let bb_id = bb.id;
        history.execute(&mut doc, Command::AddPlatform { platform: Box::new(bb), index: None }).unwrap();
        for k in [0usize, 7, 15, 30] {
            let mut node = doc.nodes[k].clone();
            node.id = NodeId(doc.next_free_id());
            node.parent = Parent::Platform(bb_id);
            history.execute(&mut doc, Command::AddNode { node, index: Some(k) }).unwrap();
        }
        let before = doc.clone();
        let label = Command::RemovePlatform(table).label(&doc);
        assert_eq!(label, "Delete Table A");
        let changes = history.execute(&mut doc, Command::RemovePlatform(table)).unwrap();
        assert_eq!(doc.nodes.len(), 4);
        assert!(doc.nodes.iter().all(|n| n.parent == Parent::Platform(bb_id)));
        assert_eq!(changes.single_target(), None);
        let changes = history.undo(&mut doc).unwrap().unwrap();
        assert_eq!(doc.nodes, before.nodes);
        assert!(doc.content_eq(&before));
        assert_eq!(changes.single_target(), Some(SelectionTarget::Platform(table)));
        assert_eq!(history.redo_label(), Some("Delete Table A"));
    }

    // ---- C2: aspects ----

    #[test]
    fn c2_aspects_for_each_field() {
        let base = demo_document().platforms[0].clone();
        let check = |edit: &dyn Fn(&mut Platform), expected: PlatformAspects| {
            let mut new = base.clone();
            edit(&mut new);
            assert_eq!(PlatformAspects::between(&base, &new), expected, "{new:?}");
        };
        let none = PlatformAspects::default();
        check(&|_| {}, none);
        assert!(!none.any());
        check(&|p| p.name = "B".into(), PlatformAspects { name: true, ..none });
        check(&|p| p.kind = PlatformKind::Custom, PlatformAspects { kind: true, ..none });
        check(&|p| p.surface = Surface::BlackAnodized, PlatformAspects { surface: true, ..none });
        check(&|p| p.position.x += 1.0, PlatformAspects { placement: true, ..none });
        check(&|p| p.rotation_z = 0.5, PlatformAspects { placement: true, ..none });
        // Legs reach the floor, so a new top height rebuilds them.
        check(&|p| p.top_height = 900.0, PlatformAspects { placement: true, support: true, ..none });
        check(
            &|p| {
                p.support = Support::Fixed;
                p.top_height = 900.0;
            },
            PlatformAspects { placement: true, support: true, ..none },
        );
        check(&|p| p.thickness = 300.0, PlatformAspects { geometry: true, support: true, ..none });
        check(
            &|p| p.outline = Outline::Rectangle { size: DVec2::new(1500.0, 900.0) },
            PlatformAspects { geometry: true, holes: true, support: true, ..none },
        );
        check(&|p| p.hole_grid = None, PlatformAspects { holes: true, ..none });
        check(&|p| p.hole_grid.as_mut().unwrap().thread = Thread::M4, PlatformAspects { holes: true, ..none });
        check(
            &|p| p.hole_grid.as_mut().unwrap().origin = GridOrigin::At(DVec2::ZERO),
            PlatformAspects { holes: true, ..none },
        );
        check(
            &|p| {
                p.hole_grid.as_mut().unwrap().exclusions.push(ExclusionZone {
                    name: "Z".into(),
                    shape: ZoneShape::Circle { center: DVec2::ZERO, radius: 9.0 },
                })
            },
            PlatformAspects { holes: true, ..none },
        );
        check(&|p| p.support = Support::Legs { size: 100.0, inset: 120.0 }, PlatformAspects { support: true, ..none });
        assert!(PlatformAspects { surface: true, ..none }.needs_rebuild());
        assert!(!PlatformAspects { placement: true, name: true, kind: true, ..none }.needs_rebuild());
    }

    #[test]
    fn change_set_helpers() {
        let p1 = PlatformId(1);
        let p2 = PlatformId(2);
        let mut a = ChangeSet {
            nodes_added: vec![NodeId(5)],
            platforms_updated: vec![(p1, PlatformAspects { name: true, ..Default::default() })],
            ..Default::default()
        };
        assert_eq!(a.single_target(), Some(SelectionTarget::Platform(p1)));
        assert!(a.touches(SelectionTarget::Node(NodeId(5))));
        assert!(a.touches(SelectionTarget::Platform(p1)));
        assert!(!a.touches(SelectionTarget::Room));
        a.merge(ChangeSet {
            nodes_added: vec![NodeId(5), NodeId(6)],
            platforms_updated: vec![(p1, PlatformAspects { holes: true, ..Default::default() })],
            ..Default::default()
        });
        assert_eq!(a.nodes_added, vec![NodeId(5), NodeId(6)]);
        assert_eq!(a.platforms_updated, vec![(p1, PlatformAspects { name: true, holes: true, ..Default::default() })]);
        a.merge(ChangeSet { platforms_added: vec![p2], ..Default::default() });
        assert_eq!(a.single_target(), None);
        let room = ChangeSet { room_changed: true, ..Default::default() };
        assert_eq!(room.single_target(), Some(SelectionTarget::Room));
        assert!(room.touches(SelectionTarget::Room));
        assert!(!room.is_empty());
        assert!(ChangeSet::default().is_empty());
        assert_eq!(ChangeSet::default().single_target(), None);
        let removed = ChangeSet { platforms_removed: vec![p1], ..Default::default() };
        assert_eq!(removed.single_target(), None);
        let nodes = ChangeSet { nodes_updated: vec![NodeId(3)], ..Default::default() };
        assert_eq!(nodes.single_target(), None);
    }

    #[test]
    fn labels() {
        let doc = demo_document();
        let node = &doc.nodes[0];
        let table = &doc.platforms[0];
        assert_eq!(Command::RemoveNodes(vec![node.id]).label(&doc), format!("Delete {}", node.name));
        assert_eq!(Command::Rename { id: node.id, name: "x".into() }.label(&doc), format!("Rename {}", node.name));
        assert_eq!(
            Command::SetPlacement { id: node.id, position: DVec3::ZERO, rotation: DQuat::IDENTITY }.label(&doc),
            format!("Move {}", node.name)
        );
        assert_eq!(Command::AddNode { node: node.clone(), index: None }.label(&doc), format!("Add {}", node.name));
        let mut renamed = table.clone();
        renamed.name = "New name".into();
        assert_eq!(Command::UpdatePlatform { platform: Box::new(renamed) }.label(&doc), "Edit Table A");
        assert_eq!(Command::AddPlatform { platform: Box::new(table.clone()), index: None }.label(&doc), "Add Table A");
        assert_eq!(Command::SetRoom(Box::new(doc.room.clone())).label(&doc), "Edit Room");
        assert_eq!(Command::RemovePlatform(PlatformId(999)).label(&doc), "Delete Platform");
    }

    // ---- M2: parents, attachments and assemblies ----

    fn part(id: u64, name: &str, parent: Parent, attach: Attach, z: f64) -> Node {
        Node { select_lock: false,
            note: String::new(),
            id: NodeId(id),
            name: name.into(),
            part: format!("thorlabs:{name}"),
            parent,
            attach,
            position: DVec3::new(0.0, 0.0, z),
            rotation: DQuat::IDENTITY,
            shape: crate::model::Shape::Part,
            finish: crate::model::Finish::AnodizedBlack,
            params: Default::default(),
            pickable: true,
            locked: false,
        }
    }

    fn mate(self_conn: &str, parent_conn: &str, slide: f64) -> Attach {
        Attach::Mate { self_conn: self_conn.into(), parent_conn: parent_conn.into(), slide, twist: 0.0 }
    }

    /// base -> holder -> post -> mount on the demo table.
    fn stack(doc: &Document) -> Vec<Node> {
        let first = doc.next_free_id();
        let table = doc.platforms[0].id;
        let hole = Attach::Hole { hole: crate::holes::LatticeIndex { i: 3, j: 4 } };
        vec![
            part(first, "BA1/M", Parent::Platform(table), hole, 0.0),
            part(first + 1, "PH50/M", Parent::Node(NodeId(first)), mate("bottom", "top", 0.0), 10.0),
            part(first + 2, "TR50/M", Parent::Node(NodeId(first + 1)), mate("bottom", "bore", -12.0), 38.0),
            part(first + 3, "KM100", Parent::Node(NodeId(first + 2)), mate("base_tap", "top", 0.0), 50.0),
        ]
    }

    #[test]
    fn place_assembly_undo_redo() {
        let mut doc = demo_document();
        let before = doc.clone();
        let mut history = History::default();
        let nodes = stack(&doc);
        let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
        let changes = history.execute(&mut doc, Command::PlaceAssembly { nodes }).unwrap();
        assert_eq!(changes.nodes_added, ids);
        assert_eq!(doc.validate(), Ok(()));
        let after = doc.clone();
        let table = doc.platforms[0].id;
        assert_eq!(doc.root_platform(ids[3]), Some(table));
        assert_eq!(doc.descendants(ids[0]), ids[1..].to_vec());
        assert_eq!(doc.node_pose(ids[3]).unwrap().0, DVec3::new(0.0, 0.0, 98.0));
        assert_eq!(doc.nodes_on(table).count(), before.nodes.len() + 4);

        history.undo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&before));
        history.redo(&mut doc).unwrap().unwrap();
        assert!(doc.content_eq(&after));

        // Removing the holder takes the post and the mount with it; undo restores them.
        let (restore, changes) = Command::RemoveNodes(vec![ids[1]]).apply(&mut doc).unwrap();
        assert_eq!(changes.nodes_removed, ids[1..].to_vec());
        restore.apply(&mut doc).unwrap();
        assert!(doc.content_eq(&after));
    }

    #[test]
    fn attach_commands_round_trip() {
        let mut doc = demo_document();
        let nodes = stack(&doc);
        let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
        Command::PlaceAssembly { nodes }.apply(&mut doc).unwrap();
        let unmate = Command::unmate(&doc, ids[2], DVec3::new(5.0, 0.0, 0.0), DQuat::IDENTITY);
        assert_eq!(unmate.label(&doc), "Unmate TR50/M");
        round_trip(&doc, unmate);
        let mate = Command::mate(ids[2], ids[1], "bottom", "bore", -5.0, 0.0, DVec3::ZERO, DQuat::IDENTITY);
        assert_eq!(mate.label(&doc), "Mate TR50/M");
        round_trip(&doc, mate);
        round_trip(&doc, Command::SetLocked { id: ids[0], locked: true });
        round_trip(&doc, Command::SetNote { id: ids[0], note: "aligned 2026-10".into() });
        round_trip(&doc, Command::SetSelectLock { target: SelectionTarget::Node(ids[0]), on: true });
        let mut params = crate::part::Params::new();
        params.insert("length".into(), crate::part::ParamValue::Number(75.0));
        round_trip(&doc, Command::SetParams { id: ids[2], params });
        round_trip(&doc, Command::RemoveNodes(vec![ids[0], ids[2]]));

        // A node cannot be attached below itself, nor to a missing parent.
        let cycle = Command::mate(ids[0], ids[3], "seat", "top", 0.0, 0.0, DVec3::ZERO, DQuat::IDENTITY);
        assert_rejected(&doc, cycle, |e| *e == CommandError::ParentCycle(ids[0].0));
        let orphan = Command::mate(ids[0], NodeId(9999), "seat", "top", 0.0, 0.0, DVec3::ZERO, DQuat::IDENTITY);
        assert_rejected(&doc, orphan, |e| *e == CommandError::UnknownParent(ids[0]));
        let mut bad = stack(&doc);
        bad[1].parent = Parent::Node(NodeId(9999));
        assert_rejected(&doc, Command::PlaceAssembly { nodes: bad }, |e| matches!(e, CommandError::UnknownParent(_)));
    }

    #[test]
    fn stacked_platforms() {
        let mut doc = demo_document();
        let table = doc.platforms[0].clone();
        let mut bb = breadboard(&doc, None);
        bb.position = DVec2::new(100.0, 0.0);
        bb.rotation_z = 0.0;
        bb.support = Support::OnPlatform { parent: table.id, standoff: 100.0 };
        let bb_id = bb.id;
        Command::AddPlatform { platform: Box::new(bb.clone()), index: None }.apply(&mut doc).unwrap();
        let (origin, rotation) = doc.platform_pose(bb_id).unwrap();
        let expected = table.position.extend(table.top_height) + DVec3::new(100.0, 0.0, 100.0 + bb.thickness);
        assert!(origin.distance(expected) < 1e-9, "{origin}");
        assert_eq!(rotation, table.rotation_z);
        let (lo, hi) = doc.platform_room_aabb(bb_id).unwrap();
        assert!((hi.z - origin.z).abs() < 1e-9 && (lo.z - (origin.z - bb.thickness)).abs() < 1e-9);
        let centre = (lo.truncate() + hi.truncate()) / 2.0;
        let bb_centre = (bb.outline.bounds().0 + bb.outline.bounds().1) / 2.0;
        assert!(centre.distance(origin.truncate() + DVec2::from_angle(rotation).rotate(bb_centre)) < 1e-6);
        assert_rejected(&doc, Command::RemovePlatform(table.id), |e| *e == CommandError::PlatformHasChildren(table.id));
        let mut looped = table.clone();
        looped.support = Support::OnPlatform { parent: bb_id, standoff: 10.0 };
        assert_rejected(&doc, Command::UpdatePlatform { platform: Box::new(looped) }, |e| {
            *e == CommandError::ParentCycle(table.id.0)
        });
        assert_eq!(doc.validate(), Ok(()));
    }
}
