//! Laser beams (M6-PLAN B): the viewport traces its own snapshot with `lab_optics` and draws each
//! segment as an unlit truncated cone of the 1/e² radius. Traced only on document, catalog,
//! isolation and `SetBeams` changes, never on a timer. Beams carry no `PickId`, so picking skips them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use bevy::{
    camera::visibility::VisibilitySystems,
    ecs::system::ParamSet,
    light::NotShadowCaster,
    math::DVec3,
    mesh::{ConicalFrustumMeshBuilder, MeshBuilder},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
};
use lab_core::{NodeId, Shape};
use lab_optics::{Segment, Trace};

use crate::glb::GlbLoading;
use crate::host::{EventSender, PendingRequests, SettleFrames};
use crate::isolate::{Isolated, apply_isolation, kept_platforms};
use crate::scene::{MM, SceneIndex, SceneNode, normal_to_render, to_render};
use crate::{CatalogRes, FrameRequest, ViewportEvent};

/// Thinnest drawn radius, so a focused beam stays visible.
const MIN_RADIUS_MM: f64 = 0.3;

#[derive(Resource)]
pub(crate) struct Beams {
    show: bool,
    inspecting: bool,
    root: Option<Entity>,
    materials: HashMap<u64, Handle<StandardMaterial>>,
    /// The last trace drawn (empty while beams are hidden), for the probe's screen-space snap.
    pub(crate) trace: Trace,
}

impl Default for Beams {
    fn default() -> Self {
        Beams { show: true, inspecting: false, root: None, materials: HashMap::new(), trace: Trace::default() }
    }
}

pub(crate) struct BeamsPlugin;

impl Plugin for BeamsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Beams>().add_systems(
            PostUpdate,
            update_beams
                .after(apply_isolation)
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate)
                .before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

#[derive(Default)]
struct BeamTraceState {
    full_dirty: bool,
    cheap_dirty: bool,
    last_blocking_current: Option<bool>,
}

#[allow(clippy::too_many_arguments)]
fn update_beams(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    index: Res<SceneIndex>,
    catalog: Res<CatalogRes>,
    isolated: Res<Isolated>,
    loading: Res<GlbLoading>,
    mut beams: ResMut<Beams>,
    scene_meshes: Query<(Entity, &SceneNode), With<Mesh3d>>,
    mut sets: ParamSet<(MeshRayCast, ResMut<Assets<Mesh>>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut settle: ResMut<SettleFrames>,
    events: Res<EventSender>,
    mut state: Local<BeamTraceState>,
) {
    if catalog.is_changed() || isolated.is_changed() {
        state.cheap_dirty = true;
    }
    for request in &pending.0 {
        match request {
            FrameRequest::SetBeams(on) => {
                beams.show = *on;
                state.cheap_dirty = true;
            }
            FrameRequest::InspectPart(part) => {
                beams.inspecting = part.is_some();
                state.cheap_dirty = true;
            }
            FrameRequest::ApplyChanges(..) => state.cheap_dirty = true,
            FrameRequest::LoadDocument(_) | FrameRequest::RecalculateBeams => state.full_dirty = true,
            _ => {}
        }
    }
    if !beams.show || beams.inspecting {
        if state.full_dirty || state.cheap_dirty {
            state.full_dirty = false;
            state.cheap_dirty = false;
            if let Some(root) = beams.root.take() {
                commands.entity(root).try_despawn();
            }
            settle.0 = settle.0.max(2);
            beams.trace = Trace::default();
        }
        return;
    }
    if (!state.full_dirty && !state.cheap_dirty) || (state.full_dirty && loading.0) {
        return;
    }
    let full = state.full_dirty;
    state.full_dirty = false;
    state.cheap_dirty = false;
    if let Some(root) = beams.root.take() {
        commands.entity(root).try_despawn();
    }
    settle.0 = settle.0.max(2);
    beams.trace = Trace::default();
    let Some(doc) = index.snapshot.as_ref() else { return };
    let (trace, blocking_current, full_ms) = if full {
        // Unregistered parts (empty optical_ports) block beams with their actual mesh geometry.
        let blockers: HashMap<Entity, NodeId> = scene_meshes
            .iter()
            .filter_map(|(entity, scene_node)| {
                let node = doc.node(scene_node.0)?;
                if !matches!(node.shape, Shape::Part) {
                    return None;
                }
                catalog.0.get(&node.part).is_some_and(|d| d.optical_ports.is_empty()).then_some((entity, scene_node.0))
            })
            .collect();
        let ray_cast = RefCell::new(sets.p0());
        let block = |origin: DVec3, unit_dir: DVec3, max_dist: f64| -> Option<(NodeId, f64)> {
            if blockers.is_empty() {
                return None;
            }
            let dir = Dir3::new(normal_to_render(unit_dir)).ok()?;
            let ray = Ray3d::new(to_render(origin), dir);
            let filter = |e: Entity| blockers.contains_key(&e);
            let settings = MeshRayCastSettings::default()
                .with_filter(&filter)
                .with_visibility(RayCastVisibility::Any)
                .never_early_exit();
            let mut rc = ray_cast.borrow_mut();
            for (entity, hit) in rc.cast_ray(ray, &settings) {
                let dist_mm = hit.distance as f64 / MM as f64;
                if dist_mm < 0.01 {
                    continue;
                }
                if dist_mm < max_dist {
                    return blockers.get(entity).copied().map(|id| (id, dist_mm));
                }
                break;
            }
            None
        };
        let t0 = Instant::now();
        let trace = lab_optics::trace_document_with(doc, &|id| catalog.0.get(id).cloned(), &block);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        (trace, true, Some(ms))
    } else {
        let trace = lab_optics::trace_document(doc, &|id| catalog.0.get(id).cloned());
        let has_blockers = doc.nodes.iter().any(|n| {
            matches!(n.shape, Shape::Part) && catalog.0.get(&n.part).is_some_and(|d| d.optical_ports.is_empty())
        });
        (trace, !has_blockers, None)
    };
    if full_ms.is_some() || state.last_blocking_current != Some(blocking_current) {
        state.last_blocking_current = Some(blocking_current);
        events.send(ViewportEvent::BeamsTraced { blocking_current, full_ms });
    }
    if trace.segments.is_empty() {
        return;
    }
    let kept = isolated.0.map(|p| kept_platforms(doc, p));
    let root = commands.spawn((Transform::default(), Visibility::Inherited)).id();
    let mut meshes = sets.p1();
    for segment in &trace.segments {
        if let Some(kept) = &kept
            && !doc.root_platform(segment.source).is_some_and(|p| kept.contains(&p))
        {
            continue;
        }
        beams.trace.segments.push(segment.clone());
        let wavelength = doc
            .node(segment.source)
            .and_then(|n| n.params.get("beam_wavelength_nm"))
            .and_then(lab_core::part::ParamValue::as_f64)
            .unwrap_or(780.0);
        let material = beams
            .materials
            .entry(wavelength.to_bits())
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: beam_color(wavelength),
                    unlit: true,
                    ..default()
                })
            })
            .clone();
        for (a, wa, b, wb) in pieces(segment) {
            let (ra, rb) = (to_render(a), to_render(b));
            let height = ra.distance(rb);
            if height < 1e-6 {
                continue;
            }
            let r = |w: f64| (w.max(MIN_RADIUS_MM) as f32) * MM;
            let mesh = ConicalFrustumMeshBuilder::new(r(wb), r(wa), height, 12).build();
            let transform = Transform::from_translation((ra + rb) * 0.5)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, (rb - ra) / height));
            commands.spawn((
                ChildOf(root),
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                NotShadowCaster,
                transform,
            ));
        }
    }
    beams.root = Some(root);
}

/// The drawn pieces of a segment `(start, w, end, w)`: split at the waist so a focus narrows.
/// Radii between known points are linear.
fn pieces(s: &Segment) -> Vec<(DVec3, f64, DVec3, f64)> {
    let mut spans = vec![(s.start, s.w_start)];
    if let Some(waist) = s.waist {
        spans.push(waist);
    }
    spans.push((s.end, s.w_end));
    spans.windows(2).map(|pair| (pair[0].0, pair[0].1, pair[1].0, pair[1].1)).collect()
}

/// Visible light: its hue. IR and UV: the false-colour accent.
fn beam_color(nm: f64) -> Color {
    if !(400.0..=700.0).contains(&nm) {
        return Color::srgb(1.0, 0.25, 0.8);
    }
    let (r, g, b) = match nm {
        n if n < 440.0 => ((440.0 - n) / 40.0, 0.0, 1.0),
        n if n < 490.0 => (0.0, (n - 440.0) / 50.0, 1.0),
        n if n < 510.0 => (0.0, 1.0, (510.0 - n) / 20.0),
        n if n < 580.0 => ((n - 510.0) / 70.0, 1.0, 0.0),
        n if n < 645.0 => (1.0, (645.0 - n) / 65.0, 0.0),
        _ => (1.0, 0.0, 0.0),
    };
    Color::srgb(r as f32, g as f32, b as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::NodeId;

    #[test]
    fn focus_splits_at_the_waist() {
        let s = Segment {
            source: NodeId(1),
            start: DVec3::ZERO,
            end: DVec3::new(0.0, 200.0, 0.0),
            w_start: 0.5,
            w_end: 0.5,
            waist: Some((DVec3::new(0.0, 100.0, 0.0), 0.05)),
            hit: None,
            blocked: None,
            depth: 0,
            field: lab_optics::Field { re: DVec3::X, im: DVec3::ZERO },
            power: 1.0,
            q_start: (0.0, 1.0),
            lm2: 1e-3,
            frame_x: DVec3::X,
            path_start: 0.0,
        };
        let solid = pieces(&s);
        assert_eq!(solid.len(), 2);
        assert!((solid[0].3 - 0.05).abs() < 1e-12);
    }
}
