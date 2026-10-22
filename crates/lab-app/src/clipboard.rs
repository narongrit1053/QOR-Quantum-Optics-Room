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
}
