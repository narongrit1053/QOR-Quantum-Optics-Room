//! Inspect mode for the connector wizard (M4-PLAN POLA-60): one catalog part alone at the
//! origin, framed, with connector markers; clicks on it report the hit point and normal.

use bevy::{
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    math::{DQuat, DVec3},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    ecs::system::ParamSet,
    prelude::*,
};
use lab_core::part::Frame;
use lab_core::{Attach, Finish, Node, NodeId, Parent, Shape};

use crate::camera::{Clicks, OrbitCamera, TopPane, View, camera_at, frame_box, handle_input};
use crate::host::{EventSender, PendingRequests, SettleFrames, ViewportSize};
use crate::isolate::Isolated;
use crate::scene::{SceneAssets, SceneEntity, SceneIndex, from_render, spawn_node, to_render};
use crate::{FrameRequest, ViewportEvent};

#[derive(Resource, Default)]
struct Inspect {
    root: Option<Entity>,
    /// The main camera's view before inspecting, restored on release.
    saved: Option<View>,
    frame_pending: bool,
    markers: Vec<Frame>,
}

pub(crate) struct InspectPlugin;

impl Plugin for InspectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Inspect>()
            .add_systems(Update, (apply_inspect.after(handle_input), draw_markers))
            .add_systems(
                PostUpdate,
                hide_document.after(crate::isolate::apply_isolation).before(VisibilitySystems::VisibilityPropagate),
            );
    }
}

#[allow(clippy::too_many_arguments)]
type InspectParams<'w, 's> = (
    Query<'w, 's, (Entity, &'static Aabb, &'static GlobalTransform)>,
    Query<'w, 's, &'static ChildOf>,
    Res<'w, ViewportSize>,
    Res<'w, EventSender>,
    ResMut<'w, SettleFrames>,
);

fn apply_inspect(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    clicks: Res<Clicks>,
    mut state: ResMut<Inspect>,
    mut sets: ParamSet<(SceneAssets, MeshRayCast)>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, &GlobalTransform), Without<TopPane>>,
    (pieces, parents, full, events, mut settle): InspectParams,
) {
    for request in &pending.0 {
        match request {
            FrameRequest::InspectPart(part) => {
                if let Some(root) = state.root.take() {
                    commands.entity(root).try_despawn();
                }
                settle.0 = settle.0.max(2);
                match part {
                    Some(id) => {
                        if state.saved.is_none() {
                            state.saved = cameras.iter().next().map(|(o, ..)| o.target_view());
                        }
                        let node = Node {
                            id: NodeId(u64::MAX),
                            name: id.clone(),
                            part: id.clone(),
                            parent: Parent::Room,
                            attach: Attach::Free,
                            position: DVec3::ZERO,
                            rotation: DQuat::IDENTITY,
                            shape: Shape::Part,
                            finish: Finish::AnodizedBlack,
                            params: Default::default(),
                            pickable: false,
                            locked: false,
                        };
                        let root = spawn_node(&mut commands, None, &node, node.position, node.rotation, &mut sets.p0(), &events);
                        // Not part of the document: a document reload must not despawn it.
                        commands.entity(root).remove::<SceneEntity>();
                        state.root = Some(root);
                        state.frame_pending = true;
                    }
                    None => {
                        state.markers.clear();
                        state.frame_pending = false;
                        if let (Some(view), Some((mut orbit, ..))) = (state.saved.take(), cameras.iter_mut().next()) {
                            orbit.ease_to(view);
                        }
                    }
                }
            }
            FrameRequest::InspectMarkers(frames) => state.markers = frames.clone(),
            _ => {}
        }
    }
    let Some(root) = state.root else { return };
    let inside = |e: Entity| e == root || parents.iter_ancestors(e).any(|a| a == root);

    // Frame once the part's meshes have bounds (a GLB arrives some frames later).
    if state.frame_pending {
        let bounds = pieces
            .iter()
            .filter(|(e, ..)| inside(*e))
            .map(|(_, aabb, t)| {
                let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
                let corners = [-1.0f32, 1.0].into_iter().flat_map(|x| {
                    [-1.0f32, 1.0].into_iter().flat_map(move |y| [-1.0f32, 1.0].map(|z| Vec3::new(x, y, z)))
                });
                corners.map(|k| t.transform_point(c + h * k)).fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)))
            })
            .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)));
        if let Some((min, max)) = bounds {
            for (mut orbit, camera, _) in &mut cameras {
                frame_box(&mut orbit, camera, min, max);
            }
            state.frame_pending = false;
        } else {
            settle.0 = settle.0.max(1);
        }
    }

    for &click in &clicks.0 {
        let Some(((), camera, transform, p)) = camera_at(cameras.iter().map(|(_, c, t)| ((), c, t)), &full, click) else {
            continue;
        };
        let Ok(ray) = camera.viewport_to_world(transform, p) else { continue };
        let filter = |e: Entity| inside(e);
        let settings = MeshRayCastSettings::default().with_filter(&filter);
        if let Some((_, hit)) = sets.p1().cast_ray(ray, &settings).first() {
            // The part sits at the origin unrotated: render space → part space is the document map.
            let n = hit.normal;
            events.send(ViewportEvent::PartPoint {
                point: from_render(hit.point),
                normal: DVec3::new(n.x as f64, -n.z as f64, n.y as f64).normalize_or_zero(),
            });
        }
    }
}

/// While inspecting, everything of the document is hidden (re-applied every frame, because
/// isolation and the wall fade write visibility too).
fn hide_document(
    state: Res<Inspect>,
    index: Res<SceneIndex>,
    isolated: Res<Isolated>,
    mut was_inspecting: Local<bool>,
    mut visibility: Query<&mut Visibility>,
) {
    let inspecting = state.root.is_some();
    // On release, show everything again unless isolation owns the visibility.
    if !inspecting && (!*was_inspecting || isolated.0.is_some()) {
        *was_inspecting = false;
        return;
    }
    *was_inspecting = inspecting;
    let want = if inspecting { Visibility::Hidden } else { Visibility::Inherited };
    let entities = index.room.iter().chain(index.platforms.values().map(|p| &p.root)).chain(index.nodes.values());
    for &entity in entities {
        if let Ok(mut v) = visibility.get_mut(entity)
            && *v != want
        {
            *v = want;
        }
    }
}

fn draw_markers(state: Res<Inspect>, mut gizmos: Gizmos) {
    if state.root.is_none() {
        return;
    }
    for f in &state.markers {
        let o = to_render(f.origin);
        gizmos.arrow(o, to_render(f.origin + f.z * 12.0), Color::srgb(0.2, 0.55, 1.0));
        gizmos.arrow(o, to_render(f.origin + f.x * 6.0), Color::srgb(1.0, 0.3, 0.25));
    }
}
