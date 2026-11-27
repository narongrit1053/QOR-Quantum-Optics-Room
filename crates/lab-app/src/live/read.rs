//! Answers the read commands as JSON, without touching the document. Millimetres and degrees.

use std::path::PathBuf;

use lab_core::{Attach, HoleRef, Node, NodeId, Parent, PlatformId};
use serde_json::{json, Value};

use super::features::node_features;
use super::protocol::{ErrorCode, LiveError, ReadRequest};
use super::resolve::{quat_to_euler_deg, Ctx};

pub struct StatusInfo {
    pub version: String,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub undo_label: Option<String>,
}

fn err(code: ErrorCode, message: impl Into<String>) -> LiveError {
    LiveError::new(code, message)
}

/// Same spelling as `ParentArg`: `"room"`, `{"platform":1}`, `{"node":5}`.
fn parent_json(p: &Parent) -> Value {
    match p {
        Parent::Room => json!("room"),
        Parent::Platform(id) => json!({"platform": id.0}),
        Parent::Node(id) => json!({"node": id.0}),
    }
}

fn attach_name(a: &Attach) -> &'static str {
    match a {
        Attach::Free => "free",
        Attach::Hole { .. } => "hole",
        Attach::Mate { .. } => "mate",
        Attach::Constrained { .. } => "constrained",
        Attach::Joint(_) => "joint",
    }
}

fn list_item(n: &Node, ctx: &Ctx) -> Value {
    let (vendor, number) =
        ctx.catalog.get(&n.part).map_or((String::new(), n.part.clone()), |d| (d.vendor.clone(), d.part_number.clone()));
    json!({
        "id": n.id.0, "name": n.name, "part": n.part, "vendor": vendor, "part_number": number,
        "parent": parent_json(&n.parent), "attach": attach_name(&n.attach),
        "position": [n.position.x, n.position.y, n.position.z],
        "rotation": quat_to_euler_deg(n.rotation), "locked": n.locked,
    })
}

fn holes(platform: u64, ctx: &Ctx) -> Result<Value, LiveError> {
    let id = PlatformId(platform);
    ctx.doc.platform(id).ok_or_else(|| err(ErrorCode::NotFound, format!("no platform {platform}")))?;
    let hs = (ctx.holes)(id).ok_or_else(|| err(ErrorCode::NotFound, format!("platform {platform} has no hole grid")))?;
    let last = HoleRef { col: hs.cols.saturating_sub(1), row: hs.rows.saturating_sub(1) };
    let occupied: Vec<Value> = ctx
        .doc
        .nodes
        .iter()
        .filter(|n| n.parent == Parent::Platform(id))
        .filter_map(|n| match &n.attach {
            Attach::Hole { hole } => hs.label(*hole).map(|l| json!({"hole": l.to_string(), "node": n.id.0})),
            _ => None,
        })
        .collect();
    Ok(json!({
        "pitch": [hs.pitch.x, hs.pitch.y],
        "first": HoleRef { col: 0, row: 0 }.to_string(),
        "last": last.to_string(),
        "thread_radius": hs.thread_radius,
        "occupied": occupied,
    }))
}

pub fn read(req: &ReadRequest, ctx: &Ctx, status: &StatusInfo) -> Result<Value, LiveError> {
    Ok(match req {
        ReadRequest::Status => json!({
            "version": status.version, "title": status.title, "path": status.path,
            "dirty": status.dirty, "undo": status.undo_label,
        }),
        ReadRequest::List { platform } => Value::Array(
            ctx.doc
                .nodes
                .iter()
                .filter(|n| platform.is_none_or(|p| ctx.doc.root_platform(n.id) == Some(PlatformId(p))))
                .map(|n| list_item(n, ctx))
                .collect(),
        ),
        ReadRequest::Platforms => Value::Array(
            ctx.doc
                .platforms
                .iter()
                .map(|p| {
                    let (lo, hi) = p.outline.bounds();
                    json!({
                        "id": p.id.0, "name": p.name, "kind": p.kind, "top_height": p.top_height,
                        "bounds": [[lo.x, lo.y], [hi.x, hi.y]], "hole_grid": p.hole_grid.is_some(),
                    })
                })
                .collect(),
        ),
        ReadRequest::Holes { platform } => holes(*platform, ctx)?,
        ReadRequest::Part { id } => {
            let d = ctx.catalog.get(id).ok_or_else(|| err(ErrorCode::NotFound, format!("no part {id} in the catalog")))?;
            let connectors: Vec<Value> =
                d.connectors.iter().map(|c| json!({"name": c.name, "kind": c.kind, "frame": c.frame})).collect();
            json!({
                "id": d.id, "name": d.name, "vendor": d.vendor, "part_number": d.part_number,
                "category": d.category, "connectors": connectors,
            })
        }
        ReadRequest::Catalog { search, limit } => Value::Array(
            ctx.catalog
                .search(search, limit.unwrap_or(50))
                .into_iter()
                .map(|d| json!({"id": d.id, "vendor": d.vendor, "part_number": d.part_number, "name": d.name}))
                .collect(),
        ),
        ReadRequest::Features { node } => {
            let n = ctx.doc.node(NodeId(*node)).ok_or_else(|| err(ErrorCode::NotFound, format!("no node {node}")))?;
            let d = ctx.catalog.get(&n.part).ok_or_else(|| {
                err(ErrorCode::NotFound, format!("part {} of node {node} is not in the catalog", n.part))
            })?;
            Value::Array(
                node_features(n, d, ctx.features)
                    .into_iter()
                    .map(|f| json!({"id": f.id, "source": f.source, "description": f.description, "feature": f.feature}))
                    .collect(),
            )
        }
        ReadRequest::Beams => Value::Array(
            ctx.trace
                .segments
                .iter()
                .map(|s| {
                    let pol = s.polarization();
                    json!({
                        "start": [s.start.x, s.start.y, s.start.z], "end": [s.end.x, s.end.y, s.end.z],
                        "hits": s.hit.as_ref().map(|(n, _)| n.0), "radius": [s.w_start, s.w_end],
                        "power": s.power, "polarization": pol.label(),
                        "azimuth_deg": pol.azimuth_deg, "ellipticity_deg": pol.ellipticity_deg,
                    })
                })
                .collect(),
        ),
        ReadRequest::Bundles => Value::Array(
            ctx.bundles
                .iter()
                .map(|b| {
                    let parts: Vec<&str> = b.nodes.iter().map(|n| n.part.as_str()).collect();
                    json!({"name": b.name, "group": b.group, "anchor": parts.first(), "parts": parts})
                })
                .collect(),
        ),
        ReadRequest::Snapshot { .. } => return Err(err(ErrorCode::BadRequest, "snapshot is handled by the app")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::features::FeatureLookup;
    use lab_catalog::Catalog;
    use lab_core::glam::{DQuat, DVec3};
    use lab_core::part::{Feature, PartDef};
    use lab_core::{Command, Document, HoleSet};
    use std::rc::Rc;

    struct NoFeatures;
    impl FeatureLookup for NoFeatures {
        fn detected(&self, _: &Node, _: &PartDef) -> Vec<Feature> {
            vec![]
        }
    }

    fn status() -> StatusInfo {
        StatusInfo { version: "1".into(), title: "T".into(), path: None, dirty: true, undo_label: Some("Agent: x".into()) }
    }

    struct World {
        doc: Document,
        catalog: Catalog,
        holes: Rc<HoleSet>,
        trace: lab_optics::Trace,
    }

    impl World {
        fn new() -> Self {
            let doc = lab_core::sample::demo_document();
            let holes = Rc::new(lab_core::compute_holes(&doc.platforms[0]).unwrap());
            World { doc, catalog: Catalog::load_builtin().0, holes, trace: Default::default() }
        }
        fn read(&self, req: ReadRequest) -> Result<Value, LiveError> {
            let holes = |_: PlatformId| Some(self.holes.clone());
            let ctx = Ctx { doc: &self.doc, catalog: &self.catalog, holes: &holes, features: &NoFeatures, trace: &self.trace, bundles: &[] };
            read(&req, &ctx, &status())
        }
        fn table(&self) -> u64 {
            self.doc.platforms[0].id.0
        }
    }

    #[test]
    fn status_shape() {
        let v = World::new().read(ReadRequest::Status).unwrap();
        assert_eq!(v, json!({"version": "1", "title": "T", "path": null, "dirty": true, "undo": "Agent: x"}));
    }

    #[test]
    fn list_has_every_node() {
        let w = World::new();
        let v = w.read(ReadRequest::List { platform: None }).unwrap();
        let items = v.as_array().unwrap();
        assert_eq!(items.len(), w.doc.nodes.len());
        assert_eq!(items[0]["id"], w.doc.nodes[0].id.0);
        for k in ["name", "part", "vendor", "part_number", "parent", "attach", "position", "rotation", "locked"] {
            assert!(items[0].get(k).is_some(), "missing {k}");
        }
        assert_eq!(items[0]["rotation"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn list_filters_by_platform() {
        let w = World::new();
        let all = w.read(ReadRequest::List { platform: None }).unwrap().as_array().unwrap().len();
        let on = w.read(ReadRequest::List { platform: Some(w.table()) }).unwrap();
        let want = w.doc.nodes_on(w.doc.platforms[0].id).count();
        assert_eq!(on.as_array().unwrap().len(), want);
        assert!(want > 0 && want <= all);
        assert!(w.read(ReadRequest::List { platform: Some(9999) }).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn platforms_shape() {
        let w = World::new();
        let v = w.read(ReadRequest::Platforms).unwrap();
        let p = &v[0];
        assert_eq!(p["id"], w.table());
        assert_eq!(p["hole_grid"], true);
        assert_eq!(p["bounds"].as_array().unwrap().len(), 2);
        assert!(p["kind"].is_string() && p["top_height"].is_number());
    }

    #[test]
    fn holes_reports_occupied() {
        let mut w = World::new();
        let part = w.catalog.all()[0].id.clone();
        let idx = w.holes.index_of("C14".parse().unwrap());
        let id = w.doc.next_free_id();
        let table = w.doc.platforms[0].id;
        let node = crate::live::resolve::new_part_node(
            &w.catalog,
            &part,
            id,
            Parent::Platform(table),
            Attach::Hole { hole: idx },
            DVec3::ZERO,
            DQuat::IDENTITY,
        )
        .unwrap();
        Command::AddNode { node, index: None }.apply(&mut w.doc).unwrap();
        let v = w.read(ReadRequest::Holes { platform: w.table() }).unwrap();
        assert_eq!(v["first"], "A1");
        assert_eq!(v["occupied"], json!([{"hole": "C14", "node": id}]));
        assert_eq!(v["pitch"], json!([w.holes.pitch.x, w.holes.pitch.y]));
        assert!(v["last"].as_str().unwrap().len() >= 2);
        assert_eq!(w.read(ReadRequest::Holes { platform: 9999 }).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn part_unknown_is_not_found() {
        let w = World::new();
        assert_eq!(w.read(ReadRequest::Part { id: "nope:none".into() }).unwrap_err().code, ErrorCode::NotFound);
        let id = w.catalog.all().into_iter().find(|d| !d.connectors.is_empty()).unwrap().id.clone();
        let v = w.read(ReadRequest::Part { id: id.clone() }).unwrap();
        assert_eq!(v["id"], id);
        let c = &v["connectors"][0];
        assert!(c["name"].is_string() && c["frame"]["origin"].is_array() && c["frame"]["z"].is_array());
    }

    #[test]
    fn catalog_respects_limit() {
        let w = World::new();
        let v = w.read(ReadRequest::Catalog { search: String::new(), limit: Some(3) }).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 3);
        for k in ["id", "vendor", "part_number", "name"] {
            assert!(v[0].get(k).is_some());
        }
        let d = w.read(ReadRequest::Catalog { search: String::new(), limit: None }).unwrap();
        assert_eq!(d.as_array().unwrap().len(), w.catalog.all().len().min(50));
    }

    #[test]
    fn features_shape() {
        let w = World::new();
        let n = w.doc.nodes.iter().find(|n| w.catalog.get(&n.part).is_some_and(|d| !d.connectors.is_empty())).unwrap();
        let v = w.read(ReadRequest::Features { node: n.id.0 }).unwrap();
        let f = &v[0];
        assert_eq!(f["source"], "connector");
        assert!(f["id"].is_string() && f["description"].is_string());
        assert!(matches!(f["feature"]["type"].as_str(), Some("plane" | "axis")));
        assert_eq!(w.read(ReadRequest::Features { node: 9999 }).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn beams_shape() {
        let mut w = World::new();
        let seg = |hit: Option<(NodeId, String)>| lab_optics::Segment {
            source: NodeId(1),
            start: DVec3::new(0.0, 0.0, 10.0),
            end: DVec3::new(100.0, 0.0, 10.0),
            w_start: 0.5,
            w_end: 0.6,
            waist: None,
            hit,
            blocked: None,
            depth: 0,
            field: lab_optics::Field { re: DVec3::Y, im: DVec3::ZERO },
            power: 1.0,
            q_start: (0.0, 1.0),
            lm2: 1e-3,
            frame_x: DVec3::X,
            path_start: 0.0,
        };
        w.trace.segments = vec![seg(Some((NodeId(7), "in".into()))), seg(None)];
        let v = w.read(ReadRequest::Beams).unwrap();
        assert_eq!(v[0]["hits"], 7);
        assert!(v[1]["hits"].is_null());
        assert_eq!(v[0]["start"], json!([0.0, 0.0, 10.0]));
        assert_eq!(v[0]["end"], json!([100.0, 0.0, 10.0]));
        assert_eq!(v[0]["radius"], json!([0.5, 0.6]));
    }

    #[test]
    fn snapshot_is_bad_request() {
        let r = World::new().read(ReadRequest::Snapshot { path: None, width: None, height: None });
        assert_eq!(r.unwrap_err().code, ErrorCode::BadRequest);
    }
}
