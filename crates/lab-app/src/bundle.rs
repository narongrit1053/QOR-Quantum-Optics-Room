//! Library bundles (saved assemblies) placed into the document: shared by the app's
//! drag-and-drop and live control's `place_bundle`.

use std::collections::HashMap;

use lab_core::glam::{DQuat, DVec3};
use lab_core::{Attach, Command, Group, GroupId, Node, NodeId, Parent};

/// POLA-203: bundle nodes placed where the anchor was dropped (`parent`, `attach`, `position`):
/// fresh ids from `next`, parents remapped, roots re-parented to `parent`; hole roots shift by the
/// drop hole, all roots by the drop point. Nodes `keep` rejects are skipped with their descendants.
/// Returns the nodes and the number skipped.
pub fn place_bundle(
    nodes: &[Node],
    mut next: u64,
    parent: Parent,
    attach: &Attach,
    position: DVec3,
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

/// Turns the placed bundle's roots (nodes not under another placed node) by `angle_deg` about
/// the vertical through `pivot`; children follow their roots.
pub fn turn_roots(nodes: &mut [Node], pivot: DVec3, angle_deg: f64) {
    let turn = DQuat::from_rotation_z(angle_deg.to_radians());
    let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
    for n in nodes.iter_mut().filter(|n| !matches!(n.parent, Parent::Node(p) if ids.contains(&p))) {
        let offset = n.position - pivot;
        n.position = pivot + turn * offset;
        n.rotation = turn * n.rotation;
        // shortcut: a root on a hole other than the anchor's is freed rather than re-snapped to a
        // turned hole index (exact position kept); upgrade if bundles with several hole roots matter.
        if matches!(n.attach, Attach::Hole { .. }) && offset.truncate().length() > 1e-6 {
            n.attach = Attach::Free;
        }
    }
}

/// The command for placing a bundle: `PlaceAssembly`, wrapped in a `Batch` with `AddGroup`
/// when the bundle carries a group name and at least two document chain roots were placed.
pub fn bundle_place_command(bundle: &lab_file::Bundle, nodes: Vec<Node>, next: u64) -> Command {
    if let Some(group_name) = &bundle.group {
        // Placed roots on a platform or the room are document chain roots; dropping onto a part
        // parents them under that part, where they cannot form a group.
        let roots: Vec<NodeId> = nodes.iter().filter(|n| !matches!(n.parent, Parent::Node(_))).map(|n| n.id).collect();
        if !roots.is_empty() {
            let group = Group { id: GroupId(next + nodes.len() as u64), name: group_name.clone(), members: roots };
            return Command::Batch(vec![Command::PlaceAssembly { nodes }, Command::AddGroup { group, index: None }]);
        }
    }
    Command::PlaceAssembly { nodes }
}
