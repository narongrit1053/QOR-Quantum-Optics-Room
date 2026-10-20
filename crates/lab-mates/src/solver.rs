//! Beam-height stack solver (DESIGN.md §4.4, docs/M2-PLAN.md POLA-40).

use std::fmt;

use lab_core::part::PartDef;

use crate::{compatible, slide_range};

#[derive(Clone, Debug, PartialEq)]
pub struct StackSolution {
    pub base: Option<String>,
    pub holder: String,
    pub post: String,
    pub slide: f64,
    pub margin: f64,
    pub beam_height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoFit {
    MountHasNoTap,
    NoCandidates,
    OutOfRange { closest: f64 },
}

impl fmt::Display for NoFit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoFit::MountHasNoTap => write!(f, "This mount has no base tap or optic seat, so it cannot go on a post."),
            NoFit::NoCandidates => write!(f, "No compatible post and holder are in the library."),
            NoFit::OutOfRange { closest } => {
                write!(f, "No post stack reaches this beam height; the closest is {closest:.1} mm.")
            }
        }
    }
}

fn top_z(part: &PartDef, conn: &str) -> Option<f64> {
    part.connector(conn).map(|c| c.frame.origin.z)
}

/// Shortest post first, then the largest margin, then ids in order.
pub fn solve_stack(mount: &PartDef, beam_height: f64, candidates: &[&PartDef]) -> Result<StackSolution, NoFit> {
    let (Some(seat), Some(tap)) = (top_z(mount, "optic_seat"), top_z(mount, "base_tap")) else {
        return Err(NoFit::MountHasNoTap);
    };
    let a = seat - tap;
    let of = |cat: &'static str| candidates.iter().copied().filter(move |p| p.category == cat);
    let bases: Vec<Option<&PartDef>> =
        std::iter::once(None).chain(of("posts/bases").filter(|b| b.connector("top").is_some()).map(Some)).collect();

    let mut best: Option<(f64, StackSolution)> = None;
    let mut closest: Option<f64> = None;
    for holder in of("posts/holders") {
        let Some(bore) = holder.connector("bore") else { continue };
        let (lo, hi) = slide_range(&bore.kind);
        let h = bore.frame.origin.z;
        for post in of("posts/posts") {
            let (Some(bottom), Some(l)) = (post.connector("bottom"), top_z(post, "top")) else { continue };
            if !compatible(&bottom.kind, &bore.kind) {
                continue;
            }
            for base in &bases {
                let b = match base {
                    None => 0.0,
                    Some(base) => {
                        let fits = matches!((base.connector("top"), holder.connector("bottom")),
                            (Some(t), Some(hb)) if compatible(&t.kind, &hb.kind));
                        if !fits {
                            continue;
                        }
                        base.connector("top").unwrap().frame.origin.z
                    }
                };
                let fixed = a + b + h + l;
                let slide = beam_height - fixed;
                let reach = slide.clamp(lo, hi) + fixed;
                if closest.is_none_or(|c| (reach - beam_height).abs() < (c - beam_height).abs()) {
                    closest = Some(reach);
                }
                if slide < lo || slide > hi {
                    continue;
                }
                let margin = (slide - lo).min(hi - slide);
                let sol = StackSolution {
                    base: base.map(|p| p.id.clone()),
                    holder: holder.id.clone(),
                    post: post.id.clone(),
                    slide,
                    margin,
                    beam_height,
                };
                let better = match &best {
                    None => true,
                    Some((bl, bs)) => (l, -margin, &sol.post, &sol.holder, &sol.base)
                        .partial_cmp(&(*bl, -bs.margin, &bs.post, &bs.holder, &bs.base))
                        == Some(std::cmp::Ordering::Less),
                };
                if better {
                    best = Some((l, sol));
                }
            }
        }
    }
    match (best, closest) {
        (Some((_, sol)), _) => Ok(sol),
        (None, Some(closest)) => Err(NoFit::OutOfRange { closest }),
        (None, None) => Err(NoFit::NoCandidates),
    }
}

#[cfg(test)]
mod tests {
    use lab_core::part::*;

    use super::*;

    use glam::DVec3;

    fn def(id: &str, category: &str, connectors: Vec<ConnectorDef>) -> PartDef {
        PartDef {
            id: id.into(),
            version: 1,
            vendor: String::new(),
            part_number: String::new(),
            name: id.into(),
            category: category.into(),
            tags: vec![],
            geometry: Geometry::Mesh { asset: String::new() },
            connectors,
            optical_ports: vec![],
            meta: Default::default(),
        }
    }

    fn conn(name: &str, kind: ConnectorKind, z: f64, dir: f64) -> ConnectorDef {
        ConnectorDef { name: name.into(), kind, frame: Frame { origin: DVec3::new(0.0, 0.0, z), z: DVec3::Z * dir, x: DVec3::X } }
    }

    fn thread(spec: &str, gender: Gender) -> ConnectorKind {
        ConnectorKind::Thread { spec: spec.into(), gender }
    }

    fn bore(gender: Gender, range: Option<[f64; 2]>) -> ConnectorKind {
        ConnectorKind::Bore { diameter: 12.0, gender, range }
    }

    #[test]
    fn km100_at_75mm() {
        use Gender::*;
        let mount = def("thorlabs:KM100", "mounts/kinematic", vec![
            conn("base_tap", thread("M4", Female), 0.0, -1.0),
            conn("optic_seat", ConnectorKind::OpticBore { diameter: 25.4, gender: Female }, 20.3, 1.0),
        ]);
        let post = |id: &str, l: f64| {
            def(id, "posts/posts", vec![conn("bottom", bore(Male, None), 0.0, -1.0), conn("top", thread("M4", Male), l, 1.0)])
        };
        let tr50 = post("thorlabs:TR50/M", 50.0);
        let tr75 = post("thorlabs:TR75/M", 75.0);
        let ph50 = def("thorlabs:PH50/M", "posts/holders", vec![
            conn("bottom", thread("M6", Female), 0.0, -1.0),
            conn("bore", bore(Female, Some([-28.0, -2.0])), 30.0, 1.0),
        ]);
        let ba1 = def("thorlabs:BA1/M", "posts/bases", vec![conn("top", thread("M6", Male), 9.5, 1.0)]);
        let all = [&tr75, &tr50, &ph50, &ba1];

        // No base: 75 − 20.3 − 30 − 50 = −25.3 ∈ [−28, −2]; with BA1 −34.8 and TR75 −50.3 are out.
        let s = solve_stack(&mount, 75.0, &all).unwrap();
        assert_eq!((s.base.as_deref(), s.holder.as_str(), s.post.as_str()), (None, "thorlabs:PH50/M", "thorlabs:TR50/M"));
        assert!((s.slide + 25.3).abs() < 1e-9 && (s.margin - 2.7).abs() < 1e-9, "{s:?}");

        // Highest reach: TR75 + BA1 at slide −2: 20.3 + 9.5 + 30 + 75 − 2 = 132.8.
        match solve_stack(&mount, 500.0, &all) {
            Err(NoFit::OutOfRange { closest }) => assert!((closest - 132.8).abs() < 1e-9, "{closest}"),
            r => panic!("{r:?}"),
        }
    }
}
