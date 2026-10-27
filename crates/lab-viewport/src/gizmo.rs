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
use lab_core::{Attach, Document, Joint, Node, NodeId, Parent, SelectionTarget, Shape, compute_holes};
use lab_mates::constraints::FreeMotion;
use lab_mates::joints::{Dof, dofs};

use crate::camera::{OrbitCamera, PointerState, camera_at, handle_input, to_camera_space};
use crate::host::{EventSender, PendingRequests, SettleFrames, ViewportSize};
use crate::picking::{PickId, Selection};
use crate::scene::{SceneIndex, from_render, part_mesh, to_render};
use crate::{CatalogRes, DragUnit, FrameRequest, PointerButton, Tool, ViewportEvent, ViewportInput};

const ROTATE_STEP: f64 = std::f64::consts::PI / 12.0;
/// Joint rotation step (M9 E2).
const JOINT_STEP: f64 = std::f64::consts::PI / 36.0;
/// Pointer travel (px) that picks the DOF of a multi-DOF joint drag.
const DOF_PICK_PX: f32 = 6.0;
/// Rotate ring inner and outer radius, in gizmo radii.
const RING: (f32, f32) = (1.05, 1.35);
/// A press this far from the centre (gizmo radii) still grabs the ring, so a near miss does not orbit.
const RING_GRAB: (f64, f64) = (0.85, 1.6);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub(crate) enum GizmoHandle {
    /// Move on the platform (or floor) plane.
    Plane,
    /// Slide along the parent connector's axis.
    Slide,
    /// Turn about the vertical axis.
    Ring,
    /// M9 E2. The jointed part itself: drags along its joint's DOFs.
    Joint,
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
    /// Joint drags: the DOF index being dragged (`None` until picked) and the joint origin frame
    /// in the room at the press.
    pub dof: Option<usize>,
    pub joint: Option<(DVec3, DQuat)>,
    /// Constrained drags: the free motion at the press and the parent frame's room pose.
    free: Option<(FreeMotion, (DVec3, DQuat))>,
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
pub(crate) struct OnTop;

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
    if let Attach::Joint(j) = &node.attach {
        return match tool {
            Tool::Measure | Tool::Constrain | Tool::Joint => Vec::new(),
            _ if j.suppressed || dofs(j.kind).is_empty() => Vec::new(),
            _ => vec![GizmoHandle::Joint],
        };
    }
    // A constrained node moves only along its free degrees of freedom (POLA-160).
    if let Some((free, _)) = constrained_motion(doc, node) {
        return match tool {
            Tool::Move | Tool::Select if free.slides.len() == 1 => vec![GizmoHandle::Slide],
            Tool::Move | Tool::Select if free.slides.len() > 1 => vec![GizmoHandle::Plane],
            Tool::Rotate if free.turn.is_some() => vec![GizmoHandle::Ring],
            _ => Vec::new(),
        };
    }
    let mate = mate_frames(doc, catalog, node);
    match tool {
        Tool::Measure | Tool::Constrain | Tool::Joint => Vec::new(),
        // M9: in Select the part itself is the handle (left-drag moves it hole to hole).
        Tool::Select => vec![GizmoHandle::Plane],
        // A node on a node can also be dragged off sideways: that unmates it (POLA-104).
        Tool::Move => match node.parent {
            Parent::Node(_) => match (&node.attach, mate) {
                (Attach::Mate { .. }, Some((parent, _, range))) if range.0 < range.1 && parent.z.length() > 0.0 => {
                    vec![GizmoHandle::Slide, GizmoHandle::Plane]
                }
                _ => vec![GizmoHandle::Plane],
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
    let parent = doc.node(*pid)?;
    let parent_def = catalog.get(&parent.part)?;
    let range = lab_mates::slide_range(&parent_def.connector(parent_conn)?.kind);
    let pf = lab_mates::connector_frame(parent_def, parent_conn, &parent.params)?;
    let cf = lab_mates::connector_frame(catalog.get(&node.part)?, self_conn, &node.params)?;
    Some((pf, cf, range))
}

/// Free motion of a constrained node in its parent frame, and the parent frame's room pose.
fn constrained_motion(doc: &Document, node: &Node) -> Option<(FreeMotion, (DVec3, DQuat))> {
    let Attach::Constrained { constraints } = &node.attach else { return None };
    let parent = lab_mates::constraints::world_pose(doc, node.parent)?;
    Some((lab_mates::constraints::free_motion((node.position, node.rotation), constraints), parent))
}

/// Normal of the plane a constrained Plane drag moves in (parent frame).
fn slide_normal(free: &FreeMotion) -> DVec3 {
    match free.slides[..] {
        [a, b] => a.cross(b).normalize(),
        _ => DVec3::Z,
    }
}

/// Room-frame centre and axis of the rotate ring.
fn ring_axis(doc: &Document, catalog: &Catalog, node: &Node) -> Option<(DVec3, DVec3)> {
    match constrained_motion(doc, node) {
        Some((free, (pp, pq))) => free.turn.map(|(o, a)| (pp + pq * o, pq * a)),
        None => Some((room_anchor(doc, catalog, node)?, DVec3::Z)),
    }
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

/// Room-frame joint origin frame of `node` (z = joint axis), current slide values included.
pub(crate) fn joint_frame(doc: &Document, node: &Node, j: &Joint) -> Option<(DVec3, DQuat)> {
    let (wp, wq) = lab_mates::constraints::world_pose(doc, node.parent)?;
    let (pp, pq) = lab_mates::joints::frame_pose(&j.parent);
    let r = pq * DQuat::from_rotation_z(j.angle);
    let mut slide = DVec3::ZERO;
    for (d, v) in dofs(j.kind).iter().zip(j.values) {
        match d {
            Dof::SlideX => slide.x = v,
            Dof::SlideY => slide.y = v,
            Dof::SlideZ => slide.z = v,
            _ => {}
        }
    }
    Some((wp + wq * (pp + pq * j.offset + r * slide), wq * r))
}

pub(crate) fn is_rot(d: Dof) -> bool {
    matches!(d, Dof::RotX | Dof::RotY | Dof::RotZ)
}

/// Room direction of a DOF for joint frame rotation `r`.
pub(crate) fn dof_axis(d: Dof, r: DQuat) -> DVec3 {
    r * match d {
        Dof::SlideX | Dof::RotX => DVec3::X,
        Dof::SlideY | Dof::RotY => DVec3::Y,
        Dof::SlideZ | Dof::RotZ => DVec3::Z,
    }
}

/// Signed angle about `axis` (through `o`) from `grab` to where `ray` meets the plane normal to `axis`.
fn plane_angle(o: DVec3, axis: DVec3, grab: DVec3, (ro, rd): (DVec3, DVec3)) -> Option<f64> {
    let denom = rd.dot(axis);
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = (o - ro).dot(axis) / denom;
    if t <= 0.0 {
        return None;
    }
    let flat = |v: DVec3| v - axis * v.dot(axis);
    let (a0, a1) = (flat(grab - o), flat(ro + rd * t - o));
    if a0.length() < 1e-6 || a1.length() < 1e-6 {
        return None;
    }
    Some(axis.dot(a0.cross(a1)).atan2(a0.dot(a1)))
}

impl Drag {
    pub(crate) fn new(doc: &Document, catalog: &Catalog, node: &Node, handle: GizmoHandle, grab: DVec3) -> Option<Drag> {
        let mut start = node.clone();
        let free = constrained_motion(doc, node);
        if handle == GizmoHandle::Plane && matches!(node.parent, Parent::Node(_)) && free.is_none() {
            // Moving a node that sits on a node: it continues free on its root, where it is now.
            let (position, rotation) = doc.node_pose(node.id)?;
            start.parent = doc.root_platform(node.id).map_or(Parent::Room, Parent::Platform);
            start.attach = Attach::Free;
            start.position = position;
            start.rotation = rotation;
        }
        let (dof, joint) = match (&node.attach, handle) {
            (Attach::Joint(j), GizmoHandle::Joint) => {
                ((dofs(j.kind).len() == 1).then_some(0), Some(joint_frame(doc, node, j)?))
            }
            _ => (None, None),
        };
        Some(Drag {
            handle,
            dof,
            joint,
            free,
            anchor: room_anchor(doc, catalog, node)?,
            start,
            root: root_frame(doc, node.id),
            grab,
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
        if self.free.is_some() {
            return self.free_update(ray, shift);
        }
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
                let (parent, _, _) = mate_frames(doc, catalog, start)?;
                let Parent::Node(pid) = start.parent else { return None };
                let parent_rot = doc.node_pose(pid)?.1;
                let axis = (turn * parent_rot * parent.z).normalize_or_zero();
                let t = line_param(self.grab, axis, ray)?;
                let raw = slide + t;
                self.slide_to(doc, catalog, if shift { raw } else { raw.round() })
            }
            GizmoHandle::Ring => {
                let p = plane_point(ray, self.grab.z)?;
                let a0 = (self.grab - self.anchor).truncate();
                let a1 = (p - self.anchor).truncate();
                if a0.length() < 1e-6 || a1.length() < 1e-6 {
                    return None;
                }
                let raw = a0.angle_to(a1);
                self.turn(doc, catalog, if shift { raw } else { (raw / ROTATE_STEP).round() * ROTATE_STEP })
            }
            GizmoHandle::Joint => {
                let (i, (o, r)) = (self.dof?, self.joint?);
                let Attach::Joint(j) = &start.attach else { return None };
                let d = *dofs(j.kind).get(i)?;
                let axis = dof_axis(d, r);
                let delta = if is_rot(d) {
                    let raw = plane_angle(o, axis, self.grab, ray)?;
                    if shift { raw } else { (raw / JOINT_STEP).round() * JOINT_STEP }
                } else {
                    let t = line_param(self.grab, axis, ray)?;
                    if shift { t } else { t.round() }
                };
                self.joint_value(i, j.values[i] + delta)
            }
        }
    }

    /// M9 E2. The result for a typed value: Plane moves `v` mm along the current drag direction
    /// from the start, Ring and joint rotations turn `v` degrees, slides set `v` mm.
    pub(crate) fn exact(&self, doc: &Document, catalog: &Catalog, v: f64) -> Option<(Attach, DVec3, DQuat)> {
        let start = &self.start;
        if let Some((free, _)) = &self.free {
            return match self.handle {
                GizmoHandle::Plane => {
                    let dir = (self.last.as_ref()?.1 - start.position).normalize_or_zero();
                    (dir != DVec3::ZERO).then(|| self.free_slide(dir * v))
                }
                GizmoHandle::Slide => Some(self.free_slide(*free.slides.first()? * v)),
                _ => self.free_turn(v.to_radians()),
            };
        }
        match self.handle {
            GizmoHandle::Plane => {
                let dir = (self.last.as_ref()?.1 - start.position).truncate().normalize_or_zero();
                (dir != DVec2::ZERO).then(|| (Attach::Free, start.position + (dir * v).extend(0.0), start.rotation))
            }
            GizmoHandle::Slide => self.slide_to(doc, catalog, v),
            GizmoHandle::Ring => self.turn(doc, catalog, v.to_radians()),
            GizmoHandle::Joint => {
                let Attach::Joint(j) = &start.attach else { return None };
                let i = self.dof.unwrap_or(0);
                let d = *dofs(j.kind).get(i)?;
                self.joint_value(i, if is_rot(d) { j.values[i] + v.to_radians() } else { v })
            }
        }
    }

    pub(crate) fn unit(&self) -> DragUnit {
        let rot = match (&self.start.attach, self.handle) {
            (_, GizmoHandle::Ring) => true,
            (Attach::Joint(j), GizmoHandle::Joint) => {
                dofs(j.kind).get(self.dof.unwrap_or(0)).is_some_and(|d| is_rot(*d))
            }
            _ => false,
        };
        if rot { DragUnit::Degrees } else { DragUnit::Mm }
    }

    /// Constrained drags, worked in the parent frame: the ray and grab are mapped into it.
    fn free_update(&self, ray: (DVec3, DVec3), shift: bool) -> Option<(Attach, DVec3, DQuat)> {
        let (free, (pp, pq)) = self.free.as_ref()?;
        let inv = pq.inverse();
        let (ro, rd) = (inv * (ray.0 - *pp), inv * ray.1);
        let grab = inv * (self.grab - *pp);
        match self.handle {
            GizmoHandle::Plane => {
                let n = slide_normal(free);
                let denom = rd.dot(n);
                let t = (grab - ro).dot(n) / denom;
                (denom.abs() > 1e-9 && t > 0.0).then(|| self.free_slide(ro + rd * t - grab))
            }
            GizmoHandle::Slide => {
                let axis = *free.slides.first()?;
                let t = line_param(grab, axis, (ro, rd))?;
                Some(self.free_slide(axis * if shift { t } else { t.round() }))
            }
            GizmoHandle::Ring => {
                let (o, axis) = free.turn?;
                let raw = plane_angle(o, axis, grab, (ro, rd))?;
                self.free_turn(if shift { raw } else { (raw / ROTATE_STEP).round() * ROTATE_STEP })
            }
            GizmoHandle::Joint => None,
        }
    }

    fn free_slide(&self, delta: DVec3) -> (Attach, DVec3, DQuat) {
        let s = &self.start;
        (s.attach.clone(), s.position + delta, s.rotation)
    }

    fn free_turn(&self, angle: f64) -> Option<(Attach, DVec3, DQuat)> {
        let (o, axis) = self.free.as_ref()?.0.turn?;
        let (s, r) = (&self.start, DQuat::from_axis_angle(axis, angle));
        Some((s.attach.clone(), o + r * (s.position - o), (r * s.rotation).normalize()))
    }

    fn joint_value(&self, i: usize, v: f64) -> Option<(Attach, DVec3, DQuat)> {
        let Attach::Joint(j) = &self.start.attach else { return None };
        let mut j = j.clone();
        j.values[i] = v;
        lab_mates::joints::clamp(&mut j);
        let (position, rotation) = lab_mates::joints::joint_pose(&j);
        Some((Attach::Joint(j), position, rotation))
    }

    fn slide_to(&self, doc: &Document, catalog: &Catalog, new_slide: f64) -> Option<(Attach, DVec3, DQuat)> {
        let start = &self.start;
        let Attach::Mate { slide, .. } = &start.attach else { return None };
        let (parent, _, (lo, hi)) = mate_frames(doc, catalog, start)?;
        let new_slide = new_slide.clamp(lo, hi);
        let mut attach = start.attach.clone();
        if let Attach::Mate { slide, .. } = &mut attach {
            *slide = new_slide;
        }
        let position = start.position + parent.z.normalize_or_zero() * (new_slide - slide);
        Some((attach, position, start.rotation))
    }

    fn turn(&self, doc: &Document, catalog: &Catalog, angle: f64) -> Option<(Attach, DVec3, DQuat)> {
        let start = &self.start;
        let turn = DQuat::from_rotation_z(self.root.1);
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

/// Where `ray` (origin, direction; room mm) meets the horizontal plane at height `z`.
fn plane_point((o, d): (DVec3, DVec3), z: f64) -> Option<DVec3> {
    if d.z.abs() < 1e-9 {
        return None;
    }
    let t = (z - o.z) / d.z;
    (t > 0.0).then(|| o + d * t)
}

/// Where a press ray meets the ring's plane (through `centre`, normal `axis`), when that is
/// within the ring's grab band.
fn ring_grab(centre: DVec3, axis: DVec3, radius: f64, (o, d): (DVec3, DVec3)) -> Option<DVec3> {
    let denom = d.dot(axis);
    let t = (centre - o).dot(axis) / denom;
    if denom.abs() < 1e-9 || t <= 0.0 {
        return None;
    }
    let p = o + d * t;
    let r = (p - centre).length() / radius.max(1e-9);
    (RING_GRAB.0..=RING_GRAB.1).contains(&r).then_some(p)
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
                        let body = [GizmoHandle::Joint, GizmoHandle::Plane, GizmoHandle::Slide]
                            .into_iter()
                            .find(|h| available.contains(h));
                        let filter = |e: Entity| {
                            handles_q.contains(e)
                                || (body.is_some() && picks.get(e).is_ok_and(|p| p.0 == SelectionTarget::Node(id)))
                        };
                        let settings = MeshRayCastSettings::default().with_filter(&filter);
                        let grabbed = match ray_cast.cast_ray(ray, &settings).first() {
                            Some((entity, hit)) => {
                                handles_q.get(*entity).ok().copied().or(body).map(|h| (h, from_render(hit.point)))
                            }
                            None if available.contains(&GizmoHandle::Ring) => {
                                let radius = f64::from(gizmo.radius / crate::scene::MM);
                                ring_axis(doc, &catalog.0, node)
                                    .and_then(|(centre, axis)| ring_grab(centre, axis, radius, render_ray(ray)))
                                    .map(|p| (GizmoHandle::Ring, p))
                            }
                            None => None,
                        };
                        let Some((handle, grab)) = grabbed else { continue };
                        if let Some(drag) = Drag::new(doc, &catalog.0, node, handle, grab) {
                            // A multi-DOF joint drag reports its unit once the DOF is picked.
                            if !(handle == GizmoHandle::Joint && drag.dof.is_none()) {
                                events.send(ViewportEvent::DragStarted { id, unit: drag.unit() });
                            }
                            gizmo.drag = Some(drag);
                            capture.0 = true;
                        }
                    }
                    ViewportInput::PointerMove { pos, mods } => {
                        let Some(drag) = &gizmo.drag else { continue };
                        let Some(ray) = ray_at(pos) else { continue };
                        if drag.handle == GizmoHandle::Joint && drag.dof.is_none() {
                            // Multi-DOF joint: the DOF whose screen direction best matches the first moves.
                            let Some(from) = pointer.press_start() else { continue };
                            if from.distance(pos) < DOF_PICK_PX {
                                continue;
                            }
                            let (Attach::Joint(j), Some((o, r))) = (&drag.start.attach, drag.joint) else { continue };
                            let moved = (pos - from).normalize();
                            let screen = |p: DVec3| camera.world_to_viewport(camera_transform, to_render(p)).ok();
                            let best = dofs(j.kind).iter().enumerate().filter_map(|(i, d)| {
                                let axis = dof_axis(*d, r);
                                let dir = if is_rot(*d) { axis.cross(drag.grab - o).normalize_or_zero() } else { axis };
                                let s = (screen(drag.grab + dir * 10.0)? - screen(drag.grab)?).normalize_or_zero();
                                Some((i, s.dot(moved).abs()))
                            });
                            let Some((i, _)) = best.max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
                            let id = drag.start.id;
                            let Some(drag) = gizmo.drag.as_mut() else { continue };
                            drag.dof = Some(i);
                            events.send(ViewportEvent::DragStarted { id, unit: drag.unit() });
                        }
                        let Some(drag) = &gizmo.drag else { continue };
                        let Some(result) = drag.update(doc, &catalog.0, render_ray(ray), mods.shift, mods.ctrl) else {
                            continue;
                        };
                        if drag.last.as_ref() != Some(&result) {
                            let (attach, position, rotation) = result.clone();
                            events.send(ViewportEvent::TransformRequested {
                                id: drag.start.id,
                                parent: drag.start.parent,
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
                        // A press without a move changes nothing (and must not unmate in place).
                        let Some((attach, position, rotation)) = drag.last else { continue };
                        let s = &drag.start;
                        events.send(ViewportEvent::TransformRequested { id: s.id, parent: s.parent, attach, position, rotation, done: true });
                    }
                    _ => {}
                }
            }
            FrameRequest::LoadDocument(_) => gizmo.drag = None,
            FrameRequest::DragValue(v) => {
                let (Some(doc), Some(drag)) = (&index.snapshot, gizmo.drag.take()) else { continue };
                match drag.exact(doc, &catalog.0, *v) {
                    Some((attach, position, rotation)) => {
                        let s = &drag.start;
                        events.send(ViewportEvent::TransformRequested { id: s.id, parent: s.parent, attach, position, rotation, done: true });
                    }
                    None => gizmo.drag = Some(drag),
                }
            }
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
    capturing: Res<crate::export::Capturing>,
    mut gizmo: ResMut<Gizmo>,
    (index, catalog): (Res<SceneIndex>, Res<CatalogRes>),
    (mut meshes, mut materials, mut settle): (ResMut<Assets<Mesh>>, ResMut<Assets<StandardMaterial>>, ResMut<SettleFrames>),
) {
    let changed = selection.is_changed()
        || capturing.is_changed()
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
    if capturing.0 {
        return;
    }
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
    let free = constrained_motion(doc, node);

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
            GizmoHandle::Joint => continue,
            GizmoHandle::Plane => {
                // Annulus normal +Z onto the drag plane: room up, or a constrained node's slide plane.
                let normal = free
                    .as_ref()
                    .map_or(Vec3::Y, |(f, (_, pq))| crate::scene::normal_to_render(*pq * slide_normal(f)));
                (
                    meshes.add(Annulus::new(r * 0.8, r * 1.2)),
                    faint.clone(),
                    Transform::from_rotation(Quat::from_rotation_arc(Vec3::Z, normal)).with_translation(normal * 0.0005),
                )
            }
            GizmoHandle::Ring => {
                let (centre, axis) = ring_axis(doc, &catalog.0, node).unwrap_or((anchor, DVec3::Z));
                (
                    meshes.add(Torus::new(RING.0 * r, RING.1 * r)),
                    strong.clone(),
                    Transform::from_rotation(Quat::from_rotation_arc(Vec3::Y, crate::scene::normal_to_render(axis)))
                        .with_translation(to_render(centre) - to_render(anchor)),
                )
            }
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
    // Circles lie in the isometry's XY plane: the annulus already does, the torus lies in XZ.
    let at = |g: &GlobalTransform, turn: Quat| Isometry3d::new(g.translation(), g.rotation() * turn);
    for (handle, global) in &handles {
        match handle {
            GizmoHandle::Plane => {
                lines.circle(at(global, Quat::IDENTITY), r * 0.8, color);
                lines.circle(at(global, Quat::IDENTITY), r * 1.2, color);
            }
            GizmoHandle::Ring => {
                let flat = at(global, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
                lines.circle(flat, r * RING.0, color);
                lines.circle(flat, r * RING.1, color);
            }
            GizmoHandle::Joint => {}
            GizmoHandle::Slide => {
                let half = Vec3::Y * 0.75 * r / global.scale().y.max(1e-6);
                lines.line(global.transform_point(-half), global.transform_point(half), color);
            }
        }
    }
}

/// Render-space direction of a mated node's slide axis.
fn slide_axis(doc: &Document, catalog: &Catalog, node: &Node) -> Option<Vec3> {
    if let Some((free, (_, pq))) = constrained_motion(doc, node) {
        return Some(crate::scene::normal_to_render(pq * *free.slides.first()?));
    }
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
        Node { select_lock: false,
            note: String::new(),
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
        assert_eq!(handles(&doc, &catalog, &post, Tool::Move), vec![GizmoHandle::Slide, GizmoHandle::Plane]);
        let grab = room(p0.extend(60.0));
        let drag = Drag::new(&doc, &catalog, &post, GizmoHandle::Slide, grab).unwrap();
        // A horizontal ray passing 10.4 mm below the grab point.
        let ray = (grab - DVec3::Z * 10.4 - DVec3::Y * 500.0, DVec3::Y);
        let (attach, position, _) = drag.update(&doc, &catalog, ray, false, false).unwrap();
        assert!(matches!(attach, Attach::Mate { slide, .. } if slide == -15.0), "{attach:?}");
        assert!((position - DVec3::new(0.0, 0.0, 35.0)).length() < 1e-9);

        // Move sideways: the post leaves the holder and stands free on the platform (POLA-104).
        let pose = doc.node_pose(post.id).unwrap().0;
        let drag = Drag::new(&doc, &catalog, &post, GizmoHandle::Plane, room(pose)).unwrap();
        assert_eq!(drag.start.parent, Parent::Platform(pid));
        let (attach, position, _) = drag.update(&doc, &catalog, down(room(pose + DVec3::X * 30.0)), true, false).unwrap();
        assert_eq!(attach, Attach::Free);
        assert!((position - (pose + DVec3::X * 30.0)).length() < 1e-6, "{position:?}");
    }

    /// M9 E2: a typed 50 during a plane drag moves exactly 50 mm along the drag direction.
    #[test]
    fn plane_drag_value_is_exact() {
        let mut doc = lab_core::sample::demo_document();
        let catalog = catalog();
        let pid = doc.platforms[0].id;
        let hid = doc.next_free_id();
        doc.nodes.push(node(hid, "t:PH50/M", Parent::Platform(pid), Attach::Free, DVec3::new(3.0, 4.0, 0.0)));
        let holder = doc.node(NodeId(hid)).unwrap().clone();
        let (origin, angle) = doc.platform_pose(pid).unwrap();
        let room = |local: DVec3| origin + DQuat::from_rotation_z(angle) * local;
        let mut drag = Drag::new(&doc, &catalog, &holder, GizmoHandle::Plane, room(holder.position)).unwrap();
        drag.last = drag.update(&doc, &catalog, down(room(holder.position + DVec3::new(7.3, 7.3, 0.0))), true, false);
        let (attach, position, _) = drag.exact(&doc, &catalog, 50.0).unwrap();
        assert_eq!(attach, Attach::Free);
        let moved = position - holder.position;
        assert!((moved.length() - 50.0).abs() < 1e-9, "{moved:?}");
        assert!((moved.x - moved.y).abs() < 1e-9 && moved.z == 0.0, "{moved:?}");
    }

    #[test]
    fn near_miss_still_grabs_the_ring() {
        let anchor = DVec3::new(10.0, 0.0, 5.0);
        assert!(ring_grab(anchor, DVec3::Z, 40.0, down(anchor + DVec3::X * 60.0)).is_some());
        assert!(ring_grab(anchor, DVec3::Z, 40.0, down(anchor + DVec3::X * 20.0)).is_none());
        assert!(ring_grab(anchor, DVec3::Z, 40.0, down(anchor + DVec3::X * 80.0)).is_none());
    }
}
