//! Pure multi-selection transitions: plain click, Ctrl+click toggle, Shift+click range,
//! and whole-group matching.

use lab_core::{Document, GroupId, NodeId, SelectionTarget};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub primary: Option<SelectionTarget>,
    pub extra: Vec<NodeId>,
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
        return Sel { primary: Some(SelectionTarget::Node(node)), extra: Vec::new() };
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
}
