//! The Beam Profiler cards (beam polarization spec, "Beam profiler probe"): the viewport sends the
//! probe points as overlay items; each card reads `AppState.beams` there with `Trace::probe`, on
//! every overlay change and after every retrace.

use std::cell::RefCell;

use lab_core::glam::DVec3;
use lab_core::units::format_length_with_unit;
use lab_core::{Document, LengthUnit};
use lab_optics::{PROBE_TOL_MM, Polarization, ProbeReading};
use lab_ui::{AppWindow, ProbeCard};
use lab_viewport::Vec2;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app_state::AppState;

/// The widest spot among the cards fills this diameter.
const SPOT_PX: f64 = 40.0;
/// Half the ellipse box: a linear state is a line this long either side.
const ELLIPSE_R_PX: f32 = 14.0;

thread_local! {
    /// The last probes from the overlay: (index, world point, physical px).
    static LAST: RefCell<Vec<(usize, DVec3, Vec2)>> = const { RefCell::new(Vec::new()) };
}

pub fn on_items(ui: &AppWindow, app: &AppState, items: Vec<(usize, DVec3, Vec2)>) {
    LAST.with(|l| *l.borrow_mut() = items);
    refresh(ui, app);
}

/// Recomputes the cards from the current trace, e.g. after a retrace.
pub fn refresh(ui: &AppWindow, app: &AppState) {
    let scale = ui.window().scale_factor();
    let probes = LAST.with(|l| l.borrow().clone());
    let readings: Vec<_> = probes.iter().map(|(_, at, _)| app.beams.probe(*at, PROBE_TOL_MM)).collect();
    let widest = readings.iter().filter_map(|r| r.first()).map(|r| r.w).fold(0.0, f64::max);
    let cards: Vec<ProbeCard> = probes
        .iter()
        .zip(&readings)
        .map(|((index, _, pos), readings)| {
            let (title, lines) = card(readings, &app.session.doc, app.units());
            // The graphics show the first pass; the lines list every pass.
            let first = readings.first();
            ProbeCard {
                index: *index as i32,
                x: pos.x / scale,
                y: pos.y / scale,
                title: title.into(),
                lines: lines.join("\n").into(),
                spot: first.filter(|_| widest > 0.0).map_or(0.0, |r| (SPOT_PX * r.w / widest) as f32),
                ellipse: first.map(|r| ellipse_path(r.pol, ELLIPSE_R_PX)).unwrap_or_default().into(),
                power: first.map_or(0.0, |r| r.power as f32),
            }
        })
        .collect();
    ui.set_probe_cards(ModelRc::new(VecModel::from(cards)));
}

/// The card title and its text lines, one block per pass.
fn card(readings: &[ProbeReading], doc: &Document, unit: LengthUnit) -> (String, Vec<String>) {
    if readings.is_empty() {
        return ("Beam profiler".into(), vec!["No beam here".into()]);
    }
    let several = readings.len() > 1;
    let mut lines = Vec::new();
    for (k, r) in readings.iter().enumerate() {
        if several {
            lines.push(format!("Pass {}", k + 1));
        }
        let name = doc.node(r.source).map_or_else(|| "source".to_string(), |n| n.name.clone());
        lines.push(format!("2w = {:.2} mm", 2.0 * r.w));
        lines.push(r.pol.label());
        lines.push(format!("{:.0} % power", r.power * 100.0));
        lines.push(format!("{} from {name}", format_length_with_unit(r.distance, unit)));
    }
    let title = if several { format!("Beam profiler · {} passes", readings.len()) } else { "Beam profiler".into() };
    (title, lines)
}

/// SVG path commands of the polarization ellipse in a `2 × radius_px` box, as seen by the receiver
/// (horizontal right, vertical up), with a half arrowhead for the handedness.
fn ellipse_path(pol: Polarization, radius_px: f32) -> String {
    use std::fmt::Write;
    const STEPS: usize = 48;
    let r = radius_px as f64;
    let (psi, chi) = (pol.azimuth_deg.to_radians(), pol.ellipticity_deg.to_radians());
    // Semi-axes with a² + b² = r²; b is signed so χ > 0 turns clockwise in the receiver frame.
    let (a, b) = (r * chi.cos(), -r * chi.sin());
    let (s, c) = psi.sin_cos();
    // Receiver frame (y up), turned by ψ, to path space (y down) centred in the box.
    let at = |x: f64, y: f64| (r + x * c - y * s, r - (x * s + y * c));
    let mut out = String::new();
    for i in 0..STEPS {
        let t = std::f64::consts::TAU * i as f64 / STEPS as f64;
        let (x, y) = at(a * t.cos(), b * t.sin());
        let _ = write!(out, "{} {x:.2} {y:.2} ", if i == 0 { "M" } else { "L" });
    }
    out.push('Z');
    if pol.ellipticity_deg.abs() >= 0.5 {
        // Half arrowhead at the major-axis end, its tail behind the tip along the turn, inside.
        let h = 0.35 * r;
        let ((tx, ty), (hx, hy)) = (at(a, 0.0), at(a - h, -h * b.signum()));
        let _ = write!(out, " M {tx:.2} {ty:.2} L {hx:.2} {hy:.2}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(pol: Polarization, source: lab_core::NodeId) -> ProbeReading {
        ProbeReading { source, segment: 0, w: 0.51, pol, power: 0.62, distance: 312.0 }
    }

    #[test]
    fn card_formats_readings() {
        let doc = lab_core::sample::demo_document();
        let laser = doc.nodes.iter().find(|n| n.name.starts_with("Laser")).unwrap().id;
        let pol = Polarization { azimuth_deg: 22.0, ellipticity_deg: 0.0 };
        let (_, lines) = card(&[reading(pol, laser)], &doc, LengthUnit::Millimetre);
        let text = lines.join("\n");
        for want in ["2w = 1.02 mm", "Linear 22°", "62 % power", "from Laser"] {
            assert!(text.contains(want), "{want} missing in {text}");
        }
    }

    #[test]
    fn card_without_readings_says_no_beam() {
        let (_, lines) = card(&[], &Document::new("x"), LengthUnit::Millimetre);
        assert_eq!(lines, ["No beam here"]);
    }

    /// (min x, min y, max x, max y) of the path's points.
    fn bbox(path: &str) -> (f32, f32, f32, f32) {
        let nums: Vec<f32> = path.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        nums.chunks(2).fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p[0]), b.1.min(p[1]), b.2.max(p[0]), b.3.max(p[1])))
    }

    #[test]
    fn ellipse_path_is_closed_and_circular_for_circular() {
        let circular = ellipse_path(Polarization { azimuth_deg: 30.0, ellipticity_deg: 45.0 }, 14.0);
        assert!(circular.contains('Z'), "{circular}");
        let (x0, y0, x1, y1) = bbox(&circular);
        assert!(((x1 - x0) - (y1 - y0)).abs() <= 0.5, "{circular}");
        assert!((x0 + x1 - 28.0).abs() <= 0.5 && (y0 + y1 - 28.0).abs() <= 0.5, "not centred: {circular}");
        let linear = ellipse_path(Polarization { azimuth_deg: 0.0, ellipticity_deg: 0.0 }, 14.0);
        let (x0, y0, x1, y1) = bbox(&linear);
        assert!((x1 - x0 - 28.0).abs() < 0.01 && (y1 - y0).abs() < 0.01, "{linear}");
    }

    /// χ > 0 turns clockwise as seen by the receiver: from the right end the field moves down,
    /// which is +y on screen; the arrow's tail sits behind the tip, above it.
    #[test]
    fn right_handed_arrow_points_down_at_the_right_end() {
        let path = ellipse_path(Polarization { azimuth_deg: 0.0, ellipticity_deg: 20.0 }, 14.0);
        let arrow: Vec<f32> = path.split('Z').nth(1).unwrap().split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let (tip, tail) = ((arrow[0], arrow[1]), (arrow[2], arrow[3]));
        assert!(tip.0 > 14.0 && tail.1 < tip.1, "{path}");
    }
}
