//! Move/rotate gizmo for the selected node (M2-PLAN POLA-45).
//!
//! Move: drag the flat ring (or the part itself) on the platform plane; it snaps to holes
//! (Shift free, Ctrl 1 mm steps). A mated node with a slide range gets a vertical arrow that
//! changes `slide` (1 mm steps, Shift free). Rotate: drag the ring about the vertical axis in
//! 15° steps (Shift free). Every pointer move becomes a [`ViewportEvent::TransformRequested`];
//! the app turns it into a command and sends the document back, which moves the gizmo.

use bevy::{
    light::NotShadowCaster,
    math::{DQuat, DVec2, DVec3},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use lab_catalog::Catalog;
use lab_core::part::Frame;
use lab_core::{Attach, Document, Node, NodeId, Parent, SelectionTarget, Shape, compute_holes};

use crate::camera::{OrbitCamera, PointerState, camera_at, handle_input, to_camera_space};
use crate::host::{EventSender, PendingRequests, SettleFrames, ViewportSize};
use crate::picking::{PickId, Selection};
use crate::scene::{SceneIndex, from_render, part_mesh, to_render};
use crate::{CatalogRes, FrameRequest, PointerButton, Tool, ViewportEvent, ViewportInput};

const ROTATE_STEP: f64 = std::f64::consts::PI / 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub(crate) enum GizmoHandle {
    /// Move on the platform (or floor) plane.
    Plane,
    /// Slide along the parent connector's axis.
    Slide,
    /// Turn about the vertical axis.
    Ring,
}

/// Set while a pointer press belongs to the gizmo, so the camera neither orbits nor clicks.
/// Set by `gizmo_input` on press, cleared by `handle_input` on release.
#[derive(Resource, Default)]
pub(crate) struct GizmoCapture(pub bool);

/// A drag in progress. Everything is captured at the press, so document updates sent back
/// during the drag do not move the reference.
#[derive(Clone, Debug)]
pub(crate) struct Drag {
    pub handle: GizmoHandle,
    pub start: Node,
    /// Root frame (platform top, or room) in the room frame: origin and rotation about +Z.
    pub root: (DVec3, f64),
    /// Where the press hit, in room mm.
    pub grab: DVec3,
    /// Room-frame point the node turns about and whose xy snaps to holes.
    pub anchor: DVec3,
    last: Option<(Attach, DVec3, DQuat)>,
}

#[derive(Resource, Default)]
struct Gizmo {
    tool: Tool,
    root: Option<Entity>,
    /// The node the spawned gizmo belongs to.
    shown: Option<NodeId>,
    drag: Option<Drag>,
    materials: Option<(Handle<StandardMaterial>, Handle<StandardMaterial>)>,
    /// Handle radius in render units, for the on-top outline.
    radius: f32,
}

/// Line gizmos drawn without depth test, so handles inside large parts stay visible (POLA-45).
#[derive(Default, Reflect, GizmoConfigGroup)]
struct OnTop;

pub(crate) struct GizmoPlugin;

impl Plugin for GizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Gizmo>()
            .init_resource::<GizmoCapture>()
            .init_gizmo_group::<OnTop>()
            .add_systems(Startup, |mut store: ResMut<GizmoConfigStore>| store.config_mut::<OnTop>().0.depth_bias = -1.0)
            .add_systems(
                Update,
                (gizmo_input.before(handle_input), update_gizmo.after(crate::picking::pick), draw_on_top.after(update_gizmo)),
            );
    }
}

/// The handles node `node` gets for `tool`.
pub(crate) fn handles(doc: &Document, catalog: &Catalog, node: &Node, tool: Tool) -> Vec<GizmoHandle> {
    if node.locked {
        return Vec::new();
    }
    let mate = mate_frames(doc, catalog, node);
    match tool {
        Tool::Select | Tool::Measure => Vec::new(),
        Tool::Move => match node.parent {
            Parent::Node(_) => match (&node.attach, mate) {
                (Attach::Mate { .. }, Some((parent, _, range))) if range.0 < range.1 && parent.z.length() > 0.0 => {
                    vec![GizmoHandle::Slide]
                }
                _ => Vec::new(),
            },
            _ => vec![GizmoHandle::Plane],
        },
        Tool::Rotate => match node.parent {
            Parent::Node(id) => match mate {
                Some((parent, _, _)) if vertical(doc, id, parent.z) => vec![GizmoHandle::Ring],
                _ => Vec::new(),
            },
            _ => vec![GizmoHandle::Ring],
        },
    }
}

/// Parent connector frame (in the parent node's part frame), child connector frame and the slide
/// range, for a mated node.
fn mate_frames(doc: &Document, catalog: &Catalog, node: &Node) -> Option<(Frame, Frame, (f64, f64))> {
    let (Parent::Node(pid), Attach::Mate { self_conn, parent_conn, .. }) = (&node.parent, &node.attach) else {
        return None;
    };
    let parent_def = catalog.get(&doc.node(*pid)?.part)?;
    let pc = parent_def.connector(parent_conn)?;
    let cc = catalog.get(&node.part)?.connector(self_conn)?;
    Some((pc.frame, cc.frame, lab_mates::slide_range(&pc.kind)))
}

/// Whether direction `z` of node `id`'s part frame is vertical in the room.
fn vertical(doc: &Document, id: NodeId, z: DVec3) -> bool {
    doc.node_pose(id).is_some_and(|(_, rot)| (rot * z).normalize_or_zero().z.abs() > 0.9)
}

fn root_frame(doc: &Document, id: NodeId) -> (DVec3, f64) {
    doc.root_platform(id).and_then(|p| doc.platform_pose(p)).unwrap_or((DVec3::ZERO, 0.0))
}

/// The node's anchor in its root frame: the seat connector of a part, else its origin.
fn local_anchor(catalog: &Catalog, node: &Node) -> DVec3 {
    let seat = (node.shape == Shape::Part)
        .then(|| catalog.get(&node.part).and_then(|d| d.connector("seat")).map(|c| c.frame.origin))
        .flatten();
    node.position + node.rotation * seat.unwrap_or(DVec3::ZERO)
}

/// Room-frame anchor of `node` (for mated nodes: their origin).
fn room_anchor(doc: &Document, catalog: &Catalog, node: &Node) -> Option<DVec3> {
    let (origin, angle) = root_frame(doc, node.id);
    let local = match node.parent {
        Parent::Node(_) => doc.node_pose(node.id)?.0,
        _ => local_anchor(catalog, node),
    };
    Some(origin + DQuat::from_rotation_z(angle) * local)
}

impl Drag {
    pub(crate) fn new(doc: &Document, catalog: &Catalog, node: &Node, handle: GizmoHandle, grab: DVec3) -> Option<Drag> {
        Some(Drag {
            handle,
            start: node.clone(),
            root: root_frame(doc, node.id),
            grab,
            anchor: room_anchor(doc, catalog, node)?,
            last: None,
        })
    }

    /// The node's new attach and placement for the pointer ray (room mm), or `None` when the ray
    /// gives no usable point.
    pub(crate) fn update(
        &self,
        doc: &Document,
        catalog: &Catalog,
        ray: (DVec3, DVec3),
        shift: bool,
        ctrl: bool,
    ) -> Option<(Attach, DVec3, DQuat)> {
        let start = &self.start;
        let turn = DQuat::from_rotation_z(self.root.1);
        match self.handle {
            GizmoHandle::Plane => {
                let p = plane_point(ray, self.grab.z)?;
                let delta = turn.inverse() * (p - self.grab);
                let from = local_anchor(catalog, start);
                let mut to = (from + delta).truncate();
                let mut attach = Attach::Free;
                if ctrl {
                    to = to.round();
                } else if !shift
                    && let Parent::Platform(pid) = start.parent
                    && let Some(holes) = doc.platform(pid).and_then(|p| compute_holes(p).ok())
                    && let Some(hit) = lab_mates::snap(&holes, to, holes.pitch.max_element())
                {
                    to = hit.position;
                    attach = Attach::Hole { hole: hit.hole };
                }
                if matches!(start.parent, Parent::Room) {
                    attach = Attach::Free;
                }
                let position = start.position + (to - from.truncate()).extend(0.0);
                Some((attach, position, start.rotation))
            }
            GizmoHandle::Slide => {
                let Attach::Mate { slide, .. } = &start.attach else { return None };
                let (parent, _, (lo, hi)) = mate_frames(doc, catalog, start)?;
                let Parent::Node(pid) = start.parent else { return None };
                let parent_rot = doc.node_pose(pid)?.1;
                let axis = (turn * parent_rot * parent.z).normalize_or_zero();
                let t = line_param(self.grab, axis, ray)?;
                let raw = slide + t;
                let new_slide = if shift { raw } else { raw.round() }.clamp(lo, hi);
                let mut attach = start.attach.clone();
                if let Attach::Mate { slide, .. } = &mut attach {
                    *slide = new_slide;
                }
                let position = start.position + parent.z.normalize_or_zero() * (new_slide - slide);
                Some((attach, position, start.rotation))
            }
            GizmoHandle::Ring => {
                let p = plane_point(ray, self.grab.z)?;
                let a0 = (self.grab - self.anchor).truncate();
                let a1 = (p - self.anchor).truncate();
                if a0.length() < 1e-6 || a1.length() < 1e-6 {
                    return None;
                }
                let raw = a0.angle_to(a1);
                let angle = if shift { raw } else { (raw / ROTATE_STEP).round() * ROTATE_STEP };
                match &start.attach {
                    Attach::Mate { self_conn, parent_conn, slide, twist } => {
                        let (parent, child, _) = mate_frames(doc, catalog, start)?;
                        let Parent::Node(pid) = start.parent else { return None };
                        let up = (turn * doc.node_pose(pid)?.1 * parent.z).z.signum();
                        let twist = twist + angle * up;
                        let (position, rotation) = lab_mates::mate_pose(&parent, &child, *slide, twist);
                        let attach = Attach::Mate {
                            self_conn: self_conn.clone(),
                            parent_conn: parent_conn.clone(),
                            slide: *slide,
                            twist,
                        };
                        Some((attach, position, rotation))
                    }
                    attach => {
                        let spin = DQuat::from_rotation_z(angle);
                        let pivot = local_anchor(catalog, start);
                        let position = pivot + spin * (start.position - pivot);
                        Some((attach.clone(), position, (spin * start.rotation).normalize()))
                    }
                }
            }
        }
    }
}

/// Where `ray` (origin, direction; room mm) meets the horizontal plane at height `z`.
fn plane_point((o, d): (DVec3, DVec3), z: f64) -> Option<DVec3> {
    if d.z.abs() < 1e-9 {
        return None;
    }
    let t = (z - o.z) / d.z;
    (t > 0.0).then(|| o + d * t)
}

/// Parameter along the line `p + t·axis` of the point closest to `ray`.
fn line_param(p: DVec3, axis: DVec3, (o, d): (DVec3, DVec3)) -> Option<f64> {
    let d = d.normalize_or_zero();
    let w = p - o;
    let (b, dd, e) = (axis.dot(d), d.dot(w), axis.dot(w));
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-6 {
        return None;
    }
    Some((b * dd - e) / denom)
}

fn render_ray(ray: Ray3d) -> (DVec3, DVec3) {
    let d = ray.direction;
    (from_render(ray.origin), DVec3::new(d.x as f64, -d.z as f64, d.y as f64))
}

#[allow(clippy::too_many_arguments)]
fn gizmo_input(
    pending: Res<PendingRequests>,
    cameras: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    pointer: Res<PointerState>,
    handles_q: Query<&GizmoHandle>,
    picks: Query<&PickId>,
    mut ray_cast: MeshRayCast,
    mut gizmo: ResMut<Gizmo>,
    mut capture: ResMut<GizmoCapture>,
    (index, catalog, events, full): (
        Res<SceneIndex>,
        Res<CatalogRes>,
        Res<EventSender>,
        Res<ViewportSize>,
    ),
) {
    for request in &pending.0 {
        match request {
            FrameRequest::SetTool(tool) => gizmo.tool = *tool,
            FrameRequest::Input(input) => {
                let Some(doc) = &index.snapshot else { continue };
                let anchor = match *input {
                    ViewportInput::PointerDown { pos, .. } => pos,
                    _ => pointer.press_start().unwrap_or_default(),
                };
                let Some(((), camera, camera_transform, _)) = camera_at(cameras.iter().map(|(c, t)| ((), c, t)), &full, anchor) else { continue };
                let ray_at = |pos: Vec2| camera.viewport_to_world(camera_transform, to_camera_space(camera, &full, pos)).ok();
                match *input {
                    ViewportInput::PointerDown { button: PointerButton::Left, pos, .. } => {
                        let Some(id) = gizmo.shown else { continue };
                        let Some(node) = doc.node(id) else { continue };
                        let Some(ray) = ray_at(pos) else { continue };
                        let available = handles(doc, &catalog.0, node, gizmo.tool);
                        // Handles, and in Move mode the selected part itself (as the plane handle).
                        let filter = |e: Entity| {
                            handles_q.contains(e)
                                || (available.contains(&GizmoHandle::Plane)
                                    && picks.get(e).is_ok_and(|p| p.0 == SelectionTarget::Node(id)))
                        };
                        let settings = MeshRayCastSettings::default().with_filter(&filter);
                        let Some((entity, hit)) = ray_cast.cast_ray(ray, &settings).first().cloned() else { continue };
                        let handle = handles_q.get(entity).copied().unwrap_or(GizmoHandle::Plane);
                        if let Some(drag) = Drag::new(doc, &catalog.0, node, handle, from_render(hit.point)) {
                            gizmo.drag = Some(drag);
                            capture.0 = true;
                        }
                    }
                    ViewportInput::PointerMove { pos, mods } => {
                        let Some(drag) = &gizmo.drag else { continue };
                        let Some(ray) = ray_at(pos) else { continue };
                        let Some(result) = drag.update(doc, &catalog.0, render_ray(ray), mods.shift, mods.ctrl) else {
                            continue;
                        };
                        if drag.last.as_ref() != Some(&result) {
                            let (attach, position, rotation) = result.clone();
                            events.send(ViewportEvent::TransformRequested {
                                id: drag.start.id,
                                attach,
                                position,
                                rotation,
                                done: false,
                            });
                            if let Some(drag) = gizmo.drag.as_mut() {
                                drag.last = Some(result);
                            }
                        }
                    }
                    ViewportInput::PointerUp { button: PointerButton::Left, .. } => {
                        let Some(drag) = gizmo.drag.take() else { continue };
                        let s = &drag.start;
                        let (attach, position, rotation) =
                            drag.last.unwrap_or((s.attach.clone(), s.position, s.rotation));
                        events.send(ViewportEvent::TransformRequested { id: s.id, attach, position, rotation, done: true });
                    }
                    _ => {}
                }
            }
            FrameRequest::LoadDocument(_) => gizmo.drag = None,
            _ => {}
        }
    }
}

/// Rebuilds the gizmo when the tool, the selection or the document changed.
#[allow(clippy::too_many_arguments)]
fn update_gizmo(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    selection: Res<Selection>,
    mut gizmo: ResMut<Gizmo>,
    (index, catalog): (Res<SceneIndex>, Res<CatalogRes>),
    (mut meshes, mut materials, mut settle): (ResMut<Assets<Mesh>>, ResMut<Assets<StandardMaterial>>, ResMut<SettleFrames>),
) {
    let changed = selection.is_changed()
        || pending.0.iter().any(|r| {
            matches!(r, FrameRequest::SetTool(_) | FrameRequest::LoadDocument(_) | FrameRequest::ApplyChanges(..))
        });
    if !changed {
        return;
    }
    if let Some(root) = gizmo.root.take() {
        commands.entity(root).try_despawn();
    }
    gizmo.shown = None;
    let Some(SelectionTarget::Node(id)) = selection.selected else { return };
    let Some(doc) = &index.snapshot else { return };
    let Some(node) = doc.node(id) else { return };
    let available = handles(doc, &catalog.0, node, gizmo.tool);
    gizmo.shown = Some(id);
    if available.is_empty() {
        return;
    }
    let Some(anchor) = room_anchor(doc, &catalog.0, node) else { return };
    let radius = gizmo_radius(&catalog.0, node);

    let (strong, faint) = gizmo
        .materials
        .get_or_insert_with(|| {
            let make = |alpha: f32| StandardMaterial {
                base_color: Color::srgba(10.0 / 255.0, 132.0 / 255.0, 1.0, alpha),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            };
            (materials.add(make(0.85)), materials.add(make(0.3)))
        })
        .clone();
    let root = commands.spawn((Transform::from_translation(to_render(anchor)), Visibility::Inherited)).id();
    let r = radius as f32 * crate::scene::MM;
    gizmo.radius = r;
    for handle in available {
        let (mesh, material, transform) = match handle {
            GizmoHandle::Plane => (
                meshes.add(Annulus::new(r * 0.8, r * 1.2)),
                faint.clone(),
                Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_translation(Vec3::Y * 0.0005),
            ),
            GizmoHandle::Ring => (meshes.add(Torus::new(r * 1.1, r * 1.3)), strong.clone(), Transform::default()),
            GizmoHandle::Slide => {
                let axis = slide_axis(doc, &catalog.0, node).unwrap_or(Vec3::Y);
                let length = r * 1.5;
                (
                    meshes.add(Cylinder::new(r * 0.12, length)),
                    strong.clone(),
                    Transform::from_rotation(Quat::from_rotation_arc(Vec3::Y, axis))
                        .with_translation(axis * (length * 0.5 + r * 0.3)),
                )
            }
        };
        commands.spawn((ChildOf(root), handle, Mesh3d(mesh), MeshMaterial3d(material), NotShadowCaster, transform));
    }
    gizmo.root = Some(root);
    settle.0 = settle.0.max(2);
}

/// Outlines of the spawned handles, drawn on top of everything.
fn draw_on_top(gizmo: Res<Gizmo>, handles: Query<(&GizmoHandle, &GlobalTransform)>, mut lines: Gizmos<OnTop>) {
    if gizmo.root.is_none() {
        return;
    }
    let color = Color::srgb(10.0 / 255.0, 132.0 / 255.0, 1.0);
    let r = gizmo.radius;
    let flat = |g: &GlobalTransform| Isometry3d::new(g.translation(), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    for (handle, global) in &handles {
        match handle {
            GizmoHandle::Plane => {
                lines.circle(flat(global), r * 0.8, color);
                lines.circle(flat(global), r * 1.2, color);
            }
            GizmoHandle::Ring => {
                lines.circle(flat(global), r * 1.1, color);
                lines.circle(flat(global), r * 1.3, color);
            }
            GizmoHandle::Slide => {
                let half = Vec3::Y * 0.75 * r / global.scale().y.max(1e-6);
                lines.line(global.transform_point(-half), global.transform_point(half), color);
            }
        }
    }
}

/// Render-space direction of a mated node's slide axis.
fn slide_axis(doc: &Document, catalog: &Catalog, node: &Node) -> Option<Vec3> {
    let (parent, _, _) = mate_frames(doc, catalog, node)?;
    let Parent::Node(pid) = node.parent else { return None };
    let (_, angle) = root_frame(doc, node.id);
    let dir = DQuat::from_rotation_z(angle) * doc.node_pose(pid)?.1 * parent.z;
    Some(crate::scene::normal_to_render(dir.normalize_or_zero()))
}

/// Gizmo ring radius in mm: a little outside the node's footprint.
fn gizmo_radius(catalog: &Catalog, node: &Node) -> f64 {
    let footprint = match &node.shape {
        Shape::Part => part_mesh(catalog, &node.part, &node.params)
            .map(|m| DVec2::new(m.max.x - m.min.x, m.max.y - m.min.y).length() * 0.5)
            .unwrap_or(25.0),
        Shape::Box { size } => size.truncate().length() * 0.5,
        Shape::Cylinder { radius, .. } => *radius,
    };
    footprint.max(20.0) * 1.2
}

#[cfg(test)]
mod tests {
    use lab_core::part::PartDef;

    use super::*;

    fn def(json: serde_json::Value) -> PartDef {
        serde_json::from_value(json).unwrap()
    }

    fn catalog() -> Catalog {
        Catalog::from_defs(vec![
            def(serde_json::json!({
                "id": "t:PH50/M", "name": "PH50/M", "category": "posts/holders",
                "geometry": { "kind": "parametric", "generator": "post_holder" },
                "connectors": [
                    { "name": "seat", "kind": "hole_grid_seat", "frame": { "origin": [0,0,0], "z": [0,0,-1], "x": [1,0,0] } },
                    { "name": "bore", "kind": "bore", "diameter": 12.7, "gender": "female", "range": [-45.0, -5.0],
                      "frame": { "origin": [0,0,50], "z": [0,0,1], "x": [1,0,0] } }
                ]
            })),
            def(serde_json::json!({
                "id": "t:TR50/M", "name": "TR50/M", "category": "posts/posts",
                "geometry": { "kind": "parametric", "generator": "post" },
                "connectors": [
                    { "name": "bottom", "kind": "bore", "diameter": 12.7, "gender": "male",
                      "frame": { "origin": [0,0,0], "z": [0,0,-1], "x": [1,0,0] } }
                ]
            })),
        ])
    }

    fn node(id: u64, part: &str, parent: Parent, attach: Attach, position: DVec3) -> Node {
        Node {
            id: NodeId(id),
            name: part.into(),
            part: part.into(),
            parent,
            attach,
            position,
            rotation: DQuat::IDENTITY,
            shape: Shape::Part,
            finish: lab_core::Finish::AnodizedBlack,
            params: Default::default(),
            pickable: true,
            locked: false,
        }
    }

    /// A ray straight down onto room point `p`.
    fn down(p: DVec3) -> (DVec3, DVec3) {
        (p + DVec3::Z * 1000.0, -DVec3::Z)
    }

    /// Holder moves hole to hole (snapped), rotates in 15° steps, and the mated post slides.
    #[test]
    fn move_rotate_slide() {
        let mut doc = lab_core::sample::demo_document();
        let catalog = catalog();
        let pid = doc.platforms[0].id;
        let holes = compute_holes(doc.platform(pid).unwrap()).unwrap();
        let h0 = holes.nearest(DVec2::ZERO).unwrap();
        let p0 = holes.position(h0);
        let hid = doc.next_free_id();
        doc.nodes.push(node(hid, "t:PH50/M", Parent::Platform(pid), Attach::Hole { hole: h0 }, p0.extend(0.0)));
        let holder = doc.node(NodeId(hid)).unwrap().clone();
        let (origin, angle) = doc.platform_pose(pid).unwrap();
        let room = |local: DVec3| origin + DQuat::from_rotation_z(angle) * local;

        // Move: grab at the holder, release 1.3 pitches along +X → the next-but-one hole.
        let tools = handles(&doc, &catalog, &holder, Tool::Move);
        assert_eq!(tools, vec![GizmoHandle::Plane]);
        let drag = Drag::new(&doc, &catalog, &holder, GizmoHandle::Plane, room(p0.extend(0.0))).unwrap();
        let target = p0 + DVec2::new(holes.pitch.x * 1.3, 0.0);
        let (attach, position, _) = drag.update(&doc, &catalog, down(room(target.extend(0.0))), false, false).unwrap();
        let Attach::Hole { hole } = attach else { panic!("{attach:?}") };
        assert_ne!(hole, h0);
        assert!((position.truncate() - holes.position(hole)).length() < 1e-9);
        assert!((position.x - (p0.x + holes.pitch.x)).abs() < 1e-9);
        // Shift: free, exactly under the pointer.
        let (attach, position, _) = drag.update(&doc, &catalog, down(room(target.extend(0.0))), true, false).unwrap();
        assert_eq!(attach, Attach::Free);
        assert!((position.truncate() - target).length() < 1e-6);

        // Rotate: a pointer swept by 50° gives 45°.
        let drag = Drag::new(&doc, &catalog, &holder, GizmoHandle::Ring, room(p0.extend(0.0) + DVec3::X * 40.0)).unwrap();
        let swept = p0.extend(0.0) + DQuat::from_rotation_z(50f64.to_radians()) * DVec3::X * 40.0;
        let (_, position, rotation) = drag.update(&doc, &catalog, down(room(swept)), false, false).unwrap();
        let (yaw, _, _) = rotation.to_euler(EulerRot::ZYX);
        assert!((yaw.to_degrees() - 45.0).abs() < 1e-6, "{}", yaw.to_degrees());
        assert!((position - holder.position).length() < 1e-9);

        // Slide: a post mated at slide −5 dragged 10.4 mm down → −15.
        let post = node(
            hid + 1,
            "t:TR50/M",
            Parent::Node(NodeId(hid)),
            Attach::Mate { self_conn: "bottom".into(), parent_conn: "bore".into(), slide: -5.0, twist: 0.0 },
            DVec3::new(0.0, 0.0, 45.0),
        );
        doc.nodes.push(post.clone());
        assert_eq!(handles(&doc, &catalog, &post, Tool::Move), vec![GizmoHandle::Slide]);
        let grab = room(p0.extend(60.0));
        let drag = Drag::new(&doc, &catalog, &post, GizmoHandle::Slide, grab).unwrap();
        // A horizontal ray passing 10.4 mm below the grab point.
        let ray = (grab - DVec3::Z * 10.4 - DVec3::Y * 500.0, DVec3::Y);
        let (attach, position, _) = drag.update(&doc, &catalog, ray, false, false).unwrap();
        assert!(matches!(attach, Attach::Mate { slide, .. } if slide == -15.0), "{attach:?}");
        assert!((position - DVec3::new(0.0, 0.0, 35.0)).length() < 1e-9);
    }
}
