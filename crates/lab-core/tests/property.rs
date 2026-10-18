//! P1: random edit sequences through `History` keep the document valid, give equal documents for
//! equal state ids, and undo back to the start.

use std::collections::BTreeMap;

use lab_core::glam::{DQuat, DVec2, DVec3};
use lab_core::presets::{PRESETS, default_document, platform_from_preset};
use lab_core::sample::demo_document;
use lab_core::*;

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

/// A random edit of `p`; some are valid, some are not.
fn mutate(p: &Platform, rng: &mut Rng) -> Platform {
    let mut q = p.clone();
    match rng.below(12) {
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
            // Custom: move one corner of the polygon (may cross edges: invalid).
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
            q.top_height = rng.range(5.0, 1200.0);
        }
        9 => {
            q.support = if rng.chance(50) {
                Support::Fixed
            } else {
                Support::Legs { size: rng.range(5.0, 200.0), inset: 150.0 }
            }
        }
        10 => q.surface = if q.surface == Surface::Stainless { Surface::BlackAnodized } else { Surface::Stainless },
        // Always invalid.
        _ => match rng.below(3) {
            0 => q.thickness = 0.0,
            1 => q.name = "   ".into(),
            _ => q.hole_grid = Some(HoleGrid { pitch: DVec2::splat(1.0), ..HoleGrid::metric() }),
        },
    }
    q
}

/// A random room edit through the shape helpers, or a new door.
fn room_edit(doc: &Document, rng: &mut Rng) -> Option<Room> {
    let room = &doc.room;
    let n = room.outline.len();
    let result = match rng.below(7) {
        0 => room.with_corner_inserted(rng.below(n)),
        1 => room.with_corner_removed(rng.below(n)),
        2 => {
            let k = rng.below(n);
            let to = room.outline[k] + DVec2::new(rng.range(-1500.0, 1500.0), rng.range(-1500.0, 1500.0));
            room.with_corner_moved(k, to)
        }
        3 => room.with_rectangle(DVec2::new(rng.range(3000.0, 9000.0), rng.range(3000.0, 9000.0))),
        4 => room.with_l_shape(),
        5 => room.with_dimensions(rng.range(1500.0, 4000.0), rng.range(50.0, 400.0)),
        _ => {
            let mut room = room.clone();
            let id = doc.next_free_id();
            room.openings.push(Opening {
                id: OpeningId(id),
                name: format!("Door {id}"),
                wall: rng.below(n) as u32,
                offset: rng.range(0.0, 4000.0),
                width: 1000.0,
                height: 2000.0,
                sill: 0.0,
            });
            let _ = room.refit_openings();
            Ok((room, Vec::new()))
        }
    };
    result.ok().map(|(room, _)| room)
}

#[test]
fn p1_random_edits() {
    let mut executed = 0;
    let mut rejected = 0;
    // Per step kind: (applied, rejected).
    let mut kinds: BTreeMap<&'static str, (u32, u32)> = BTreeMap::new();
    for seed in 1..=64u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut doc = if seed % 2 == 0 { demo_document() } else { default_document() };
        let start = doc.clone();
        let mut history = History::default();
        let mut states: BTreeMap<u64, Document> = BTreeMap::new();
        states.insert(history.state_id(), doc.clone());
        let mut next_id = doc.next_free_id();

        for step in 0..60 {
            let before = doc.clone();
            let state_before = history.state_id();
            let mut kind = "";
            let command = match rng.below(9) {
                0 | 1 => {
                    let preset = &PRESETS[rng.below(PRESETS.len())];
                    let on = (!doc.platforms.is_empty() && rng.chance(50))
                        .then(|| doc.platforms[rng.below(doc.platforms.len())].id);
                    let platform = platform_from_preset(&doc, preset, PlatformId(doc.next_free_id()), on);
                    kind = "add platform";
                    Some(Command::AddPlatform { platform: Box::new(platform), index: None })
                }
                2 => {
                    kind = "remove platform";
                    (!doc.platforms.is_empty())
                        .then(|| Command::RemovePlatform(doc.platforms[rng.below(doc.platforms.len())].id))
                }
                3 | 4 => {
                    kind = "update platform";
                    (!doc.platforms.is_empty()).then(|| {
                        let p = &doc.platforms[rng.below(doc.platforms.len())];
                        Command::UpdatePlatform { platform: Box::new(mutate(p, &mut rng)) }
                    })
                }
                5 => {
                    kind = "set room";
                    room_edit(&doc, &mut rng).map(|room| Command::SetRoom(Box::new(room)))
                }
                6 => {
                    if !doc.nodes.is_empty() && rng.chance(40) {
                        kind = "remove node";
                        Some(Command::RemoveNode(doc.nodes[rng.below(doc.nodes.len())].id))
                    } else if !doc.platforms.is_empty() {
                        kind = "add node";
                        let platform = doc.platforms[rng.below(doc.platforms.len())].id;
                        let node = new_node(&doc, platform, &mut rng);
                        Some(Command::AddNode { node, index: Some(rng.below(doc.nodes.len() + 1)) })
                    } else {
                        None
                    }
                }
                7 => {
                    if let Some(result) = history.undo(&mut doc) {
                        result.unwrap_or_else(|e| panic!("seed {seed} step {step}: undo failed: {e}"));
                        kinds.entry("undo").or_default().0 += 1;
                    }
                    None
                }
                _ => {
                    if let Some(result) = history.redo(&mut doc) {
                        result.unwrap_or_else(|e| panic!("seed {seed} step {step}: redo failed: {e}"));
                        kinds.entry("redo").or_default().0 += 1;
                    }
                    None
                }
            };
            if let Some(command) = command {
                match history.execute(&mut doc, command) {
                    Ok(_) => {
                        executed += 1;
                        kinds.entry(kind).or_default().0 += 1;
                    }
                    Err(_) => {
                        rejected += 1;
                        kinds.entry(kind).or_default().1 += 1;
                        assert_eq!(doc, before, "seed {seed} step {step}: a failed command changed the document");
                        assert_eq!(history.state_id(), state_before);
                    }
                }
            }

            assert_eq!(doc.validate(), Ok(()), "seed {seed} step {step}");
            assert!(doc.next_free_id() >= next_id, "ids only grow");
            next_id = doc.next_free_id();
            assert!(next_id > doc.max_id());
            let state = history.state_id();
            match states.get(&state) {
                Some(known) => assert!(
                    known.content_eq(&doc),
                    "seed {seed} step {step}: state {state} reached with different content"
                ),
                None => {
                    states.insert(state, doc.clone());
                }
            }
        }

        while history.can_undo() {
            history.undo(&mut doc).unwrap().unwrap();
        }
        assert_eq!(history.state_id(), 0);
        assert!(doc.content_eq(&start), "seed {seed}: undoing everything returns to the start");
    }
    eprintln!("P1: {executed} applied, {rejected} rejected; per kind (applied, rejected): {kinds:?}");
    assert!(executed > 1000, "{executed} commands executed");
    assert!(rejected > 50, "{rejected} commands rejected");
    for kind in
        ["add platform", "remove platform", "update platform", "set room", "add node", "remove node", "undo", "redo"]
    {
        assert!(kinds.get(kind).is_some_and(|k| k.0 >= 20), "{kind} was applied too rarely: {kinds:?}");
    }
    assert!(kinds["update platform"].1 >= 20, "invalid platform updates were exercised: {kinds:?}");
}
