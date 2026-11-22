//! The Beam Profiler cards (beam polarization spec, "Beam profiler probe"): the viewport sends the
//! probe points as overlay items; each card reads `AppState.beams` there with `Trace::probe`, on
//! every overlay change and after every retrace.

use std::cell::RefCell;

use lab_core::glam::DVec3;
use lab_core::units::format_length_with_unit;
use lab_core::{Document, LengthUnit};
use lab_optics::{PROBE_TOL_MM, Polarization, ProbeReading};
use lab_ui::{AppWindow, ProbeCard, ProbePass};
use lab_viewport::Vec2;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app_state::AppState;
use crate::beams::power_label;

/// The widest spot among the cards fills this diameter.
const SPOT_PX: f64 = 40.0;
/// Half the ellipse box: a linear state is a line this long either side.
const ELLIPSE_R_PX: f32 = 14.0;
/// Card width (overlays.slint `ProbeCards`), its offset from the probe dot and the gap between cards.
const CARD_W: f32 = 176.0;
const CARD_OFFSET: f32 = 12.0;
const CARD_GAP: f32 = 6.0;
/// shortcut: card height estimated from the slint layout (padding, title, 40 px graphics, bar,
/// caption lines); if the card layout changes, change these too.
const LINE_PX: f32 = 15.0;

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
    let widest = readings.iter().flatten().map(|r| r.w).fold(0.0, f64::max);
    let rows: Vec<Vec<ProbePass>> = readings.iter().map(|r| passes(r, &app.session.doc, app.units(), widest)).collect();
    let anchors: Vec<(f32, f32)> = probes.iter().map(|(_, _, pos)| (pos.x / scale, pos.y / scale)).collect();
    let heights: Vec<f32> = rows.iter().map(|r| card_height(r)).collect();
    let placed = place_cards(&anchors, &heights, (ui.get_viewport_width(), ui.get_viewport_height()));
    let cards: Vec<ProbeCard> = probes
        .iter()
        .zip(&readings)
        .zip(rows)
        .zip(anchors.iter().zip(placed))
        .map(|((((index, _, _), readings), rows), (&(x, y), (card_x, card_y)))| {
            let (title, _) = card(readings, &app.session.doc, app.units());
            ProbeCard {
                index: *index as i32,
                x,
                y,
                card_x,
                card_y,
                title: title.into(),
                passes: ModelRc::new(VecModel::from(rows)),
            }
        })
        .collect();
    ui.set_probe_cards(ModelRc::new(VecModel::from(cards)));
}

/// Estimated height of a card with these rows (see `LINE_PX`).
fn card_height(rows: &[ProbePass]) -> f32 {
    let row = |p: &ProbePass| 4.0 + if p.ellipse.is_empty() { 0.0 } else { 56.0 } + LINE_PX * p.text.lines().count() as f32;
    4.0 + 20.0 + rows.iter().map(row).sum::<f32>() + 8.0
}

/// Top-left of each card: beside its probe (right and below, or left/up near the pane's edge),
/// stacked below any card already placed that it would cover, on whichever side of the probe
/// that moves it least while staying in the pane.
fn place_cards(anchors: &[(f32, f32)], heights: &[f32], pane: (f32, f32)) -> Vec<(f32, f32)> {
    let mut placed: Vec<(f32, f32, f32)> = Vec::new();
    for (&(ax, ay), &h) in anchors.iter().zip(heights) {
        let (right, left) = (ax + CARD_OFFSET, ax - CARD_OFFSET - CARD_W);
        let xs = if right + CARD_W <= pane.0 || left < 0.0 { [right, left] } else { [left, right] };
        let y0 = if ay + CARD_OFFSET + h <= pane.1 { ay + CARD_OFFSET } else { (ay - CARD_OFFSET - h).max(0.0) };
        let stack = |x: f32| {
            let mut y = y0;
            for _ in 0..=placed.len() {
                let over = placed.iter().find(|&&(px, py, ph)| x < px + CARD_W && px < x + CARD_W && y < py + ph && py < y + h);
                match over {
                    Some(&(_, py, ph)) => y = py + ph + CARD_GAP,
                    None => return Some(y).filter(|y| y + h <= pane.1),
                }
            }
            None
        };
        // The side that moves the card least; on a tie the preferred side.
        let (x, y) = xs
            .iter()
            .filter(|&&x| x >= 0.0 && x + CARD_W <= pane.0)
            .filter_map(|&x| stack(x).map(|y| (x, y)))
            .min_by(|a, b| (a.1 - y0).total_cmp(&(b.1 - y0)))
            .unwrap_or((xs[0], y0));
        placed.push((x, y, h));
    }
    placed.into_iter().map(|(x, y, _)| (x, y)).collect()
}

/// One graphics row per pass; `widest` is the largest w among all shown passes.
fn passes(readings: &[ProbeReading], doc: &Document, unit: LengthUnit, widest: f64) -> Vec<ProbePass> {
    let (_, texts) = card(readings, doc, unit);
    if readings.is_empty() {
        return texts.into_iter().map(|text| ProbePass { text: text.into(), spot: 0.0, ellipse: "".into(), power: 0.0 }).collect();
    }
    readings
        .iter()
        .zip(texts)
        .map(|(r, text)| ProbePass {
            text: text.into(),
            spot: if widest > 0.0 { (SPOT_PX * r.w / widest) as f32 } else { 0.0 },
            ellipse: ellipse_path(r.pol, ELLIPSE_R_PX).into(),
            power: r.power as f32,
        })
        .collect()
}

/// The card title and one text block per pass (lines joined by newlines).
fn card(readings: &[ProbeReading], doc: &Document, unit: LengthUnit) -> (String, Vec<String>) {
    if readings.is_empty() {
        return ("Beam profiler".into(), vec!["No beam here".into()]);
    }
    let several = readings.len() > 1;
    let mut blocks = Vec::new();
    for (k, r) in readings.iter().enumerate() {
        let mut lines = Vec::new();
        if several {
            lines.push(format!("Pass {}", k + 1));
        }
        let name = doc.node(r.source).map_or_else(|| "source".to_string(), |n| n.name.clone());
        lines.push(format!("2w = {:.2} mm", 2.0 * r.w));
        lines.push(r.pol.label());
        lines.push(power_label(r.power));
        // Two short lines: the card does not wrap text.
        lines.push(format!("{} optical path", format_length_with_unit(r.distance, unit)));
        lines.push(format!("from {name}"));
        blocks.push(lines.join("\n"));
    }
    let title = if several { format!("Beam profiler · {} passes", readings.len()) } else { "Beam profiler".into() };
    (title, blocks)
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

    /// A double pass shows a graphics row per pass; the widest spot fills the box.
    #[test]
    fn each_pass_gets_its_own_row() {
        let doc = lab_core::sample::demo_document();
        let laser = doc.nodes.iter().find(|n| n.name.starts_with("Laser")).unwrap().id;
        let linear = reading(Polarization { azimuth_deg: 0.0, ellipticity_deg: 0.0 }, laser);
        let mut circular = reading(Polarization { azimuth_deg: 0.0, ellipticity_deg: 45.0 }, laser);
        circular.w = 0.255;
        let rows = passes(&[linear, circular], &doc, LengthUnit::Millimetre, 0.51);
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].ellipse, rows[1].ellipse);
        assert!(rows[0].text.contains("Linear 0°") && rows[1].text.contains("Circular R"), "{} / {}", rows[0].text, rows[1].text);
        assert!((rows[0].spot - 40.0).abs() < 1e-4 && (rows[1].spot - 20.0).abs() < 1e-4);
        let none = passes(&[], &doc, LengthUnit::Millimetre, 0.0);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].text, "No beam here");
        assert_eq!(none[0].ellipse, "");
    }

    #[test]
    fn cards_do_not_overlap_and_stay_in_the_pane() {
        let pane = (800.0, 600.0);
        // Two probes 10 px apart near the left edge: the second card stacks below the first.
        let at = place_cards(&[(100.0, 100.0), (110.0, 105.0)], &[120.0, 120.0], pane);
        assert_eq!(at[0], (112.0, 112.0));
        assert!(at[1].1 >= at[0].1 + 120.0, "{at:?}");
        // With room on the left, the second card goes there instead of down the stack.
        let at = place_cards(&[(300.0, 100.0), (300.0, 100.0)], &[120.0, 120.0], pane);
        assert_eq!(at[1], (300.0 - 12.0 - CARD_W, 112.0));
        // Near the right and bottom edges the card flips to the left and up.
        let at = place_cards(&[(790.0, 590.0)], &[120.0], pane);
        assert_eq!(at[0], (790.0 - 12.0 - CARD_W, 590.0 - 12.0 - 120.0));
        // A stack that would leave the bottom goes to the other side.
        let at = place_cards(&[(300.0, 400.0), (300.0, 400.0)], &[150.0, 150.0], pane);
        let (a, b) = (at[0], at[1]);
        let apart = b.0 + CARD_W <= a.0 || b.0 >= a.0 + CARD_W || b.1 >= a.1 + 150.0 || b.1 + 150.0 <= a.1;
        assert!(apart && b.1 + 150.0 <= pane.1, "{at:?}");
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
