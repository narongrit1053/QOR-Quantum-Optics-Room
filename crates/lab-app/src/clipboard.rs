//! M7 A: copy, cut, paste and duplicate of whole sets (docs/M7-PLAN.md).

use std::collections::HashMap;

use lab_core::{Attach, Command, Document, Node, NodeId, Parent, compute_holes};

/// Fallback paste offset when the root's parent has no hole grid.
const FREE_STEP: f64 = 25.0;

/// The selected set roots and all their descendants, in document order (parents first).
pub fn copy(doc: &Document, selection: &[NodeId]) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for &id in selection {
        let root = lab_app::set_root(doc, id);
        if out.iter().any(|n| n.id == root) {
            continue;
        }
        for nid in std::iter::once(root).chain(doc.descendants(root)) {
            if let Some(n) = doc.node(nid) {
                out.push(n.clone());
            }
        }
    }
    out
}

/// Fresh ids, parents remapped inside the list, roots moved one hole pitch in +X (`Hole { i + 1 }`,
/// `Free` roots +pitch, 25 mm without a grid). Returns the command and the new root ids.
pub fn paste(doc: &Document, nodes: &[Node]) -> Option<(Command, Vec<NodeId>)> {
    let mut next = doc.next_free_id();
    let mut map: HashMap<NodeId, NodeId> = HashMap::new();
    let mut out = Vec::new();
    let mut roots = Vec::new();
    for n in nodes {
        let mut node = n.clone();
        match n.parent {
            // Inside the copied set: follow the remapped parent (dropped if its parent was).
            Parent::Node(p) if nodes.iter().any(|m| m.id == p) => match map.get(&p) {
                Some(&np) => node.parent = Parent::Node(np),
                None => continue,
            },
            Parent::Node(p) if doc.node(p).is_none() => continue,
            Parent::Platform(p) => {
                let platform = doc.platform(p)?;
                let pitch = compute_holes(platform).ok().map(|h| h.pitch.x).filter(|x| *x > 0.0).unwrap_or(FREE_STEP);
                if let Attach::Hole { hole } = &mut node.attach {
                    hole.i += 1;
                }
                node.position.x += pitch;
                roots.push(NodeId(next));
            }
            _ => {
                node.position.x += FREE_STEP;
                roots.push(NodeId(next));
            }
        }
        node.id = NodeId(next);
        map.insert(n.id, node.id);
        next += 1;
        out.push(node);
    }
    (!out.is_empty()).then(|| (Command::PlaceAssembly { nodes: out }, roots))
}

/// POLA-203: `copy` output made position-free: the first root becomes the anchor at hole (0,0)
/// (or the origin when it is not on a hole); other roots keep their offset from it.
pub fn bundle_nodes(mut nodes: Vec<Node>) -> Vec<Node> {
    let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
    let is_root = |n: &Node| !matches!(n.parent, Parent::Node(p) if ids.contains(&p));
    let Some(anchor) = nodes.first().cloned() else { return nodes };
    let anchor_hole = match anchor.attach {
        Attach::Hole { hole } => Some(hole),
        _ => None,
    };
    for n in nodes.iter_mut().filter(|n| is_root(n)) {
        n.position -= anchor.position;
        match (&mut n.attach, anchor_hole) {
            (Attach::Hole { hole }, Some(a)) => {
                hole.i -= a.i;
                hole.j -= a.j;
            }
            (attach @ Attach::Hole { .. }, None) => *attach = Attach::Free,
            _ => {}
        }
    }
    nodes
}

/// POLA-203: bundle nodes placed where the anchor was dropped (`parent`, `attach`, `position`):
/// fresh ids from `next`, parents remapped, roots re-parented to `parent`; hole roots shift by the
/// drop hole, all roots by the drop point. Nodes `keep` rejects are skipped with their descendants.
/// Returns the nodes and the number skipped.
pub fn place_bundle(
    nodes: &[Node],
    mut next: u64,
    parent: Parent,
    attach: &Attach,
    position: lab_core::glam::DVec3,
    keep: impl Fn(&Node) -> bool,
) -> (Vec<Node>, usize) {
    let mut map: HashMap<NodeId, NodeId> = HashMap::new();
    let (mut out, mut skipped) = (Vec::new(), 0);
    for (k, n) in nodes.iter().enumerate() {
        let mut node = n.clone();
        let inner = matches!(n.parent, Parent::Node(p) if nodes.iter().any(|m| m.id == p));
        if inner {
            let Parent::Node(p) = n.parent else { unreachable!() };
            match map.get(&p) {
                Some(&np) => node.parent = Parent::Node(np),
                None => {
                    skipped += 1;
                    continue;
                }
            }
        } else {
            node.parent = parent;
            node.position += position;
            match (&mut node.attach, attach) {
                (Attach::Hole { hole }, Attach::Hole { hole: at }) => {
                    hole.i += at.i;
                    hole.j += at.j;
                }
                // The anchor dropped onto a part: it takes the drop's own mate.
                (a, at @ (Attach::Mate { .. } | Attach::Constrained { .. } | Attach::Joint(_))) if k == 0 => *a = at.clone(),
                (a @ Attach::Hole { .. }, _) => *a = Attach::Free,
                _ => {}
            }
        }
        if !keep(n) {
            skipped += 1;
            continue;
        }
        node.id = NodeId(next);
        map.insert(n.id, node.id);
        next += 1;
        out.push(node);
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_remaps_a_hole_set() {
        let mut doc = lab_core::sample::demo_document();
        let mut root = doc.nodes.iter().find(|n| matches!(n.parent, Parent::Platform(_))).expect("demo has a part on a platform").clone();
        root.attach = Attach::Hole { hole: lab_core::LatticeIndex { i: 3, j: 4 } };
        root.id = NodeId(doc.allocate_id());
        doc.nodes.push(root.clone());
        // A 5-node mated chain under it.
        let mut parent = root.id;
        for _ in 0..4 {
            let mut child = root.clone();
            child.id = NodeId(doc.allocate_id());
            child.parent = Parent::Node(parent);
            child.attach = Attach::Mate { self_conn: "a".into(), parent_conn: "b".into(), slide: 0.0, twist: 0.0 };
            parent = child.id;
            doc.nodes.push(child);
        }
        let root = &root;
        let Attach::Hole { hole } = root.attach.clone() else { unreachable!() };
        let copied = copy(&doc, &[*doc.descendants(root.id).last().unwrap()]);
        assert_eq!(copied.len(), 1 + doc.descendants(root.id).len());
        let (Command::PlaceAssembly { nodes }, roots) = paste(&doc, &copied).unwrap() else { panic!() };
        assert_eq!(nodes.len(), copied.len());
        assert!(nodes.iter().all(|n| doc.node(n.id).is_none()));
        assert_eq!(roots, vec![nodes[0].id]);
        assert_eq!(nodes[0].attach, Attach::Hole { hole: lab_core::LatticeIndex { i: hole.i + 1, j: hole.j } });
        for n in &nodes[1..] {
            let Parent::Node(p) = n.parent else { panic!() };
            assert!(nodes.iter().any(|m| m.id == p));
        }
    }

    #[test]
    fn bundle_saved_at_3_4_lands_at_10_2() {
        use lab_core::LatticeIndex;
        let mut doc = lab_core::sample::demo_document();
        let mut root = doc.nodes.iter().find(|n| matches!(n.parent, Parent::Platform(_))).expect("demo has a part on a platform").clone();
        root.attach = Attach::Hole { hole: LatticeIndex { i: 3, j: 4 } };
        root.id = NodeId(doc.allocate_id());
        doc.nodes.push(root.clone());
        let mut parent = root.id;
        for _ in 0..2 {
            let mut child = root.clone();
            child.id = NodeId(doc.allocate_id());
            child.parent = Parent::Node(parent);
            child.attach = Attach::Mate { self_conn: "a".into(), parent_conn: "b".into(), slide: 0.0, twist: 0.0 };
            parent = child.id;
            doc.nodes.push(child);
        }
        let saved = bundle_nodes(copy(&doc, &[parent]));
        assert_eq!(saved.len(), 3);
        assert_eq!(saved[0].attach, Attach::Hole { hole: LatticeIndex { i: 0, j: 0 } });
        let target = doc.platforms.last().unwrap().id;
        let drop = Attach::Hole { hole: LatticeIndex { i: 10, j: 2 } };
        let at = lab_core::glam::DVec3::new(250.0, 50.0, 0.0);
        let next = doc.next_free_id();
        let (nodes, skipped) = place_bundle(&saved, next, Parent::Platform(target), &drop, at, |_| true);
        assert_eq!((nodes.len(), skipped), (3, 0));
        assert!(nodes.iter().all(|n| doc.node(n.id).is_none()));
        assert_eq!(nodes[0].parent, Parent::Platform(target));
        assert_eq!(nodes[0].attach, drop);
        assert_eq!(nodes[0].position, at);
        assert_eq!(nodes[1].parent, Parent::Node(nodes[0].id));
        assert_eq!(nodes[2].parent, Parent::Node(nodes[1].id));
        // A missing middle part drops it and its child.
        let (nodes, skipped) = place_bundle(&saved, next, Parent::Platform(target), &drop, at, |n| n.id != saved[1].id);
        assert_eq!((nodes.len(), skipped), (1, 2));
    }
}
