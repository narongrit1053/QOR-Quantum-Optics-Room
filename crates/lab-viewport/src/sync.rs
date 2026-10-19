//! Incremental scene sync, planned purely from documents and change sets (M1-SPEC §5.3).
//!
//! The viewport keeps the snapshot it last rendered. A new snapshot plus the [`ChangeSet`] that
//! produced it becomes a short list of [`SyncOp`]s, which `scene::apply_scene_requests` carries
//! out on the Bevy world. The plan is tested against a full rebuild with a fake world.

use lab_core::{ChangeSet, Document, NodeId, PlatformId};

/// One scene operation. The derived order is the execution order: all despawns, then room and
/// platform builds, then placements, then nodes (which need their platform's root).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SyncOp {
    DespawnNode(NodeId),
    /// Root + parts + node children.
    DespawnPlatform(PlatformId),
    DespawnRoom,
    SpawnRoom,
    /// Create the root if missing, (re)build its parts, set the root transform.
    BuildPlatform(PlatformId),
    /// Root transform only.
    PlacePlatform(PlatformId),
    SpawnNode(NodeId),
}

/// SpawnRoom, BuildPlatform(each), SpawnNode(each whose platform exists).
pub(crate) fn plan_full(doc: &Document) -> Vec<SyncOp> {
    let mut ops = vec![SyncOp::SpawnRoom];
    ops.extend(doc.platforms.iter().map(|p| SyncOp::BuildPlatform(p.id)));
    ops.extend(doc.nodes.iter().filter(|n| doc.platform(n.platform).is_some()).map(|n| SyncOp::SpawnNode(n.id)));
    ops
}

/// The operations that turn the scene built from `old` into the one `plan_full(new)` builds,
/// given the `changes` that led from `old` to `new`. Sorted (see [`SyncOp`]) and deduplicated.
pub(crate) fn plan_changes(old: &Document, new: &Document, changes: &ChangeSet) -> Vec<SyncOp> {
    let mut ops = Vec::new();
    let respawn_node = |ops: &mut Vec<SyncOp>, id: NodeId| {
        ops.push(SyncOp::DespawnNode(id));
        if new.node(id).is_some_and(|n| new.platform(n.platform).is_some()) {
            ops.push(SyncOp::SpawnNode(id));
        }
    };

    for &id in changes.nodes_added.iter().chain(&changes.nodes_removed).chain(&changes.nodes_updated) {
        respawn_node(&mut ops, id);
    }

    for &id in &changes.platforms_removed {
        // The root despawns its node children with it; despawning them first keeps the index exact.
        for node in old.nodes_on(id) {
            ops.push(SyncOp::DespawnNode(node.id));
        }
        ops.push(SyncOp::DespawnPlatform(id));
        // Removed and added again in one (merged) change set: rebuild it and its nodes.
        if new.platform(id).is_some() {
            ops.push(SyncOp::BuildPlatform(id));
            for node in new.nodes_on(id) {
                respawn_node(&mut ops, node.id);
            }
        }
    }

    for &id in &changes.platforms_added {
        if new.platform(id).is_some() {
            ops.push(SyncOp::BuildPlatform(id));
        }
    }

    for &(id, aspects) in &changes.platforms_updated {
        if new.platform(id).is_none() {
            continue;
        }
        if aspects.needs_rebuild() {
            ops.push(SyncOp::BuildPlatform(id));
        } else if aspects.placement {
            ops.push(SyncOp::PlacePlatform(id));
        }
    }

    if changes.room_changed {
        ops.push(SyncOp::DespawnRoom);
        ops.push(SyncOp::SpawnRoom);
    }

    ops.sort();
    ops.dedup();
    // A rebuild sets the root transform too.
    let built: Vec<PlatformId> =
        ops.iter().filter_map(|op| if let SyncOp::BuildPlatform(id) = op { Some(*id) } else { None }).collect();
    ops.retain(|op| !matches!(op, SyncOp::PlacePlatform(id) if built.contains(id)));
    ops
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lab_core::glam::{DQuat, DVec2, DVec3};
    use lab_core::presets::{PRESETS, default_document, platform_from_preset};
    use lab_core::sample::demo_document;
    use lab_core::*;

    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }

        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.next() % 100 < percent
        }
    }

    /// What a platform's parts are built from (everything except name, kind and placement; the
    /// top height only matters through the legs).
    #[derive(Clone, Debug, PartialEq)]
    struct PlatformBuild {
        outline: Outline,
        thickness: f64,
        surface: Surface,
        hole_grid: Option<HoleGrid>,
        support: Support,
        leg_top: Option<f64>,
    }

    impl PlatformBuild {
        fn of(p: &Platform) -> Self {
            Self {
                outline: p.outline.clone(),
                thickness: p.thickness,
                surface: p.surface,
                hole_grid: p.hole_grid.clone(),
                support: p.support.clone(),
                leg_top: matches!(p.support, Support::Legs { .. }).then_some(p.top_height),
            }
        }
    }

    type Placement = (DVec2, f64, f64);

    fn placement(p: &Platform) -> Placement {
        (p.position, p.rotation_z, p.top_height)
    }

    /// The scene as the ops leave it: what each entity was built from.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct FakeWorld {
        room: Option<Room>,
        platforms: BTreeMap<PlatformId, (PlatformBuild, Placement)>,
        /// Node clone at spawn time and its parent platform.
        nodes: BTreeMap<NodeId, Node>,
    }

    impl FakeWorld {
        fn apply(&mut self, ops: &[SyncOp], doc: &Document) {
            for &op in ops {
                match op {
                    SyncOp::DespawnNode(id) => {
                        self.nodes.remove(&id);
                    }
                    SyncOp::DespawnPlatform(id) => {
                        self.platforms.remove(&id);
                        // Node children despawn with their root.
                        self.nodes.retain(|_, n| n.platform != id);
                    }
                    SyncOp::DespawnRoom => self.room = None,
                    SyncOp::SpawnRoom => {
                        assert!(self.room.is_none(), "room spawned twice");
                        self.room = Some(doc.room.clone());
                    }
                    SyncOp::BuildPlatform(id) => {
                        let p = doc.platform(id).expect("BuildPlatform of a platform that does not exist");
                        self.platforms.insert(id, (PlatformBuild::of(p), placement(p)));
                    }
                    SyncOp::PlacePlatform(id) => {
                        let p = doc.platform(id).expect("PlacePlatform of a platform that does not exist");
                        let entry = self.platforms.get_mut(&id).expect("PlacePlatform without a root");
                        entry.1 = placement(p);
                    }
                    SyncOp::SpawnNode(id) => {
                        let n = doc.node(id).expect("SpawnNode of a node that does not exist");
                        assert!(self.platforms.contains_key(&n.platform), "SpawnNode without its platform root");
                        assert!(!self.nodes.contains_key(&id), "node {id:?} spawned twice");
                        self.nodes.insert(id, n.clone());
                    }
                }
            }
        }

        fn full(doc: &Document) -> Self {
            let mut world = FakeWorld::default();
            world.apply(&plan_full(doc), doc);
            world
        }
    }

    fn assert_ordered(ops: &[SyncOp]) {
        assert!(ops.windows(2).all(|w| w[0] < w[1]), "ops not sorted and deduplicated: {ops:?}");
    }

    fn new_node(doc: &Document, platform: PlatformId, rng: &mut Rng) -> Node {
        let id = doc.next_free_id();
        Node {
            id: NodeId(id),
            name: format!("Part {id}"),
            part: "generic:post-12.7-50".into(),
            platform,
            position: DVec3::new(rng.range(-300.0, 300.0), rng.range(-200.0, 200.0), 25.0),
            rotation: DQuat::from_rotation_z(rng.range(-3.0, 3.0)),
            shape: Shape::Cylinder { radius: 6.35, length: 50.0 },
            finish: Finish::Stainless,
            pickable: true,
        }
    }

    /// A random platform edit (lab-core's P1 command set), biased toward placement-only edits.
    fn mutate(p: &Platform, rng: &mut Rng) -> Platform {
        let mut q = p.clone();
        match rng.below(13) {
            0 => q.position += DVec2::new(rng.range(-500.0, 500.0), rng.range(-500.0, 500.0)),
            1 => q.rotation_z = rng.range(-3.0, 3.0),
            2 => q.name = format!("Renamed {}", rng.below(100)),
            3 => q.outline = Outline::Rectangle { size: DVec2::new(rng.range(100.0, 2500.0), rng.range(100.0, 1500.0)) },
            4 => {
                let size = DVec2::new(rng.range(300.0, 2500.0), rng.range(300.0, 1500.0));
                q.outline = Outline::RoundedRectangle { size, radius: rng.range(0.0, 100.0) };
            }
            5 => {
                let size = DVec2::new(rng.range(300.0, 2500.0), rng.range(300.0, 1500.0));
                let notch = DVec2::new(rng.range(50.0, size.x - 50.0), rng.range(50.0, size.y - 50.0));
                let notch_corner = RectCorner::from_index(rng.below(4)).unwrap();
                q.outline = Outline::LShape { size, notch, notch_corner };
            }
            6 => {
                let mut corners = q.outline.corners();
                let k = rng.below(corners.len());
                corners[k].point += DVec2::new(rng.range(-400.0, 400.0), rng.range(-400.0, 400.0));
                q.outline = Outline::Polygon { corners };
                q.kind = PlatformKind::Custom;
            }
            7 => {
                q.hole_grid = match rng.below(4) {
                    0 => None,
                    1 => Some(HoleGrid::imperial()),
                    2 => Some(HoleGrid {
                        origin: GridOrigin::At(DVec2::new(rng.range(-50.0, 50.0), 0.0)),
                        ..HoleGrid::metric()
                    }),
                    _ => {
                        let mut grid = q.hole_grid.clone().unwrap_or_else(HoleGrid::metric);
                        grid.exclusions.push(ExclusionZone {
                            name: format!("Zone {}", grid.exclusions.len() + 1),
                            shape: ZoneShape::Circle { center: DVec2::ZERO, radius: rng.range(5.0, 200.0) },
                        });
                        Some(grid)
                    }
                }
            }
            8 => {
                q.thickness = rng.range(5.0, 400.0);
                q.top_height = rng.range(q.thickness, 1200.0);
            }
            9 => {
                q.support = if rng.chance(50) {
                    Support::Fixed
                } else {
                    Support::Legs { size: rng.range(10.0, 200.0), inset: 150.0 }
                }
            }
            10 => q.surface = if q.surface == Surface::Stainless { Surface::BlackAnodized } else { Surface::Stainless },
            // Top height alone: a placement on a Fixed platform, a rebuild on legs.
            11 => q.top_height = rng.range(q.thickness, 1500.0),
            _ => q.kind = if q.kind == PlatformKind::Breadboard { PlatformKind::OpticalTable } else { PlatformKind::Breadboard },
        }
        q
    }

    fn room_edit(doc: &Document, rng: &mut Rng) -> Option<Room> {
        let room = &doc.room;
        let n = room.outline.len();
        let result = match rng.below(6) {
            0 => room.with_corner_inserted(rng.below(n)),
            1 => room.with_corner_removed(rng.below(n)),
            2 => {
                let k = rng.below(n);
                let to = room.outline[k] + DVec2::new(rng.range(-1500.0, 1500.0), rng.range(-1500.0, 1500.0));
                room.with_corner_moved(k, to)
            }
            3 => room.with_rectangle(DVec2::new(rng.range(3000.0, 9000.0), rng.range(3000.0, 9000.0))),
            4 => room.with_l_shape(),
            _ => room.with_dimensions(rng.range(1500.0, 4000.0), rng.range(50.0, 400.0)),
        };
        result.ok().map(|(room, _)| room)
    }

    /// A random command for `doc`, or None for undo/redo or when nothing applies.
    fn random_command(doc: &Document, rng: &mut Rng) -> Option<Command> {
        let any_platform = |rng: &mut Rng| doc.platforms[rng.below(doc.platforms.len())].id;
        let any_node = |rng: &mut Rng| doc.nodes[rng.below(doc.nodes.len())].id;
        match rng.below(10) {
            0 => {
                let preset = &PRESETS[rng.below(PRESETS.len())];
                let on = (!doc.platforms.is_empty() && rng.chance(50)).then(|| any_platform(rng));
                let platform = platform_from_preset(doc, preset, PlatformId(doc.next_free_id()), on);
                Some(Command::AddPlatform { platform: Box::new(platform), index: None })
            }
            1 => (!doc.platforms.is_empty()).then(|| Command::RemovePlatform(any_platform(rng))),
            2 | 3 => (!doc.platforms.is_empty()).then(|| {
                let p = &doc.platforms[rng.below(doc.platforms.len())];
                Command::UpdatePlatform { platform: Box::new(mutate(p, rng)) }
            }),
            4 => room_edit(doc, rng).map(|room| Command::SetRoom(Box::new(room))),
            5 => {
                if !doc.nodes.is_empty() && rng.chance(40) {
                    Some(Command::RemoveNode(any_node(rng)))
                } else if !doc.platforms.is_empty() {
                    let node = new_node(doc, any_platform(rng), rng);
                    Some(Command::AddNode { node, index: Some(rng.below(doc.nodes.len() + 1)) })
                } else {
                    None
                }
            }
            6 => (!doc.nodes.is_empty()).then(|| {
                let id = any_node(rng);
                Command::SetPlacement {
                    id,
                    position: DVec3::new(rng.range(-300.0, 300.0), rng.range(-200.0, 200.0), 25.0),
                    rotation: DQuat::from_rotation_z(rng.range(-3.0, 3.0)),
                }
            }),
            7 => (!doc.nodes.is_empty()).then(|| Command::Rename { id: any_node(rng), name: format!("Name {}", rng.below(50)) }),
            _ => None,
        }
    }

    /// 64 seeds × 40 steps of lab-core commands, undo and redo: after every step the
    /// incrementally updated fake world equals a full rebuild of the new document.
    #[test]
    fn plan_changes_matches_plan_full() {
        let mut ops_seen: BTreeMap<&'static str, usize> = BTreeMap::new();
        for seed in 1..=64u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut doc = if seed % 2 == 0 { demo_document() } else { default_document() };
            let mut history = History::default();
            let mut world = FakeWorld::full(&doc);
            assert_eq!(world, FakeWorld::full(&doc));

            for step in 0..40 {
                let old = doc.clone();
                let changes = match rng.below(10) {
                    0 => history.undo(&mut doc).map(|r| r.expect("undo")),
                    1 => history.redo(&mut doc).map(|r| r.expect("redo")),
                    _ => random_command(&doc, &mut rng).and_then(|c| history.execute(&mut doc, c).ok()),
                };
                let Some(changes) = changes else { continue };
                let ops = plan_changes(&old, &doc, &changes);
                assert_ordered(&ops);
                for op in &ops {
                    let name = match op {
                        SyncOp::DespawnNode(_) => "DespawnNode",
                        SyncOp::DespawnPlatform(_) => "DespawnPlatform",
                        SyncOp::DespawnRoom => "DespawnRoom",
                        SyncOp::SpawnRoom => "SpawnRoom",
                        SyncOp::BuildPlatform(_) => "BuildPlatform",
                        SyncOp::PlacePlatform(_) => "PlacePlatform",
                        SyncOp::SpawnNode(_) => "SpawnNode",
                    };
                    *ops_seen.entry(name).or_default() += 1;
                }
                world.apply(&ops, &doc);
                assert_eq!(world, FakeWorld::full(&doc), "seed {seed} step {step}: {changes:?} gave {ops:?}");
            }
        }
        eprintln!("ops exercised: {ops_seen:?}");
        for name in ["DespawnNode", "DespawnPlatform", "DespawnRoom", "SpawnRoom", "BuildPlatform", "PlacePlatform", "SpawnNode"] {
            assert!(ops_seen.get(name).copied().unwrap_or(0) > 0, "{name} never exercised");
        }
    }

    #[test]
    fn plan_full_builds_everything() {
        let doc = demo_document();
        let ops = plan_full(&doc);
        assert_eq!(ops[0], SyncOp::SpawnRoom);
        assert_eq!(ops.iter().filter(|op| matches!(op, SyncOp::BuildPlatform(_))).count(), doc.platforms.len());
        assert_eq!(ops.iter().filter(|op| matches!(op, SyncOp::SpawnNode(_))).count(), doc.nodes.len());
    }

    #[test]
    fn placement_only_update_emits_one_place_platform() {
        let mut doc = demo_document();
        let old = doc.clone();
        let mut p = doc.platforms[0].clone();
        p.position += DVec2::new(250.0, -100.0);
        p.rotation_z = 0.3;
        let changes = Command::UpdatePlatform { platform: Box::new(p.clone()) }.apply(&mut doc).unwrap().1;
        assert_eq!(plan_changes(&old, &doc, &changes), vec![SyncOp::PlacePlatform(p.id)]);

        // Renaming and changing the kind need nothing at all.
        let old = doc.clone();
        p.name = "Main table".into();
        p.kind = PlatformKind::Breadboard;
        let changes = Command::UpdatePlatform { platform: Box::new(p.clone()) }.apply(&mut doc).unwrap().1;
        assert!(!changes.is_empty());
        assert_eq!(plan_changes(&old, &doc, &changes), Vec::new());

        // A geometry change rebuilds (and the rebuild places the root).
        let old = doc.clone();
        p.thickness = 250.0;
        p.position += DVec2::new(10.0, 0.0);
        let changes = Command::UpdatePlatform { platform: Box::new(p.clone()) }.apply(&mut doc).unwrap().1;
        assert_eq!(plan_changes(&old, &doc, &changes), vec![SyncOp::BuildPlatform(p.id)]);
    }

    #[test]
    fn removing_a_platform_despawns_its_nodes_first() {
        let mut doc = demo_document();
        let old = doc.clone();
        let id = doc.platforms[0].id;
        let changes = Command::RemovePlatform(id).apply(&mut doc).unwrap().1;
        let ops = plan_changes(&old, &doc, &changes);
        assert_eq!(ops.last(), Some(&SyncOp::DespawnPlatform(id)));
        assert_eq!(ops.len(), old.nodes_on(id).count() + 1);
        assert!(ops.iter().all(|op| matches!(op, SyncOp::DespawnNode(_) | SyncOp::DespawnPlatform(_))));
    }

    #[test]
    fn merged_remove_and_restore_rebuilds_the_nodes() {
        let mut doc = demo_document();
        let old = doc.clone();
        let id = doc.platforms[0].id;
        let (inverse, mut changes) = Command::RemovePlatform(id).apply(&mut doc).unwrap();
        let (_, restored) = inverse.apply(&mut doc).unwrap();
        changes.merge(restored);
        let mut world = FakeWorld::full(&old);
        world.apply(&plan_changes(&old, &doc, &changes), &doc);
        assert_eq!(world, FakeWorld::full(&doc));
    }
}
