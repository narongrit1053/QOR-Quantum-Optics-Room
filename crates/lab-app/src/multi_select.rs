//! Pure multi-selection transitions: plain click, Ctrl+click toggle, Shift+click range,
//! and whole-group matching.

use lab_core::{Command, CommandError, Document, Group, GroupId, NodeId, PlatformId, SelectionTarget};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub primary: Option<SelectionTarget>,
    pub extra: Vec<NodeId>,
}

impl Sel {
    pub fn from_nodes(nodes: &[NodeId]) -> Self {
        let Some((&first, rest)) = nodes.split_first() else {
            return Self::default();
        };
        Self { primary: Some(SelectionTarget::Node(first)), extra: rest.to_vec() }
    }
}

pub fn chain_roots(doc: &Document, selected: &[NodeId]) -> Vec<NodeId> {
    let mut roots = Vec::new();
    for &id in selected {
        let root = doc.chain_root(id);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

pub fn next_group_name(doc: &Document) -> String {
    let mut n = 1usize;
    while doc.groups.iter().any(|g| g.name == format!("Group {n}")) {
        n += 1;
    }
    format!("Group {n}")
}

/// Right-click keeps an active multi-selection when hitting an already-selected node,
/// and otherwise selects the clicked target (clearing stale extras).
pub fn right_click(doc: &Document, prev: &Sel, hit: Option<SelectionTarget>) -> Sel {
    if let Some(SelectionTarget::Node(id)) = hit
        && (prev.primary == Some(SelectionTarget::Node(id)) || prev.extra.contains(&id))
    {
        return prev.clone();
    }
    click(doc, prev, hit)
}

pub fn make_group_command(doc: &Document, selected: &[NodeId]) -> (Command, Group) {
    let members = chain_roots(doc, selected);
    let group = Group {
        id: GroupId(doc.next_free_id()),
        name: next_group_name(doc),
        members,
    };
    (Command::AddGroup { group: group.clone(), index: None }, group)
}

pub fn is_whole_group(doc: &Document, sel: &Sel) -> Option<GroupId> {
    let SelectionTarget::Node(primary) = sel.primary? else {
        return None;
    };
    let group = doc.group_of(primary)?;
    let all_in_group = sel.extra.iter().all(|id| group.members.contains(id));
    let all_members_selected =
        group.members.iter().all(|m| *m == primary || sel.extra.contains(m));
    (all_in_group && all_members_selected).then_some(group.id)
}

pub fn click(doc: &Document, prev: &Sel, hit: Option<SelectionTarget>) -> Sel {
    let Some(target) = hit else {
        return Sel::default();
    };
    let SelectionTarget::Node(id) = target else {
        return Sel { primary: Some(target), extra: Vec::new() };
    };
    let root = doc.chain_root(id);
    if let Some(group) = doc.group_of(root) {
        if is_whole_group(doc, prev) == Some(group.id) {
            return Sel { primary: Some(SelectionTarget::Node(id)), extra: Vec::new() };
        }
        let extra = group.members.iter().copied().filter(|&m| m != root).collect();
        return Sel { primary: Some(SelectionTarget::Node(root)), extra };
    }
    Sel { primary: Some(SelectionTarget::Node(id)), extra: Vec::new() }
}

pub fn toggle(prev: &Sel, node: NodeId) -> Sel {
    let Some(SelectionTarget::Node(primary)) = prev.primary else {
        return Sel { primary: Some(SelectionTarget::Node(node)), extra: Vec::new() };
    };
    if primary == node {
        let mut extra = prev.extra.clone();
        let next_primary = extra.pop().map(SelectionTarget::Node);
        return Sel { primary: next_primary, extra };
    }
    let mut extra = prev.extra.clone();
    if let Some(pos) = extra.iter().position(|&id| id == node) {
        extra.remove(pos);
    } else {
        extra.push(node);
    }
    Sel { primary: Some(SelectionTarget::Node(primary)), extra }
}

pub fn range(rows: &[(i32, i32)], prev: &Sel, node: NodeId) -> Sel {
    let Some(SelectionTarget::Node(primary)) = prev.primary else {
        return Sel { primary: Some(SelectionTarget::Node(node)), extra: Vec::new() };
    };
    let row_pos = |target: NodeId| {
        rows.iter().position(|&(kind, id)| kind == 0 && u64::try_from(id).ok() == Some(target.0))
    };
    let (Some(a), Some(b)) = (row_pos(primary), row_pos(node)) else {
        let mut extra = prev.extra.clone();
        if node != primary && !extra.contains(&node) {
            extra.push(node);
        }
        return Sel { primary: Some(SelectionTarget::Node(primary)), extra };
    };
    let (lo, hi) = (a.min(b), a.max(b));
    let mut extra = Vec::new();
    for &(kind, id) in &rows[lo..=hi] {
        if kind == 0
            && let Ok(raw) = u64::try_from(id)
        {
            let nid = NodeId(raw);
            if nid != primary && !extra.contains(&nid) {
                extra.push(nid);
            }
        }
    }
    Sel { primary: Some(SelectionTarget::Node(primary)), extra }
}

/// Plain-sentence toast text for a refused `AddGroup` command.
pub fn group_error_message(error: &CommandError) -> String {
    match error {
        CommandError::GroupTooSmall => "Select at least two parts to group".into(),
        CommandError::AlreadyGrouped(_) => "A selected part is already in a group".into(),
        CommandError::GroupCrossPlatform => "Parts on different platforms can't be grouped".into(),
        CommandError::GroupMemberNotRoot(_) => "Only top-level parts or assemblies can be grouped".into(),
        other => other.to_string(),
    }
}

/// Scene-tree row click (and relation-chip selection via kind 0): plain click on a node row
/// selects just that node; clicking a group folder row (kind 3) selects the whole group.
pub fn scene_row_click(
    doc: &Document,
    prev: &Sel,
    kind: i32,
    id: i32,
    ctrl: bool,
    shift: bool,
    visible_rows: impl FnOnce() -> Vec<(i32, i32)>,
) -> Option<Sel> {
    match kind {
        3 if id >= 0 => doc.group(GroupId(id as u64)).map(|g| Sel::from_nodes(&g.members)),
        0 if id >= 0 => {
            let nid = NodeId(id as u64);
            Some(if ctrl {
                toggle(prev, nid)
            } else if shift {
                range(&visible_rows(), prev, nid)
            } else if prev.primary == Some(SelectionTarget::Node(nid)) && prev.extra.is_empty() {
                Sel::default()
            } else {
                Sel { primary: Some(SelectionTarget::Node(nid)), extra: Vec::new() }
            })
        }
        1 if id >= 0 => {
            let target = SelectionTarget::Platform(PlatformId(id as u64));
            let hit = (prev.primary != Some(target) || !prev.extra.is_empty()).then_some(target);
            Some(Sel { primary: hit, extra: Vec::new() })
        }
        2 => {
            let target = SelectionTarget::Room;
            let hit = (prev.primary != Some(target) || !prev.extra.is_empty()).then_some(target);
            Some(Sel { primary: hit, extra: Vec::new() })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::glam::{DQuat, DVec3};
    use lab_core::part::Params;
    use lab_core::{Attach, Finish, Group, Node, Parent, PlatformId, Shape};

    fn make_node(id: u64, parent: Parent) -> Node {
        Node {
            id: NodeId(id),
            name: format!("N{id}"),
            part: "generic:box".into(),
            parent,
            attach: Attach::Free,
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            shape: Shape::Part,
            finish: Finish::Stainless,
            params: Params::new(),
            pickable: true,
            locked: false,
            select_lock: false,
            note: String::new(),
        }
    }

    fn doc_with_group() -> Document {
        let mut doc = Document::new("Test");
        let plat = Parent::Platform(PlatformId(1));
        doc.nodes.push(make_node(1, plat));
        doc.nodes.push(make_node(2, plat));
        doc.nodes.push(make_node(3, plat));
        doc.nodes.push(make_node(4, Parent::Node(NodeId(1))));
        doc.nodes.push(make_node(5, plat));
        doc.groups.push(Group {
            id: GroupId(10),
            name: "Group 1".into(),
            members: vec![NodeId(1), NodeId(2), NodeId(3)],
        });
        doc
    }

    #[test]
    fn toggle_adds_and_removes() {
        let empty = Sel::default();
        let s = toggle(&empty, NodeId(1));
        assert_eq!(s, Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![] });

        let s = toggle(&s, NodeId(2));
        assert_eq!(s, Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2)] });

        let s = toggle(&s, NodeId(3));
        assert_eq!(
            s,
            Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2), NodeId(3)] }
        );

        let s = toggle(&s, NodeId(2));
        assert_eq!(s, Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(3)] });

        // Platform or Room primary is dropped when toggling a node in.
        let plat = Sel { primary: Some(SelectionTarget::Platform(PlatformId(1))), extra: vec![] };
        assert_eq!(
            toggle(&plat, NodeId(5)),
            Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![] }
        );
        let room = Sel { primary: Some(SelectionTarget::Room), extra: vec![] };
        assert_eq!(
            toggle(&room, NodeId(5)),
            Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![] }
        );
    }

    #[test]
    fn toggle_off_primary_promotes_last_extra() {
        let sel = Sel {
            primary: Some(SelectionTarget::Node(NodeId(1))),
            extra: vec![NodeId(2), NodeId(3)],
        };
        let next = toggle(&sel, NodeId(1));
        assert_eq!(
            next,
            Sel { primary: Some(SelectionTarget::Node(NodeId(3))), extra: vec![NodeId(2)] }
        );

        let next = toggle(&next, NodeId(3));
        assert_eq!(next, Sel { primary: Some(SelectionTarget::Node(NodeId(2))), extra: vec![] });

        let next = toggle(&next, NodeId(2));
        assert_eq!(next, Sel { primary: None, extra: vec![] });
    }

    #[test]
    fn click_group_member_selects_whole_group() {
        let doc = doc_with_group();
        let empty = Sel::default();

        // Clicking a direct member selects the whole group with that member as primary.
        let sel = click(&doc, &empty, Some(SelectionTarget::Node(NodeId(2))));
        assert_eq!(
            sel,
            Sel { primary: Some(SelectionTarget::Node(NodeId(2))), extra: vec![NodeId(1), NodeId(3)] }
        );
        assert_eq!(is_whole_group(&doc, &sel), Some(GroupId(10)));

        // Clicking a child of a grouped chain root selects the whole group with its chain root as primary.
        let from_child = click(&doc, &empty, Some(SelectionTarget::Node(NodeId(4))));
        assert_eq!(
            from_child,
            Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2), NodeId(3)] }
        );
        assert_eq!(is_whole_group(&doc, &from_child), Some(GroupId(10)));
    }

    #[test]
    fn click_again_selects_member_alone() {
        let doc = doc_with_group();
        let whole = click(&doc, &Sel::default(), Some(SelectionTarget::Node(NodeId(2))));
        assert_eq!(is_whole_group(&doc, &whole), Some(GroupId(10)));

        // Clicking a member while the whole group is selected narrows to that clicked node alone.
        let alone = click(&doc, &whole, Some(SelectionTarget::Node(NodeId(2))));
        assert_eq!(alone, Sel { primary: Some(SelectionTarget::Node(NodeId(2))), extra: vec![] });
        assert_eq!(is_whole_group(&doc, &alone), None);

        // Clicking a child while the whole group is selected narrows to that child alone.
        let child_alone = click(&doc, &whole, Some(SelectionTarget::Node(NodeId(4))));
        assert_eq!(child_alone, Sel { primary: Some(SelectionTarget::Node(NodeId(4))), extra: vec![] });
        assert_eq!(is_whole_group(&doc, &child_alone), None);
    }

    #[test]
    fn click_none_platform_room_and_ungrouped() {
        let doc = doc_with_group();
        let whole = click(&doc, &Sel::default(), Some(SelectionTarget::Node(NodeId(1))));

        assert_eq!(click(&doc, &whole, None), Sel::default());
        assert_eq!(
            click(&doc, &whole, Some(SelectionTarget::Platform(PlatformId(1)))),
            Sel { primary: Some(SelectionTarget::Platform(PlatformId(1))), extra: vec![] }
        );
        assert_eq!(
            click(&doc, &whole, Some(SelectionTarget::Room)),
            Sel { primary: Some(SelectionTarget::Room), extra: vec![] }
        );
        assert_eq!(
            click(&doc, &whole, Some(SelectionTarget::Node(NodeId(5)))),
            Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![] }
        );
    }

    #[test]
    fn is_whole_group_requires_exact_member_set() {
        let doc = doc_with_group();
        let subset = Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2)] };
        assert_eq!(is_whole_group(&doc, &subset), None);

        let superset = Sel {
            primary: Some(SelectionTarget::Node(NodeId(1))),
            extra: vec![NodeId(2), NodeId(3), NodeId(5)],
        };
        assert_eq!(is_whole_group(&doc, &superset), None);

        let reordered = Sel {
            primary: Some(SelectionTarget::Node(NodeId(3))),
            extra: vec![NodeId(1), NodeId(2)],
        };
        assert_eq!(is_whole_group(&doc, &reordered), Some(GroupId(10)));
    }

    #[test]
    fn range_selects_visible_nodes_between() {
        let rows = [(2, 0), (1, 1), (0, 5), (0, 6), (0, 7)];
        let prev = Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![] };
        let sel = range(&rows, &prev, NodeId(7));
        assert_eq!(
            sel,
            Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![NodeId(6), NodeId(7)] }
        );

        // Reverse range keeps primary unchanged and skips non-node rows between them.
        let rows_mixed = [(0, 5), (1, 1), (3, 10), (0, 6), (0, 7)];
        let prev_end = Sel { primary: Some(SelectionTarget::Node(NodeId(7))), extra: vec![] };
        let sel = range(&rows_mixed, &prev_end, NodeId(5));
        assert_eq!(
            sel,
            Sel { primary: Some(SelectionTarget::Node(NodeId(7))), extra: vec![NodeId(5), NodeId(6)] }
        );

        // No primary (or non-node primary) selects the clicked node as primary.
        let sel = range(&rows, &Sel::default(), NodeId(6));
        assert_eq!(sel, Sel { primary: Some(SelectionTarget::Node(NodeId(6))), extra: vec![] });
        let plat = Sel { primary: Some(SelectionTarget::Platform(PlatformId(1))), extra: vec![] };
        assert_eq!(
            range(&rows, &plat, NodeId(6)),
            Sel { primary: Some(SelectionTarget::Node(NodeId(6))), extra: vec![] }
        );
    }

    #[test]
    fn range_fallback_keeps_primary_and_extras_when_row_hidden() {
        let rows = [(2, 0), (1, 1), (0, 3), (0, 5)];
        // Primary is hidden inside a collapsed folder; keep primary and existing extras, add clicked node.
        let prev = Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2)] };
        assert_eq!(
            range(&rows, &prev, NodeId(3)),
            Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2), NodeId(3)] }
        );
        // Clicking an already-selected extra or primary does not duplicate it.
        let prev2 = Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(3)] };
        assert_eq!(range(&rows, &prev2, NodeId(3)), prev2);
        assert_eq!(range(&rows, &prev2, NodeId(1)), prev2);

        // Clicked row not in visible rows keeps primary and extras and adds the clicked node.
        let prev_vis = Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![NodeId(3)] };
        assert_eq!(
            range(&rows, &prev_vis, NodeId(9)),
            Sel { primary: Some(SelectionTarget::Node(NodeId(5))), extra: vec![NodeId(3), NodeId(9)] }
        );
    }

    #[test]
    fn group_action_names_group_n() {
        let mut doc = doc_with_group(); // already contains "Group 1"
        doc.nodes.push(make_node(6, Parent::Platform(PlatformId(1))));
        let (cmd, group) = make_group_command(&doc, &[NodeId(5), NodeId(6)]);
        assert_eq!(group.name, "Group 2");
        assert!(matches!(cmd, Command::AddGroup { ref group, index: None } if group.name == "Group 2"));

        // Lowest unused n >= 1 wins even when a higher number exists.
        doc.groups.push(Group {
            id: GroupId(20),
            name: "Group 3".into(),
            members: vec![NodeId(5), NodeId(6)],
        });
        assert_eq!(next_group_name(&doc), "Group 2");
    }

    #[test]
    fn right_click_keeps_selected_and_clears_outside() {
        let doc = doc_with_group();
        let multi = Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(5)] };
        // Right-clicking primary or an extra keeps the selection unchanged.
        assert_eq!(right_click(&doc, &multi, Some(SelectionTarget::Node(NodeId(1)))), multi);
        assert_eq!(right_click(&doc, &multi, Some(SelectionTarget::Node(NodeId(5)))), multi);
        // Right-clicking an ungrouped node outside the selection clears stale extras.
        let mut doc2 = doc.clone();
        doc2.nodes.push(make_node(6, Parent::Platform(PlatformId(1))));
        assert_eq!(
            right_click(&doc2, &multi, Some(SelectionTarget::Node(NodeId(6)))),
            Sel { primary: Some(SelectionTarget::Node(NodeId(6))), extra: vec![] }
        );
        // Right-clicking platform/empty clears extras.
        assert_eq!(right_click(&doc, &multi, None), Sel::default());
        assert_eq!(
            right_click(&doc, &multi, Some(SelectionTarget::Platform(PlatformId(1)))),
            Sel { primary: Some(SelectionTarget::Platform(PlatformId(1))), extra: vec![] }
        );
    }

    #[test]
    fn scene_row_plain_click_selects_single_node_not_group() {
        let doc = doc_with_group();
        let empty = Sel::default();

        // Plain click on a grouped member's own tree row (kind 0) selects just that part.
        assert_eq!(
            scene_row_click(&doc, &empty, 0, 2, false, false, Vec::new),
            Some(Sel { primary: Some(SelectionTarget::Node(NodeId(2))), extra: vec![] })
        );
        // Plain click on a child of a grouped member (e.g. from tree or relation chip) selects just that child.
        assert_eq!(
            scene_row_click(&doc, &empty, 0, 4, false, false, Vec::new),
            Some(Sel { primary: Some(SelectionTarget::Node(NodeId(4))), extra: vec![] })
        );
        // Clicking the folder row (kind 3) selects the whole group.
        let group_sel = scene_row_click(&doc, &empty, 3, 10, false, false, Vec::new).unwrap();
        assert_eq!(group_sel, Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![NodeId(2), NodeId(3)] });
        // Clicking a member's row while the whole group is selected narrows to just that member.
        assert_eq!(
            scene_row_click(&doc, &group_sel, 0, 1, false, false, Vec::new),
            Some(Sel { primary: Some(SelectionTarget::Node(NodeId(1))), extra: vec![] })
        );
        // Clicking the sole selected row again clears the selection.
        let sole = Sel { primary: Some(SelectionTarget::Node(NodeId(2))), extra: vec![] };
        assert_eq!(scene_row_click(&doc, &sole, 0, 2, false, false, Vec::new), Some(Sel::default()));
    }

    #[test]
    fn group_error_messages_are_plain_sentences() {
        let cases = [
            (CommandError::GroupTooSmall, "Select at least two parts to group"),
            (CommandError::AlreadyGrouped(NodeId(12)), "A selected part is already in a group"),
            (CommandError::GroupCrossPlatform, "Parts on different platforms can't be grouped"),
            (CommandError::GroupMemberNotRoot(NodeId(7)), "Only top-level parts or assemblies can be grouped"),
        ];
        for (err, expected) in cases {
            let msg = group_error_message(&err);
            assert_eq!(msg, expected);
            assert!(!msg.contains("NodeId"), "debug text leaked: {msg}");
        }
    }
}
