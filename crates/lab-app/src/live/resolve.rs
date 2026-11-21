//! Turns one write request into one `lab_core::Command`, without touching the document.

use std::rc::Rc;

use lab_catalog::Catalog;
use lab_core::glam::{DQuat, DVec3, EulerRot};
use lab_core::part::{Geometry, ParamValue};
use lab_core::{
    Attach, Command, Document, Finish, HoleRef, HoleSet, LatticeIndex, Node, NodeId, Parent, PlatformId, SelectionTarget, Shape,
};

use super::features::FeatureLookup;
use super::protocol::{ErrorCode, HoleArg, LiveError, ParentArg, PlaceArgs, PlaceAt, WriteRequest};

pub struct Ctx<'a> {
    pub doc: &'a Document,
    pub catalog: &'a Catalog,
    pub holes: &'a dyn Fn(PlatformId) -> Option<Rc<HoleSet>>,
    pub features: &'a dyn FeatureLookup,
    pub trace: &'a lab_optics::Trace,
}

pub struct Resolved {
    pub command: Command,
    pub label: String,
    pub select: Option<SelectionTarget>,
}

const PAINT: [u8; 3] = [210, 210, 205];

/// The `Finish` stored in an imported part's `meta["finish"]` (AnodizedBlack if unknown).
pub fn finish_from_meta(value: &str) -> Finish {
    match value {
        "stainless" => Finish::Stainless,
        "painted" => Finish::Painted { rgb: PAINT },
        _ => Finish::AnodizedBlack,
    }
}

/// ZYX Euler degrees `[z, y, x]` to a rotation.
pub fn euler_deg_to_quat([z, y, x]: [f64; 3]) -> DQuat {
    DQuat::from_euler(EulerRot::ZYX, z.to_radians(), y.to_radians(), x.to_radians())
}

pub fn quat_to_euler_deg(q: DQuat) -> [f64; 3] {
    let (z, y, x) = q.to_euler(EulerRot::ZYX);
    [z.to_degrees(), y.to_degrees(), x.to_degrees()]
}

/// A part node from the catalog; `None` when `part` is not in it.
pub fn new_part_node(
    catalog: &Catalog,
    part: &str,
    id: u64,
    parent: Parent,
    attach: Attach,
    position: DVec3,
    rotation: DQuat,
) -> Option<Node> {
    let def = catalog.get(part)?;
    // The part's main finish: its first mesh piece.
    let finish = match &def.geometry {
        Geometry::Parametric { generator, params } => lab_parts::generate(generator, params)
            .ok()
            .and_then(|mesh| mesh.pieces.first().map(|(f, _)| f.clone())),
        Geometry::Mesh { .. } => def.meta.get("finish").map(|f| finish_from_meta(f)),
    };
    Some(Node {
        select_lock: false,
        note: String::new(),
        id: NodeId(id),
        name: def.name.clone(),
        part: def.id.clone(),
        parent,
        attach,
        position,
        rotation,
        shape: Shape::Part,
        finish: finish.unwrap_or(Finish::AnodizedBlack),
        params: Default::default(),
        pickable: true,
        locked: false,
    })
}

fn err(code: ErrorCode, message: impl Into<String>) -> LiveError {
    LiveError::new(code, message)
}

fn node<'a>(ctx: &'a Ctx, id: u64) -> Result<&'a Node, LiveError> {
    ctx.doc.node(NodeId(id)).ok_or_else(|| err(ErrorCode::NotFound, format!("no node {id}")))
}

pub fn resolve(req: &WriteRequest, ctx: &Ctx) -> Result<Resolved, LiveError> {
    let (command, label, select) = match req {
        WriteRequest::Place(args) => place(args, ctx)?,
        WriteRequest::Move { node: id, position, rotation } => {
            let n = node(ctx, *id)?;
            if position.is_none() && rotation.is_none() {
                return Err(err(ErrorCode::BadRequest, "move needs position and/or rotation"));
            }
            // The gizmo and nudge refuse locked (grounded) nodes; so does the agent.
            if n.locked {
                return Err(err(ErrorCode::Refused, format!("node {} is locked", n.name)));
            }
            if n.attach != Attach::Free {
                return Err(err(ErrorCode::Refused, format!("node {} is attached; detach it first", n.name)));
            }
            let command = Command::SetPlacement {
                id: n.id,
                position: position.map_or(n.position, DVec3::from),
                rotation: rotation.map_or(n.rotation, euler_deg_to_quat),
            };
            (command, format!("Agent: move {}", n.name), Some(SelectionTarget::Node(n.id)))
        }
        WriteRequest::Rename { node: id, name } => {
            let n = node(ctx, *id)?;
            let label = format!("Agent: rename {} to {name}", n.name);
            (Command::Rename { id: n.id, name: name.clone() }, label, Some(SelectionTarget::Node(n.id)))
        }
        WriteRequest::Note { node: id, text } => {
            let n = node(ctx, *id)?;
            let label = format!("Agent: note on {}", n.name);
            (Command::SetNote { id: n.id, note: text.clone() }, label, Some(SelectionTarget::Node(n.id)))
        }
        WriteRequest::SetParams { node: id, params } => {
            let n = node(ctx, *id)?;
            let mut merged = n.params.clone();
            for (k, v) in params {
                let value = match v {
                    serde_json::Value::Number(x) => x.as_f64().map(ParamValue::Number),
                    serde_json::Value::String(s) => Some(ParamValue::Text(s.clone())),
                    serde_json::Value::Bool(b) => Some(ParamValue::Flag(*b)),
                    _ => None,
                };
                let value = value.ok_or_else(|| {
                    err(ErrorCode::BadRequest, format!("parameter {k}: expected a number, string or boolean"))
                })?;
                merged.insert(k.clone(), value);
            }
            let label = format!("Agent: set parameters of {}", n.name);
            (Command::SetParams { id: n.id, params: merged }, label, Some(SelectionTarget::Node(n.id)))
        }
        WriteRequest::Remove { nodes } => {
            if nodes.is_empty() {
                return Err(err(ErrorCode::BadRequest, "remove needs at least one node"));
            }
            for id in nodes {
                node(ctx, *id)?;
            }
            let ids = nodes.iter().map(|i| NodeId(*i)).collect();
            (Command::RemoveNodes(ids), format!("Agent: remove {} parts", nodes.len()), None)
        }
        WriteRequest::Undo | WriteRequest::Redo => {
            return Err(err(ErrorCode::BadRequest, "undo and redo are handled by the app"));
        }
        WriteRequest::Stack(_) | WriteRequest::Constrain(_) => {
            return Err(err(ErrorCode::BadRequest, "this command is not resolved yet"));
        }
    };
    command.clone().apply(&mut ctx.doc.clone()).map_err(|e| err(ErrorCode::Refused, e.to_string()))?;
    Ok(Resolved { command, label, select })
}

fn place(args: &PlaceArgs, ctx: &Ctx) -> Result<(Command, String, Option<SelectionTarget>), LiveError> {
    let def = ctx
        .catalog
        .get(&args.part)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no part {} in the catalog", args.part)))?;
    let id = ctx.doc.next_free_id();
    let (parent, attach, position, rotation) = match &args.at {
        PlaceAt::Hole { platform, hole } => {
            let pid = PlatformId(*platform);
            let p = ctx.doc.platform(pid).ok_or_else(|| err(ErrorCode::NotFound, format!("no platform {platform}")))?;
            let holes = (ctx.holes)(pid)
                .ok_or_else(|| err(ErrorCode::Refused, format!("platform {} has no holes", p.name)))?;
            let (index, at, text) = match hole {
                HoleArg::Label(s) => {
                    let r: HoleRef = s.parse().map_err(|e| err(ErrorCode::BadRequest, e))?;
                    (holes.index_of(r), holes.hole_at(r), r.to_string())
                }
                HoleArg::Index([i, j]) => {
                    let idx = LatticeIndex { i: *i, j: *j };
                    let at = holes.contains(idx).then(|| holes.position(idx));
                    (idx, at, holes.label(idx).map_or_else(|| format!("[{i},{j}]"), |l| l.to_string()))
                }
            };
            let at = at.ok_or_else(|| err(ErrorCode::NotFound, format!("no hole {text} on platform {}", p.name)))?;
            // Same pose a hand drop gets: the part's seat bolted to the hole, turned about +Z.
            let (position, rotation) = lab_mates::seat_pose(def, at, args.rotate_z.unwrap_or(0.0).to_radians());
            (Parent::Platform(pid), Attach::Hole { hole: index }, position, rotation)
        }
        PlaceAt::Mate { on, conn, parent_conn, slide, twist } => {
            let parent = node(ctx, *on)?;
            let pdef = ctx.catalog.get(&parent.part).ok_or_else(|| {
                err(ErrorCode::NotFound, format!("part {} of node {on} is not in the catalog", parent.part))
            })?;
            let pc = pdef.connector(parent_conn).ok_or_else(|| {
                err(ErrorCode::NotFound, format!("part {} has no connector {parent_conn}", pdef.id))
            })?;
            let cc = def
                .connector(conn)
                .ok_or_else(|| err(ErrorCode::NotFound, format!("part {} has no connector {conn}", def.id)))?;
            if !lab_mates::compatible(&cc.kind, &pc.kind) {
                return Err(err(ErrorCode::Refused, format!("{}.{conn} does not fit {}.{parent_conn}", def.id, pdef.id)));
            }
            let (slide, twist) = (slide.unwrap_or(0.0), twist.unwrap_or(0.0).to_radians());
            let (position, rotation) = lab_mates::mate_pose(&pc.frame, &cc.frame, slide, twist);
            let attach = Attach::Mate { self_conn: conn.clone(), parent_conn: parent_conn.clone(), slide, twist };
            (Parent::Node(parent.id), attach, position, rotation)
        }
        PlaceAt::Free { parent, position, rotation } => {
            let parent = match parent {
                None | Some(ParentArg::Room) => Parent::Room,
                Some(ParentArg::Platform(p)) => {
                    ctx.doc.platform(PlatformId(*p)).ok_or_else(|| err(ErrorCode::NotFound, format!("no platform {p}")))?;
                    Parent::Platform(PlatformId(*p))
                }
                Some(ParentArg::Node(n)) => Parent::Node(node(ctx, *n)?.id),
            };
            let rotation = rotation.map_or(DQuat::IDENTITY, euler_deg_to_quat);
            (parent, Attach::Free, DVec3::from(*position), rotation)
        }
    };
    let mut new = new_part_node(ctx.catalog, &args.part, id, parent, attach, position, rotation)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no part {} in the catalog", args.part)))?;
    if let Some(name) = &args.name {
        new.name = name.clone();
    }
    let label = format!("Agent: place {}", def.name);
    Ok((Command::AddNode { node: new, index: None }, label, Some(SelectionTarget::Node(NodeId(id)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::protocol::{ErrorCode, HoleArg, PlaceArgs, PlaceAt};
    use lab_core::part::{Feature, PartDef};
    use lab_core::HoleRef;

    struct NoFeatures;
    impl FeatureLookup for NoFeatures {
        fn detected(&self, _: &Node, _: &PartDef) -> Vec<Feature> {
            vec![]
        }
    }

    const HOLDER: &str = r#"
id = "test:holder"
name = "Test holder"
category = "posts/holders"
[geometry]
kind = "parametric"
generator = "riser"
params = { length = 20, width = 20, height = 20 }
[[connectors]]
name = "bore"
kind = "bore"
diameter = 12.7
gender = "female"
frame = { origin = [0, 0, 20], z = [0, 0, 1], x = [1, 0, 0] }
"#;
    const POST: &str = r#"
id = "test:post"
name = "Test post"
category = "posts/posts"
[geometry]
kind = "parametric"
generator = "riser"
params = { length = 12, width = 12, height = 50 }
[[connectors]]
name = "bottom"
kind = "bore"
diameter = 12.7
gender = "male"
frame = { origin = [0, 0, 0], z = [0, 0, -1], x = [1, 0, 0] }
[[connectors]]
name = "thread"
kind = "thread"
spec = "M6"
gender = "male"
frame = { origin = [0, 0, 50], z = [0, 0, 1], x = [1, 0, 0] }
"#;

    fn catalog() -> Catalog {
        let (builtin, _) = Catalog::load_builtin();
        let mut defs: Vec<PartDef> = builtin.all().into_iter().cloned().collect();
        defs.push(toml::from_str(HOLDER).unwrap());
        defs.push(toml::from_str(POST).unwrap());
        Catalog::from_defs(defs)
    }

    struct World {
        doc: Document,
        catalog: Catalog,
        table: PlatformId,
        holes: Rc<HoleSet>,
    }

    impl World {
        fn new() -> Self {
            let doc = lab_core::sample::demo_document();
            let table = doc.platforms[0].id;
            let holes = Rc::new(lab_core::compute_holes(&doc.platforms[0]).unwrap());
            World { doc, catalog: catalog(), table, holes }
        }
        fn run(&self, req: WriteRequest) -> Result<Resolved, LiveError> {
            let holes = |_: PlatformId| Some(self.holes.clone());
            let trace = lab_optics::Trace::default();
            resolve(&req, &Ctx { doc: &self.doc, catalog: &self.catalog, holes: &holes, features: &NoFeatures, trace: &trace })
        }
        fn free_id(&self) -> u64 {
            self.doc.next_free_id()
        }
        /// A holder in the document, free on the table.
        fn with_holder(mut self) -> (Self, u64) {
            let id = self.free_id();
            let n = new_part_node(&self.catalog, "test:holder", id, Parent::Platform(self.table), Attach::Free, DVec3::ZERO, DQuat::IDENTITY)
                .unwrap();
            Command::AddNode { node: n, index: None }.apply(&mut self.doc).unwrap();
            (self, id)
        }
    }

    fn place_hole(part: &str, hole: &str, platform: PlatformId) -> WriteRequest {
        WriteRequest::Place(PlaceArgs {
            part: part.into(),
            at: PlaceAt::Hole { platform: platform.0, hole: HoleArg::Label(hole.into()) },
            name: None,
            rotate_z: None,
        })
    }

    #[test]
    fn place_in_hole_matches_hole_position() {
        let w = World::new();
        let r = w.run(place_hole("generic:riser", "C14", w.table)).unwrap();
        let Command::AddNode { node, .. } = &r.command else { panic!("{:?}", r.command) };
        let c14: HoleRef = "C14".parse().unwrap();
        let want = w.holes.hole_at(c14).unwrap();
        assert!(matches!(node.attach, Attach::Hole { hole } if hole == w.holes.index_of(c14)));
        assert_eq!(node.parent, Parent::Platform(w.table));
        assert!((node.position.truncate() - want).length() < 1e-9, "{:?} vs {want:?}", node.position);
        assert!(r.label.starts_with("Agent: place "), "{}", r.label);
        assert_eq!(r.select, Some(SelectionTarget::Node(node.id)));
    }

    #[test]
    fn place_in_missing_hole_is_not_found() {
        let w = World::new();
        let e = w.run(place_hole("generic:riser", "Z99", w.table)).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound, "{}", e.message);
        assert!(e.message.starts_with("no hole Z99 on platform "), "{}", e.message);
    }

    fn mate(on: u64, part: &str, conn: &str, parent_conn: &str, slide: f64) -> WriteRequest {
        WriteRequest::Place(PlaceArgs {
            part: part.into(),
            at: PlaceAt::Mate { on, conn: conn.into(), parent_conn: parent_conn.into(), slide: Some(slide), twist: None },
            name: None,
            rotate_z: None,
        })
    }

    #[test]
    fn place_mate_uses_mate_pose() {
        let (w, holder) = World::new().with_holder();
        let r = w.run(mate(holder, "test:post", "bottom", "bore", 10.0)).unwrap();
        let Command::AddNode { node, .. } = &r.command else { panic!() };
        let pf = w.catalog.get("test:holder").unwrap().connector("bore").unwrap().frame;
        let cf = w.catalog.get("test:post").unwrap().connector("bottom").unwrap().frame;
        let (p, q) = lab_mates::mate_pose(&pf, &cf, 10.0, 0.0);
        assert!((node.position - p).length() < 1e-9);
        assert!(node.rotation.angle_between(q) < 1e-9);
        assert_eq!(node.parent, Parent::Node(lab_core::NodeId(holder)));
        assert!(matches!(&node.attach, Attach::Mate { self_conn, parent_conn, slide, .. }
            if self_conn == "bottom" && parent_conn == "bore" && *slide == 10.0));
    }

    #[test]
    fn place_mate_incompatible_is_refused() {
        let (w, holder) = World::new().with_holder();
        let e = w.run(mate(holder, "test:post", "thread", "bore", 0.0)).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused, "{}", e.message);
        let e = w.run(mate(holder, "test:post", "nope", "bore", 0.0)).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn move_unknown_node_is_not_found() {
        let w = World::new();
        let e = w.run(WriteRequest::Move { node: 9999, position: Some([0.0; 3]), rotation: None }).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn move_attached_node_is_refused() {
        let mut w = World::new();
        let id = w.free_id();
        w.run(place_hole("generic:riser", "C14", w.table)).unwrap().command.apply(&mut w.doc).unwrap();
        let e = w.run(WriteRequest::Move { node: id, position: Some([0.0; 3]), rotation: None }).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused);
        assert_eq!(e.message, "node Riser block is attached; detach it first");
    }

    #[test]
    fn move_locked_node_is_refused() {
        let (mut w, holder) = World::new().with_holder();
        Command::SetLocked { id: lab_core::NodeId(holder), locked: true }.apply(&mut w.doc).unwrap();
        let e = w.run(WriteRequest::Move { node: holder, position: Some([0.0; 3]), rotation: None }).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused);
        assert_eq!(e.message, "node Test holder is locked");
        // The UI does not refuse these on a locked node, so neither does the agent.
        assert!(w.run(WriteRequest::Rename { node: holder, name: "H".into() }).is_ok());
        assert!(w.run(WriteRequest::Remove { nodes: vec![holder] }).is_ok());
    }

    #[test]
    fn move_free_node_sets_placement() {
        let (w, holder) = World::new().with_holder();
        let r = w
            .run(WriteRequest::Move { node: holder, position: Some([1.0, 2.0, 3.0]), rotation: Some([90.0, 0.0, 0.0]) })
            .unwrap();
        let Command::SetPlacement { position, rotation, .. } = r.command else { panic!() };
        assert_eq!(position, DVec3::new(1.0, 2.0, 3.0));
        assert!(rotation.angle_between(DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2)) < 1e-9);
        assert_eq!(r.label, "Agent: move Test holder");
    }

    #[test]
    fn rename_and_remove_labels() {
        let (w, holder) = World::new().with_holder();
        let r = w.run(WriteRequest::Rename { node: holder, name: "H1".into() }).unwrap();
        assert_eq!(r.label, "Agent: rename Test holder to H1");
        let r = w.run(WriteRequest::Note { node: holder, text: "hi".into() }).unwrap();
        assert_eq!(r.label, "Agent: note on Test holder");
        let r = w.run(WriteRequest::Remove { nodes: vec![holder] }).unwrap();
        assert_eq!(r.label, "Agent: remove 1 parts");
        assert_eq!(r.select, None);
    }

    #[test]
    fn set_params_merges_over_existing() {
        let (mut w, holder) = World::new().with_holder();
        let mut first = serde_json::Map::new();
        first.insert("height".into(), serde_json::json!(30.0));
        let r = w.run(WriteRequest::SetParams { node: holder, params: first }).unwrap();
        r.command.apply(&mut w.doc).unwrap();
        let mut second = serde_json::Map::new();
        second.insert("label".into(), serde_json::json!("x"));
        let r = w.run(WriteRequest::SetParams { node: holder, params: second }).unwrap();
        let Command::SetParams { params, .. } = r.command else { panic!() };
        assert_eq!(params.len(), 2);
        assert_eq!(params["height"].as_f64(), Some(30.0));
    }

    #[test]
    fn euler_round_trip() {
        for a in [[0.0, 0.0, 0.0], [30.0, 20.0, -10.0], [-170.0, 45.0, 80.0], [90.0, -60.0, 120.0]] {
            let b = quat_to_euler_deg(euler_deg_to_quat(a));
            assert!(euler_deg_to_quat(b).angle_between(euler_deg_to_quat(a)) < 1e-9, "{a:?} {b:?}");
            if a[1].abs() < 89.0 && a[0].abs() < 180.0 && a[2].abs() < 180.0 {
                for k in 0..3 {
                    assert!((a[k] - b[k]).abs() < 1e-6, "{a:?} {b:?}");
                }
            }
        }
    }
}
