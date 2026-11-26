//! Turns one write request into one `lab_core::Command`, without touching the document.

use std::rc::Rc;

use lab_catalog::Catalog;
use lab_core::glam::{DQuat, DVec3, EulerRot};
use lab_core::part::{ConnectorKind, Feature, Geometry, ParamValue, PartDef};
use lab_core::{
    Attach, Command, Constraint, ConstraintKind, Document, Finish, HoleRef, HoleSet, LatticeIndex, Node, NodeId, Parent, PlatformId, SelectionTarget, Shape,
};

use lab_optics::PortKind;

use super::features::{FeatureLookup, node_features, platform_features};
use super::protocol::{
    ConstrainArgs, ErrorCode, HoleArg, LiveError, ParentArg, PlaceArgs, PlaceAt, PlaceBundleArgs, StackArgs, WriteRequest,
};

pub struct Ctx<'a> {
    pub doc: &'a Document,
    pub catalog: &'a Catalog,
    pub holes: &'a dyn Fn(PlatformId) -> Option<Rc<HoleSet>>,
    pub features: &'a dyn FeatureLookup,
    pub trace: &'a lab_optics::Trace,
    /// The library's saved bundles (assemblies), by name.
    pub bundles: &'a [lab_file::Bundle],
}

pub struct Resolved {
    pub command: Command,
    pub label: String,
    pub select: Option<SelectionTarget>,
}

/// Node params of a laser that the inspector's Beam section edits (`beams.rs` PARAMS + on/off).
pub const LASER_PARAMS: &[&str] =
    &["beam_wavelength_nm", "beam_waist_mm", "beam_m2", "beam_on", "beam_pol_angle_deg", "beam_pol_ellipticity_deg"];
/// Angle params of waveplate and isolator parts.
pub const OPTIC_PARAMS: &[&str] = &["wp_angle_deg", "iso_angle_deg"];

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
            let def = ctx.catalog.get(&n.part);
            // As the inspector: the generator's declared params, plus the beam params of a laser.
            let declared = |k: &str| {
                def.is_some_and(|d| {
                    matches!(&d.geometry, Geometry::Parametric { params, .. } if params.contains_key(k))
                        || (LASER_PARAMS.contains(&k)
                            && d.optical_ports.iter().any(|p| PortKind::of(&p.kind, d) == PortKind::Source))
                        || (k == "wp_angle_deg"
                            && d.optical_ports.iter().any(|p| matches!(PortKind::of(&p.kind, d), PortKind::Hwp | PortKind::Qwp)))
                        || (k == "iso_angle_deg"
                            && d.optical_ports.iter().any(|p| PortKind::of(&p.kind, d) == PortKind::Isolator))
                })
            };
            let mut merged = n.params.clone();
            for (k, v) in params {
                if !declared(k) {
                    return Err(err(ErrorCode::BadRequest, format!("part {} has no parameter '{k}'", n.part)));
                }
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
            (lab_core::remove_nodes_command(ctx.doc, ids), format!("Agent: remove {} parts", nodes.len()), None)
        }
        WriteRequest::Undo | WriteRequest::Redo => {
            return Err(err(ErrorCode::BadRequest, "undo and redo are handled by the app"));
        }
        WriteRequest::Stack(args) => stack(args, ctx)?,
        WriteRequest::PlaceBundle(args) => place_bundle(args, ctx)?,
        WriteRequest::Constrain(args) => constrain(args, ctx)?,
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
        PlaceAt::Hole { platform, hole } => hole_pose(ctx, def, *platform, hole, args.rotate_z)?,
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

/// Where `def` sits when bolted into a hole, as a hand drop would: its seat on the hole, turned about +Z.
fn hole_pose(
    ctx: &Ctx,
    def: &PartDef,
    platform: u64,
    hole: &HoleArg,
    rotate_z: Option<f64>,
) -> Result<(Parent, Attach, DVec3, DQuat), LiveError> {
    let pid = PlatformId(platform);
    let p = ctx.doc.platform(pid).ok_or_else(|| err(ErrorCode::NotFound, format!("no platform {platform}")))?;
    let holes = (ctx.holes)(pid).ok_or_else(|| err(ErrorCode::Refused, format!("platform {} has no holes", p.name)))?;
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
    let (position, rotation) = lab_mates::seat_pose(def, at, rotate_z.unwrap_or(0.0).to_radians());
    Ok((Parent::Platform(pid), Attach::Hole { hole: index }, position, rotation))
}

fn missing_feature(owner: &str, id: &str) -> LiveError {
    err(ErrorCode::NotFound, format!("{owner} has no feature '{id}'; use features to list them"))
}

type ParentLookup = Box<dyn Fn(&str) -> Result<Feature, LiveError>>;

fn constrain(args: &ConstrainArgs, ctx: &Ctx) -> Result<(Command, String, Option<SelectionTarget>), LiveError> {
    let child = node(ctx, args.node)?;
    if child.locked {
        return Err(err(ErrorCode::Refused, format!("node {} is locked", child.name)));
    }
    let def_of = |n: &Node| {
        ctx.catalog
            .get(&n.part)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("part {} of node {} is not in the catalog", n.part, n.id.0)))
    };
    let child_features = node_features(child, def_of(child)?, ctx.features);
    let (parent, parent_name, parent_feature): (Parent, String, ParentLookup) = match &args.to {
        ParentArg::Room => return Err(err(ErrorCode::BadRequest, "constrain needs a node or platform to constrain to")),
        ParentArg::Platform(p) => {
            let pid = PlatformId(*p);
            let platform = ctx.doc.platform(pid).ok_or_else(|| err(ErrorCode::NotFound, format!("no platform {p}")))?;
            // `top` needs no grid; with none, `hole:` labels are simply not found.
            let holes = (ctx.holes)(pid).unwrap_or_else(|| Rc::new(HoleSet::empty()));
            let lookup = platform_features(platform, &holes);
            let owner = format!("platform {p}");
            (Parent::Platform(pid), platform.name.clone(), Box::new(move |id| lookup(id).ok_or_else(|| missing_feature(&owner, id))))
        }
        ParentArg::Node(n) => {
            let pn = node(ctx, *n)?;
            if pn.id == child.id || ctx.doc.descendants(child.id).contains(&pn.id) {
                return Err(err(ErrorCode::Refused, format!("node {} is carried by {}", pn.name, child.name)));
            }
            let features = node_features(pn, def_of(pn)?, ctx.features);
            let owner = format!("node {n}");
            (
                Parent::Node(pn.id),
                pn.name.clone(),
                Box::new(move |id| {
                    features.iter().find(|f| f.id == id).map(|f| f.feature.clone()).ok_or_else(|| missing_feature(&owner, id))
                }),
            )
        }
    };
    // As the panel: the node's existing constraints to the same parent are kept.
    let mut constraints = match &child.attach {
        Attach::Constrained { constraints } if child.parent == parent => constraints.clone(),
        _ => Vec::new(),
    };
    for c in &args.constraints {
        let cf = child_features
            .iter()
            .find(|f| f.id == c.child)
            .map(|f| f.feature.clone())
            .ok_or_else(|| missing_feature(&format!("node {}", args.node), &c.child))?;
        let pf = parent_feature(&c.parent)?;
        let default = lab_mates::constraints::default_kind(&cf, &pf)
            .ok_or_else(|| err(ErrorCode::Refused, "these features can't be constrained to each other"))?;
        let kind = c.kind.unwrap_or(default);
        // As the panel: Concentric takes axes, Mate and Flush take faces, Angle either.
        let wrong = match (kind, &cf) {
            (ConstraintKind::Concentric, Feature::Plane { .. }) => Some("two axes"),
            (ConstraintKind::Mate | ConstraintKind::Flush, Feature::Axis { .. }) => Some("two faces"),
            _ => None,
        };
        if let Some(need) = wrong {
            return Err(err(ErrorCode::Refused, format!("{kind:?} needs {need}")));
        }
        // Omitted flip: the panel's least-motion choice for axis pairs.
        let flip = c.flip.unwrap_or_else(|| match (&cf, &pf) {
            (Feature::Axis { dir: cd, .. }, Feature::Axis { dir: pd, .. }) => {
                let world = |at| lab_mates::constraints::world_pose(ctx.doc, at).map(|(_, q)| q);
                match (world(Parent::Node(child.id)), world(parent)) {
                    (Some(cq), Some(pq)) => lab_mates::constraints::least_motion_flip(cq, *cd, pq, *pd),
                    _ => false,
                }
            }
            _ => false,
        });
        constraints.push(Constraint {
            kind,
            child: cf,
            parent: pf,
            offset: c.offset.unwrap_or(0.0),
            angle: c.angle.unwrap_or(0.0).to_radians(),
            flip,
            lock_rotation: c.lock_rotation.unwrap_or(false),
        });
    }
    let command = lab_mates::constraints::constrain(ctx.doc, child.id, parent, constraints)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no node {}", args.node)))?;
    Ok((command, format!("Agent: constrain {} to {parent_name}", child.name), Some(SelectionTarget::Node(child.id))))
}

fn spec_of(args: &StackArgs, slide: f64) -> lab_viewport::StackSpec {
    lab_viewport::StackSpec {
        optic: args.optic.clone(),
        mount: args.mount.clone(),
        base: args.base.clone(),
        holder: args.holder.clone(),
        post: args.post.clone(),
        slide,
    }
}

fn no_chain() -> LiveError {
    err(ErrorCode::NotFound, "a stack part or connector is not in the catalog")
}

/// Height of the top optic axis above the root part's origin, with the post slid by `spec.slide`.
fn axis_height_of(catalog: &Catalog, spec: &lab_viewport::StackSpec) -> Result<f64, LiveError> {
    let chain = lab_viewport::stack_chain(catalog, spec).ok_or_else(no_chain)?;
    // Compose the parent-relative poses upward from the root's own frame.
    let (mut p, mut q) = (DVec3::ZERO, DQuat::IDENTITY);
    for (_, _, pos, rot) in &chain {
        p += q * *pos;
        q *= *rot;
    }
    let (top, conn) = match &spec.optic {
        Some(o) => (o, "edge"),
        None => (&spec.mount, "optic_seat"),
    };
    let frame = catalog.get(top).and_then(|d| d.connector(conn)).ok_or_else(no_chain)?.frame;
    Ok((p + q * frame.origin).z)
}

fn bore_range(catalog: &Catalog, holder: &str) -> Result<Option<[f64; 2]>, LiveError> {
    let c = catalog
        .get(holder)
        .and_then(|d| d.connector("bore"))
        .ok_or_else(|| err(ErrorCode::NotFound, format!("holder {holder} has no bore connector")))?;
    Ok(match &c.kind {
        ConnectorKind::Bore { range, .. } => *range,
        _ => None,
    })
}

/// The slide that puts the top optic axis `axis_height` mm above the root part's origin (the platform top for a
/// part bolted flat in a hole). `spec_without_slide.slide` is ignored.
pub fn slide_for_axis_height(
    catalog: &Catalog,
    spec_without_slide: &lab_viewport::StackSpec,
    axis_height: f64,
) -> Result<f64, LiveError> {
    let at_zero = axis_height_of(catalog, &lab_viewport::StackSpec { slide: 0.0, ..spec_without_slide.clone() })?;
    let slide = axis_height - at_zero;
    match bore_range(catalog, &spec_without_slide.holder)? {
        Some([lo, hi]) if slide < lo - 1e-9 || slide > hi + 1e-9 => Err(err(
            ErrorCode::Refused,
            format!("beam height {axis_height} mm is out of reach for this post ({:.1}\u{2013}{:.1} mm)", at_zero + lo, at_zero + hi),
        )),
        _ => Ok(slide),
    }
}

fn stack(args: &StackArgs, ctx: &Ctx) -> Result<(Command, String, Option<SelectionTarget>), LiveError> {
    let root_part = args.base.as_deref().unwrap_or(&args.holder);
    let root_def = ctx
        .catalog
        .get(root_part)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no part {root_part} in the catalog")))?;
    let (parent, attach, position, rotation) = hole_pose(ctx, root_def, args.platform, &args.hole, args.rotate_z)?;
    let slide = match (args.axis_height, args.slide) {
        // shortcut: assumes the root's seat keeps +Z up (true for hole seats, which only turn about Z).
        (Some(h), None) => slide_for_axis_height(ctx.catalog, &spec_of(args, 0.0), h - position.z)?,
        (None, Some(s)) => {
            if let Some([lo, hi]) = bore_range(ctx.catalog, &args.holder)? {
                if s < lo - 1e-9 || s > hi + 1e-9 {
                    return Err(err(ErrorCode::Refused, format!("slide {s} mm is outside the bore range ({lo}\u{2013}{hi} mm)")));
                }
            }
            s
        }
        _ => return Err(err(ErrorCode::BadRequest, "stack needs exactly one of axis_height and slide")),
    };
    let chain = lab_viewport::stack_chain(ctx.catalog, &spec_of(args, slide)).ok_or_else(no_chain)?;
    let first = ctx.doc.next_free_id();
    let mut nodes = vec![new_part_node(ctx.catalog, root_part, first, parent, attach, position, rotation).ok_or_else(no_chain)?];
    for (part, attach, position, rotation) in chain {
        let below = Parent::Node(nodes.last().unwrap().id);
        let id = first + nodes.len() as u64;
        nodes.push(new_part_node(ctx.catalog, &part, id, below, attach, position, rotation).ok_or_else(no_chain)?);
    }
    let mount = ctx.catalog.get(&args.mount).map_or(args.mount.as_str(), |d| d.name.as_str());
    let at = match &args.hole {
        HoleArg::Label(s) => s.clone(),
        HoleArg::Index([i, j]) => format!("[{i},{j}]"),
    };
    Ok((Command::PlaceAssembly { nodes }, format!("Agent: stack {mount} at {at}"), Some(SelectionTarget::Node(NodeId(first)))))
}

fn place_bundle(args: &PlaceBundleArgs, ctx: &Ctx) -> Result<(Command, String, Option<SelectionTarget>), LiveError> {
    let bundle = ctx.bundles.iter().find(|b| b.name == args.name).ok_or_else(|| {
        let names: Vec<&str> = ctx.bundles.iter().map(|b| b.name.as_str()).collect();
        err(ErrorCode::NotFound, format!("no bundle named {:?}; saved bundles: {}", args.name, names.join(", ")))
    })?;
    // As a drop: the anchor (first node) takes the hole's seat pose; the rest follow it.
    let anchor = bundle.nodes.first().ok_or_else(|| err(ErrorCode::Refused, format!("bundle {} is empty", bundle.name)))?;
    let anchor_def = ctx
        .catalog
        .get(&anchor.part)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("bundle {} starts with {}, which is not in the catalog", bundle.name, anchor.part)))?;
    let (parent, attach, position, _) = hole_pose(ctx, anchor_def, args.platform, &args.hole, None)?;
    let keep = |n: &Node| !matches!(n.shape, Shape::Part) || ctx.catalog.get(&n.part).is_some();
    let next = ctx.doc.next_free_id();
    let (mut nodes, skipped) = crate::bundle::place_bundle(&bundle.nodes, next, parent, &attach, position, keep);
    if skipped > 0 {
        // A half-placed assembly is worse than none: name what is missing instead.
        let missing: Vec<&str> = bundle.nodes.iter().filter(|n| !keep(n)).map(|n| n.part.as_str()).collect();
        return Err(err(ErrorCode::NotFound, format!("bundle {} uses parts not in the catalog: {}", bundle.name, missing.join(", "))));
    }
    if let Some(angle) = args.rotate_z.filter(|a| a.abs() > 1e-9) {
        crate::bundle::turn_roots(&mut nodes, position, angle);
    }
    let at = match &args.hole {
        HoleArg::Label(s) => s.clone(),
        HoleArg::Index([i, j]) => format!("[{i},{j}]"),
    };
    let first = nodes[0].id;
    let command = crate::bundle::bundle_place_command(bundle, nodes, next);
    Ok((command, format!("Agent: place bundle {} at {at}", bundle.name), Some(SelectionTarget::Node(first))))
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
range = [-30, 30]
frame = { origin = [0, 0, 20], z = [0, 0, 1], x = [1, 0, 0] }
"#;
    const MOUNT: &str = r#"
id = "test:mount"
name = "Test mount"
category = "mounts"
[geometry]
kind = "parametric"
generator = "riser"
params = { length = 30, width = 30, height = 25 }
[[connectors]]
name = "base_tap"
kind = "thread"
spec = "M6"
gender = "female"
frame = { origin = [0, 0, 0], z = [0, 0, -1], x = [1, 0, 0] }
[[connectors]]
name = "optic_seat"
kind = "optic_bore"
diameter = 25.4
gender = "female"
frame = { origin = [0, 0, 25], z = [0, 0, 1], x = [1, 0, 0] }
"#;
    const OPTIC: &str = r#"
id = "test:optic"
name = "Test optic"
category = "optics"
[geometry]
kind = "parametric"
generator = "riser"
params = { length = 25, width = 25, height = 6 }
[[connectors]]
name = "edge"
kind = "optic_bore"
diameter = 25.4
gender = "male"
frame = { origin = [0, 0, 0], z = [0, 0, -1], x = [1, 0, 0] }
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
[[connectors]]
name = "top"
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
        defs.push(toml::from_str(MOUNT).unwrap());
        defs.push(toml::from_str(OPTIC).unwrap());
        Catalog::from_defs(defs)
    }

    struct World {
        doc: Document,
        catalog: Catalog,
        table: PlatformId,
        holes: Rc<HoleSet>,
        bundles: Vec<lab_file::Bundle>,
    }

    impl World {
        fn new() -> Self {
            let doc = lab_core::sample::demo_document();
            let table = doc.platforms[0].id;
            let holes = Rc::new(lab_core::compute_holes(&doc.platforms[0]).unwrap());
            World { doc, catalog: catalog(), table, holes, bundles: Vec::new() }
        }
        fn run(&self, req: WriteRequest) -> Result<Resolved, LiveError> {
            self.run_with(req, &NoFeatures)
        }
        fn run_with(&self, req: WriteRequest, features: &dyn FeatureLookup) -> Result<Resolved, LiveError> {
            let holes = |_: PlatformId| Some(self.holes.clone());
            let trace = lab_optics::Trace::default();
            resolve(&req, &Ctx { doc: &self.doc, catalog: &self.catalog, holes: &holes, features, trace: &trace, bundles: &self.bundles })
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
    fn remove_grouped_node_updates_groups() {
        let (w, a) = World::new().with_holder();
        let (mut w, b) = w.with_holder();
        let gid = lab_core::GroupId(w.doc.allocate_id());
        w.doc.groups.push(lab_core::Group {
            id: gid,
            name: "Group 1".into(),
            members: vec![NodeId(a), NodeId(b)],
        });
        let r = w.run(WriteRequest::Remove { nodes: vec![a] }).unwrap();
        r.command.apply(&mut w.doc).unwrap();
        assert_eq!(w.doc.groups[0].members, vec![NodeId(b)], "removing a member must drop it from the group");
        let r = w.run(WriteRequest::Remove { nodes: vec![b] }).unwrap();
        r.command.apply(&mut w.doc).unwrap();
        assert!(w.doc.groups.is_empty(), "removing the last member must drop the group");
    }

    #[test]
    fn set_params_merges_over_existing() {
        let (mut w, holder) = World::new().with_holder();
        let mut first = serde_json::Map::new();
        first.insert("height".into(), serde_json::json!(30.0));
        let r = w.run(WriteRequest::SetParams { node: holder, params: first }).unwrap();
        r.command.apply(&mut w.doc).unwrap();
        let mut second = serde_json::Map::new();
        second.insert("width".into(), serde_json::json!(25.0));
        let r = w.run(WriteRequest::SetParams { node: holder, params: second }).unwrap();
        let Command::SetParams { params, .. } = r.command else { panic!() };
        assert_eq!(params.len(), 2);
        assert_eq!(params["height"].as_f64(), Some(30.0));
    }

    #[test]
    fn set_params_refuses_undeclared_keys() {
        let (w, holder) = World::new().with_holder();
        let params = |k: &str| serde_json::Map::from_iter([(k.to_string(), serde_json::json!(1.0))]);
        let e = w.run(WriteRequest::SetParams { node: holder, params: params("label") }).err().unwrap();
        assert_eq!(e.code, ErrorCode::BadRequest);
        assert_eq!(e.message, "part test:holder has no parameter 'label'");
        // Beam params belong to lasers only.
        assert!(w.run(WriteRequest::SetParams { node: holder, params: params("beam_wavelength_nm") }).is_err());

        let mut w = w;
        let id = w.free_id();
        let laser = new_part_node(&w.catalog, "generic:starter-laser", id, Parent::Platform(w.table), Attach::Free, DVec3::ZERO, DQuat::IDENTITY)
            .unwrap();
        Command::AddNode { node: laser, index: None }.apply(&mut w.doc).unwrap();
        assert!(w.run(WriteRequest::SetParams { node: id, params: params("beam_wavelength_nm") }).is_ok());
        assert!(w.run(WriteRequest::SetParams { node: id, params: params("length") }).is_ok());
    }

    /// A holder with a post on it, and a second root (a mount) 25 mm along +X of the holder.
    fn pair_bundle(w: &World) -> lab_file::Bundle {
        let part = |part: &str, id: u64, parent: Parent, at: DVec3| {
            new_part_node(&w.catalog, part, id, parent, Attach::Free, at, DQuat::IDENTITY).unwrap()
        };
        lab_file::Bundle {
            name: "Pair".into(),
            nodes: vec![
                part("test:holder", 7, Parent::Platform(w.table), DVec3::ZERO),
                part("test:post", 8, Parent::Node(NodeId(7)), DVec3::new(0.0, 0.0, 10.0)),
                part("test:mount", 9, Parent::Platform(w.table), DVec3::new(25.0, 0.0, 0.0)),
            ],
            group: None,
        }
    }

    fn bundle_args(w: &World, name: &str, rotate_z: Option<f64>) -> WriteRequest {
        WriteRequest::PlaceBundle(PlaceBundleArgs { name: name.into(), platform: w.table.0, hole: HoleArg::Label("C14".into()), rotate_z })
    }

    #[test]
    fn place_bundle_puts_the_anchor_on_the_hole_with_fresh_ids() {
        let mut w = World::new();
        w.bundles = vec![pair_bundle(&w)];
        let r = w.run(bundle_args(&w, "Pair", None)).unwrap();
        let Command::PlaceAssembly { nodes } = &r.command else { panic!("{:?}", r.command) };
        let first = w.free_id();
        assert_eq!(nodes.iter().map(|n| n.id.0).collect::<Vec<_>>(), [first, first + 1, first + 2]);
        assert_eq!(nodes[1].parent, Parent::Node(NodeId(first)), "child follows its remapped parent");
        assert_eq!(nodes[1].position, DVec3::new(0.0, 0.0, 10.0), "child pose stays parent-relative");
        // The anchor lands where a single holder placed on C14 would.
        let single = w.run(WriteRequest::Place(PlaceArgs {
            part: "test:holder".into(),
            at: PlaceAt::Hole { platform: w.table.0, hole: HoleArg::Label("C14".into()) },
            name: None,
            rotate_z: None,
        }));
        let Command::AddNode { node, .. } = single.unwrap().command else { panic!() };
        assert!((nodes[0].position - node.position).length() < 1e-9);
        assert!((nodes[2].position - node.position - DVec3::new(25.0, 0.0, 0.0)).length() < 1e-9);
    }

    #[test]
    fn place_bundle_turns_roots_about_the_hole() {
        let mut w = World::new();
        w.bundles = vec![pair_bundle(&w)];
        let Command::PlaceAssembly { nodes } = w.run(bundle_args(&w, "Pair", Some(90.0))).unwrap().command else { panic!() };
        let pivot = nodes[0].position;
        assert!((nodes[2].position - pivot - DVec3::new(0.0, 25.0, 0.0)).length() < 1e-9, "{:?}", nodes[2].position - pivot);
        let turned = nodes[0].rotation * DVec3::X;
        assert!((turned - DVec3::Y).length() < 1e-9, "anchor turned 90 deg: {turned:?}");
        assert_eq!(nodes[1].position, DVec3::new(0.0, 0.0, 10.0), "children are not turned twice");
    }

    #[test]
    fn place_bundle_refuses_unknown_names_and_missing_parts() {
        let mut w = World::new();
        let mut broken = pair_bundle(&w);
        broken.name = "Broken".into();
        broken.nodes[2].part = "nope:gone".into();
        w.bundles = vec![pair_bundle(&w), broken];
        let e = w.run(bundle_args(&w, "Nope", None)).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(e.message.contains("Pair"), "lists saved bundles: {}", e.message);
        let e = w.run(bundle_args(&w, "Broken", None)).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(e.message.contains("nope:gone"), "{}", e.message);
    }

    fn stack_args(axis_height: Option<f64>, slide: Option<f64>) -> StackArgs {
        StackArgs {
            platform: 0,
            hole: HoleArg::Label("C14".into()),
            holder: "test:holder".into(),
            post: "test:post".into(),
            mount: "test:mount".into(),
            base: None,
            optic: Some("test:optic".into()),
            axis_height,
            slide,
            rotate_z: None,
        }
    }

    fn stack_world() -> World {
        World::new()
    }

    #[test]
    fn stack_matches_library_chain() {
        let w = stack_world();
        let mut args = stack_args(None, Some(5.0));
        args.platform = w.table.0;
        let r = w.run(WriteRequest::Stack(args)).unwrap();
        let Command::PlaceAssembly { nodes } = &r.command else { panic!("{:?}", r.command) };
        let spec = lab_viewport::StackSpec {
            optic: Some("test:optic".into()),
            mount: "test:mount".into(),
            base: None,
            holder: "test:holder".into(),
            post: "test:post".into(),
            slide: 5.0,
        };
        let chain = lab_viewport::stack_chain(&w.catalog, &spec).unwrap();
        assert_eq!(nodes.len(), chain.len() + 1);
        let first = w.free_id();
        for (k, (part, attach, position, rotation)) in chain.iter().enumerate() {
            let n = &nodes[k + 1];
            assert_eq!(n.id.0, first + k as u64 + 1);
            assert_eq!(&n.part, part);
            assert_eq!(&n.attach, attach);
            assert!((n.position - *position).length() < 1e-9);
            assert!(n.rotation.angle_between(*rotation) < 1e-9);
            assert_eq!(n.parent, Parent::Node(nodes[k].id));
        }
        assert_eq!(nodes[0].part, "test:holder");
        assert!(matches!(nodes[0].attach, Attach::Hole { .. }));
        assert_eq!(r.label, "Agent: stack Test mount at C14");
        assert_eq!(r.select, Some(SelectionTarget::Node(NodeId(first))));
    }

    #[test]
    fn stack_axis_height_lands_at_height() {
        let w = stack_world();
        let mut args = stack_args(Some(75.0), None);
        args.platform = w.table.0;
        let r = w.run(WriteRequest::Stack(args)).unwrap();
        let mut doc = w.doc.clone();
        r.command.clone().apply(&mut doc).unwrap();
        let Command::PlaceAssembly { nodes } = &r.command else { panic!() };
        let top = nodes.last().unwrap();
        let edge = w.catalog.get("test:optic").unwrap().connector("edge").unwrap().frame.origin;
        let (p, q) = lab_mates::constraints::world_pose(&doc, Parent::Node(top.id)).unwrap();
        let (pp, _) = lab_mates::constraints::world_pose(&doc, Parent::Platform(w.table)).unwrap();
        let z = (p + q * edge).z - pp.z;
        assert!((z - 75.0).abs() < 1e-6, "axis at {z}");
    }

    #[test]
    fn stack_height_out_of_range_is_refused() {
        let w = stack_world();
        let mut args = stack_args(Some(1000.0), None);
        args.platform = w.table.0;
        let e = w.run(WriteRequest::Stack(args)).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused, "{}", e.message);
        assert!(e.message.starts_with("beam height 1000 mm is out of reach for this post (65.0"), "{}", e.message);
    }

    #[test]
    fn stack_needs_one_of_height_or_slide() {
        let w = stack_world();
        for (h, s) in [(None, None), (Some(75.0), Some(0.0))] {
            let mut args = stack_args(h, s);
            args.platform = w.table.0;
            let e = w.run(WriteRequest::Stack(args)).err().unwrap();
            assert_eq!(e.code, ErrorCode::BadRequest, "{}", e.message);
        }
    }

    /// A holder and a free post, both free on the table.
    fn holder_and_post() -> (World, u64, u64) {
        let (mut w, holder) = World::new().with_holder();
        let post = w.free_id();
        let n = new_part_node(
            &w.catalog,
            "test:post",
            post,
            Parent::Platform(w.table),
            Attach::Free,
            DVec3::new(50.0, 10.0, 0.0),
            DQuat::IDENTITY,
        )
        .unwrap();
        Command::AddNode { node: n, index: None }.apply(&mut w.doc).unwrap();
        (w, holder, post)
    }

    fn constrain_req(node: u64, to: ParentArg, child: &str, parent: &str, kind: Option<ConstraintKind>) -> WriteRequest {
        WriteRequest::Constrain(ConstrainArgs {
            node,
            to,
            constraints: vec![crate::live::protocol::ConstraintArg {
                child: child.into(),
                parent: parent.into(),
                kind,
                offset: None,
                angle: None,
                flip: None,
                lock_rotation: None,
            }],
        })
    }

    fn concentric(w: &World, child: (&str, &str), parent: Feature, flip: bool) -> Constraint {
        let cf = crate::live::features::connector_feature(w.catalog.get(child.0).unwrap().connector(child.1).unwrap());
        Constraint { kind: ConstraintKind::Concentric, child: cf, parent, offset: 0.0, angle: 0.0, flip, lock_rotation: false }
    }

    /// The post's `bottom` (-Z) into the upright holder bore (+Z): least motion flips.
    #[test]
    fn constrain_omitted_flip_is_least_motion() {
        let (w, holder, post) = holder_and_post();
        let bore = crate::live::features::connector_feature(w.catalog.get("test:holder").unwrap().connector("bore").unwrap());
        let Feature::Axis { dir, .. } = &bore else { panic!() };
        let bottom = w.catalog.get("test:post").unwrap().connector("bottom").unwrap().frame.z;
        let want = lab_mates::constraints::least_motion_flip(DQuat::IDENTITY, bottom, DQuat::IDENTITY, *dir);
        assert!(want);
        let r = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", None)).unwrap();
        let Command::SetAttach { attach: Attach::Constrained { constraints }, .. } = &r.command else { panic!() };
        assert_eq!(constraints[0].flip, want);
        // An explicit flip wins.
        let mut req = constrain_req(post, ParentArg::Node(holder), "bottom", "bore", None);
        let WriteRequest::Constrain(a) = &mut req else { panic!() };
        a.constraints[0].flip = Some(false);
        let r = w.run(req).unwrap();
        let Command::SetAttach { attach: Attach::Constrained { constraints }, .. } = &r.command else { panic!() };
        assert!(!constraints[0].flip);
    }

    struct Plane;
    impl FeatureLookup for Plane {
        fn detected(&self, _: &Node, _: &PartDef) -> Vec<Feature> {
            vec![Feature::Plane { origin: DVec3::ZERO, normal: DVec3::Z }]
        }
    }

    #[test]
    fn constrain_twice_appends() {
        let (mut w, holder, post) = holder_and_post();
        let first = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", Some(ConstraintKind::Concentric))).unwrap();
        first.command.apply(&mut w.doc).unwrap();
        let second = w
            .run_with(constrain_req(post, ParentArg::Node(holder), "f1", "f1", Some(ConstraintKind::Flush)), &Plane)
            .unwrap();
        let Command::SetAttach { attach: Attach::Constrained { constraints }, .. } = &second.command else { panic!() };
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].kind, ConstraintKind::Concentric);
        assert_eq!(constraints[1].kind, ConstraintKind::Flush);
    }

    #[test]
    fn constrain_with_connector_ids() {
        let (w, holder, post) = holder_and_post();
        let r = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", Some(ConstraintKind::Concentric))).unwrap();
        let bore = crate::live::features::connector_feature(w.catalog.get("test:holder").unwrap().connector("bore").unwrap());
        let want = lab_mates::constraints::constrain(
            &w.doc,
            NodeId(post),
            Parent::Node(NodeId(holder)),
            vec![concentric(&w, ("test:post", "bottom"), bore, true)],
        )
        .unwrap();
        assert_eq!(r.command, want);
        let Command::SetAttach { attach: Attach::Constrained { constraints }, .. } = &r.command else { panic!("{:?}", r.command) };
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].kind, ConstraintKind::Concentric);
        assert_eq!(r.label, "Agent: constrain Test post to Test holder");
        assert_eq!(r.select, Some(SelectionTarget::Node(NodeId(post))));
    }

    #[test]
    fn constrain_default_kind() {
        let (w, holder, post) = holder_and_post();
        let explicit = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", Some(ConstraintKind::Concentric))).unwrap();
        let defaulted = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", None)).unwrap();
        assert_eq!(defaulted.command, explicit.command);
    }

    #[test]
    fn constrain_to_platform_hole() {
        let (w, _, post) = holder_and_post();
        let r = w.run(constrain_req(post, ParentArg::Platform(w.table.0), "bottom", "hole:C14", None)).unwrap();
        let at = w.holes.hole_at("C14".parse().unwrap()).unwrap();
        let Command::SetAttach { parent, attach: Attach::Constrained { constraints }, .. } = &r.command else { panic!() };
        assert_eq!(*parent, Parent::Platform(w.table));
        assert_eq!(
            constraints[0].parent,
            Feature::Axis { origin: at.extend(0.0), dir: DVec3::Z, radius: w.holes.thread_radius, hole: true }
        );
    }

    #[test]
    fn constrain_to_platform_top_needs_no_hole_grid() {
        let (w, _, post) = holder_and_post();
        let no_grid = |_: PlatformId| None;
        let trace = lab_optics::Trace::default();
        let ctx = Ctx { doc: &w.doc, catalog: &w.catalog, holes: &no_grid, features: &Plane, trace: &trace, bundles: &[] };
        let to = || ParentArg::Platform(w.table.0);
        assert!(resolve(&constrain_req(post, to(), "f1", "top", Some(ConstraintKind::Flush)), &ctx).is_ok());
        let e = resolve(&constrain_req(post, to(), "f1", "hole:C14", None), &ctx).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound, "{}", e.message);
    }

    #[test]
    fn constrain_unknown_feature_is_not_found() {
        let (w, holder, post) = holder_and_post();
        let e = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "f9", None)).err().unwrap();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.message, format!("node {holder} has no feature 'f9'; use features to list them"));
        let e = w.run(constrain_req(post, ParentArg::Node(holder), "zzz", "bore", None)).err().unwrap();
        assert_eq!(e.message, format!("node {post} has no feature 'zzz'; use features to list them"));
    }

    #[test]
    fn constrain_mismatched_features_are_refused() {
        let (w, _, post) = holder_and_post();
        let e = w.run(constrain_req(post, ParentArg::Platform(w.table.0), "bottom", "top", None)).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused);
        assert_eq!(e.message, "these features can't be constrained to each other");
    }

    #[test]
    fn constrain_locked_node_is_refused() {
        let (mut w, holder, post) = holder_and_post();
        Command::SetLocked { id: NodeId(post), locked: true }.apply(&mut w.doc).unwrap();
        let e = w.run(constrain_req(post, ParentArg::Node(holder), "bottom", "bore", None)).err().unwrap();
        assert_eq!(e.code, ErrorCode::Refused);
        assert_eq!(e.message, "node Test post is locked");
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
