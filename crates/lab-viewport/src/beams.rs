//! Laser beams (M6-PLAN B): the viewport traces its own snapshot with `lab_optics` and draws each
//! segment as an unlit truncated cone of the 1/e² radius. Traced only on document, catalog,
//! isolation and `SetBeams` changes, never on a timer. Beams carry no `PickId`, so picking skips them.

use std::collections::HashMap;

use bevy::{
    camera::visibility::VisibilitySystems, light::NotShadowCaster, math::DVec3, mesh::ConicalFrustumMeshBuilder,
    mesh::MeshBuilder, prelude::*,
};
use lab_optics::Segment;

use crate::host::{PendingRequests, SettleFrames};
use crate::isolate::{Isolated, apply_isolation, kept_platforms};
use crate::scene::{MM, SceneIndex, to_render};
use crate::{CatalogRes, FrameRequest};

/// Thinnest drawn radius, so a focused beam stays visible.
const MIN_RADIUS_MM: f64 = 0.3;
/// IR/UV beams are dashed (DESIGN.md §7: never colour alone): drawn, then gap, in mm.
const DASH_MM: f64 = 6.0;
const GAP_MM: f64 = 4.0;
const MAX_DASHES: f64 = 400.0;

#[derive(Resource)]
struct Beams {
    show: bool,
    inspecting: bool,
    root: Option<Entity>,
    materials: HashMap<u64, Handle<StandardMaterial>>,
}

impl Default for Beams {
    fn default() -> Self {
        Beams { show: true, inspecting: false, root: None, materials: HashMap::new() }
    }
}

pub(crate) struct BeamsPlugin;

impl Plugin for BeamsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Beams>().add_systems(
            PostUpdate,
            update_beams.after(apply_isolation).before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn update_beams(
    mut commands: Commands,
    pending: Res<PendingRequests>,
    index: Res<SceneIndex>,
    catalog: Res<CatalogRes>,
    isolated: Res<Isolated>,
    mut beams: ResMut<Beams>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut settle: ResMut<SettleFrames>,
) {
    let mut dirty = catalog.is_changed() || isolated.is_changed();
    for request in &pending.0 {
        match request {
            FrameRequest::SetBeams(on) => {
                beams.show = *on;
                dirty = true;
            }
            FrameRequest::InspectPart(part) => {
                beams.inspecting = part.is_some();
                dirty = true;
            }
            FrameRequest::LoadDocument(_) | FrameRequest::ApplyChanges(..) => dirty = true,
            _ => {}
        }
    }
    if !dirty {
        return;
    }
    if let Some(root) = beams.root.take() {
        commands.entity(root).try_despawn();
    }
    settle.0 = settle.0.max(2);
    let Some(doc) = index.snapshot.as_ref().filter(|_| beams.show && !beams.inspecting) else { return };
    let trace = lab_optics::trace_document(doc, &|id| catalog.0.get(id).cloned());
    if trace.segments.is_empty() {
        return;
    }
    let kept = isolated.0.map(|p| kept_platforms(doc, p));
    let root = commands.spawn((Transform::default(), Visibility::Inherited)).id();
    for segment in &trace.segments {
        if let Some(kept) = &kept
            && !doc.root_platform(segment.source).is_some_and(|p| kept.contains(&p))
        {
            continue;
        }
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
        let dashed = !(400.0..=700.0).contains(&wavelength);
        for (a, wa, b, wb) in pieces(segment, dashed) {
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

/// The drawn pieces of a segment `(start, w, end, w)`: split at the waist so a focus narrows, and
/// into dashes when `dashed`. Radii between known points are linear.
fn pieces(s: &Segment, dashed: bool) -> Vec<(DVec3, f64, DVec3, f64)> {
    let mut spans = vec![(s.start, s.w_start)];
    if let Some(waist) = s.waist {
        spans.push(waist);
    }
    spans.push((s.end, s.w_end));
    let mut out = Vec::new();
    for pair in spans.windows(2) {
        let ((a, wa), (b, wb)) = (pair[0], pair[1]);
        let length = a.distance(b);
        if !dashed || length <= DASH_MM {
            out.push((a, wa, b, wb));
            continue;
        }
        let period = (DASH_MM + GAP_MM).max(length / MAX_DASHES);
        let at = |t: f64| (a.lerp(b, t / length), wa + (wb - wa) * t / length);
        let mut t = 0.0;
        while t < length {
            let ((p, wp), (q, wq)) = (at(t), at((t + period * 0.6).min(length)));
            out.push((p, wp, q, wq));
            t += period;
        }
    }
    out
}

/// Visible light: its hue. IR and UV: the false-colour accent (the beams are dashed too).
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
    fn focus_splits_and_ir_dashes() {
        let s = Segment {
            source: NodeId(1),
            start: DVec3::ZERO,
            end: DVec3::new(0.0, 200.0, 0.0),
            w_start: 0.5,
            w_end: 0.5,
            waist: Some((DVec3::new(0.0, 100.0, 0.0), 0.05)),
            hit: None,
            depth: 0,
        };
        let solid = pieces(&s, false);
        assert_eq!(solid.len(), 2);
        assert!((solid[0].3 - 0.05).abs() < 1e-12);
        let dashed = pieces(&s, true);
        assert_eq!(dashed.len(), 20);
    }
}
