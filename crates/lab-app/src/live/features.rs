//! Named features (connectors and detected planes/axes) for the live commands; the bin supplies
//! detection through `FeatureLookup` (it owns the mesh cache).

use lab_core::glam::DVec3;
use lab_core::part::{ConnectorDef, ConnectorKind, Feature, Gender, PartDef};
use lab_core::{HoleRef, HoleSet, Node, Platform};

pub trait FeatureLookup {
    fn detected(&self, node: &Node, def: &PartDef) -> Vec<Feature>;
}

pub struct NamedFeature {
    pub id: String,
    pub feature: Feature,
    pub description: String,
    /// "connector" or "detected".
    pub source: &'static str,
}

/// Cage rod radius (mm).
const CAGE_ROD_RADIUS: f64 = 3.0;

/// Nominal thread radius in mm; 0.0 for specs not in the table.
fn thread_radius(spec: &str) -> f64 {
    match spec {
        "M4" => 2.0,
        "M6" => 3.0,
        "8-32" => 2.08,
        "1/4-20" => 3.175,
        _ => 0.0,
    }
}

fn lens_tube_radius(thread: &str) -> f64 {
    match thread {
        "SM05" => 6.9,
        "SM1" => 12.95,
        "SM2" => 25.4,
        _ => 0.0,
    }
}

pub fn connector_feature(c: &ConnectorDef) -> Feature {
    let (origin, dir) = (c.frame.origin, c.frame.z);
    let axis = |radius: f64, gender: Gender| Feature::Axis { origin, dir, radius, hole: gender == Gender::Female };
    match &c.kind {
        ConnectorKind::Bore { diameter, gender, .. } | ConnectorKind::OpticBore { diameter, gender } => axis(diameter / 2.0, *gender),
        ConnectorKind::Thread { spec, gender } => axis(thread_radius(spec), *gender),
        ConnectorKind::LensTube { thread, gender } => axis(lens_tube_radius(thread), *gender),
        ConnectorKind::CageRod { gender, .. } => axis(CAGE_ROD_RADIUS, *gender),
        ConnectorKind::HoleGridSeat => Feature::Plane { origin, normal: dir },
    }
}

fn centi(v: f64) -> i64 {
    (v * 100.0).round() as i64
}

/// Planes before axes, then origin z descending, y ascending, x ascending (0.01 mm), then radius.
fn sort_key(f: &Feature) -> (u8, i64, i64, i64, i64) {
    match f {
        Feature::Plane { origin, .. } => (0, -centi(origin.z), centi(origin.y), centi(origin.x), 0),
        Feature::Axis { origin, radius, .. } => (1, -centi(origin.z), centi(origin.y), centi(origin.x), centi(*radius)),
    }
}

/// Rounded to 0.01 so that tiny negatives do not print as "-0.00".
fn num(v: f64) -> f64 {
    (v * 100.0).round() / 100.0 + 0.0
}

fn fmt_point(p: DVec3) -> String {
    format!("({:.2}, {:.2}, {:.2})", num(p.x), num(p.y), num(p.z))
}

fn fmt_dir(d: DVec3) -> String {
    let d = d.normalize_or_zero();
    for (axis, name) in [(DVec3::X, "X"), (DVec3::Y, "Y"), (DVec3::Z, "Z")] {
        if (d - axis).length() < 1e-6 {
            return format!("+{name}");
        }
        if (d + axis).length() < 1e-6 {
            return format!("-{name}");
        }
    }
    fmt_point(d)
}

pub fn describe(f: &Feature) -> String {
    match f {
        Feature::Plane { origin, normal } => format!("plane, normal {}, through {}", fmt_dir(*normal), fmt_point(*origin)),
        Feature::Axis { origin, dir, radius, hole } => format!(
            "{} axis \u{00d8}{:.2} along {}, at {}",
            if *hole { "hole" } else { "shaft" },
            num(radius * 2.0),
            fmt_dir(*dir),
            fmt_point(*origin)
        ),
    }
}

/// Connectors first (id = connector name), then the detected features sorted and numbered `f1…`.
pub fn node_features(node: &Node, def: &PartDef, lookup: &dyn FeatureLookup) -> Vec<NamedFeature> {
    let connectors = def.connectors.iter().map(|c| {
        let feature = connector_feature(c);
        NamedFeature { id: c.name.clone(), description: describe(&feature), feature, source: "connector" }
    });
    let mut detected = lookup.detected(node, def);
    detected.sort_by_key(sort_key);
    let detected = detected.into_iter().enumerate().map(|(i, feature)| NamedFeature {
        id: format!("f{}", i + 1),
        description: describe(&feature),
        feature,
        source: "detected",
    });
    connectors.chain(detected).collect()
}

/// `"top"` and `"hole:<label>"` on a platform, in its frame.
pub fn platform_features(_platform: &Platform, holes: &HoleSet) -> impl Fn(&str) -> Option<Feature> + use<> {
    let radius = holes.thread_radius;
    let holes = holes.clone();
    move |id| match id {
        "top" => Some(Feature::Plane { origin: DVec3::ZERO, normal: DVec3::Z }),
        _ => {
            let at = holes.hole_at(id.strip_prefix("hole:")?.parse::<HoleRef>().ok()?)?;
            Some(Feature::Axis { origin: at.extend(0.0), dir: DVec3::Z, radius, hole: true })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::part::Frame;

    struct Stub(Vec<Feature>);
    impl FeatureLookup for Stub {
        fn detected(&self, _: &Node, _: &PartDef) -> Vec<Feature> {
            self.0.clone()
        }
    }

    fn def() -> PartDef {
        toml::from_str(
            r#"
id = "test:p"
name = "P"
category = "x"
[geometry]
kind = "parametric"
generator = "riser"
params = { length = 1, width = 1, height = 1 }
[[connectors]]
name = "bore"
kind = "bore"
diameter = 12.7
gender = "female"
frame = { origin = [0, 0, 20], z = [0, 0, 1], x = [1, 0, 0] }
[[connectors]]
name = "seat"
kind = "hole_grid_seat"
frame = { origin = [0, 0, 0], z = [0, 0, -1], x = [1, 0, 0] }
"#,
        )
        .unwrap()
    }

    fn node() -> Node {
        crate::live::resolve::new_part_node(
            &lab_catalog::Catalog::from_defs(vec![def()]),
            "test:p",
            1,
            lab_core::Parent::Room,
            lab_core::Attach::Free,
            DVec3::ZERO,
            lab_core::glam::DQuat::IDENTITY,
        )
        .unwrap()
    }

    #[test]
    fn features_are_stable_and_described() {
        let plane_hi = Feature::Plane { origin: DVec3::new(0.0, 0.0, 12.5), normal: DVec3::Z };
        let plane_lo = Feature::Plane { origin: DVec3::new(0.0, 0.0, 2.0), normal: DVec3::new(1.0, 0.0, 1.0) };
        let shaft = Feature::Axis { origin: DVec3::new(0.0, 0.0, 12.5), dir: DVec3::Z, radius: 3.0, hole: false };
        let stub = Stub(vec![shaft.clone(), plane_lo.clone(), plane_hi.clone()]);
        let got = node_features(&node(), &def(), &stub);
        let ids: Vec<_> = got.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["bore", "seat", "f1", "f2", "f3"]);
        assert_eq!(got[0].source, "connector");
        assert_eq!(got[2].source, "detected");
        assert_eq!(got[2].feature, plane_hi);
        assert_eq!(got[3].feature, plane_lo);
        assert_eq!(got[4].feature, shaft);
        assert_eq!(got[2].description, "plane, normal +Z, through (0.00, 0.00, 12.50)");
        assert_eq!(got[3].description, "plane, normal (0.71, 0.00, 0.71), through (0.00, 0.00, 2.00)");
        assert_eq!(got[4].description, "shaft axis \u{00d8}6.00 along +Z, at (0.00, 0.00, 12.50)");
        assert_eq!(got[0].description, "hole axis \u{00d8}12.70 along +Z, at (0.00, 0.00, 20.00)");
        assert_eq!(got[1].description, "plane, normal -Z, through (0.00, 0.00, 0.00)");
    }

    #[test]
    fn connector_features_map_kinds() {
        let d = def();
        let f = connector_feature(d.connector("bore").unwrap());
        assert_eq!(f, Feature::Axis { origin: DVec3::new(0.0, 0.0, 20.0), dir: DVec3::Z, radius: 6.35, hole: true });
        assert!(matches!(connector_feature(d.connector("seat").unwrap()), Feature::Plane { .. }));
        let frame = Frame { origin: DVec3::ZERO, z: DVec3::Z, x: DVec3::X };
        let thread =
            ConnectorDef { name: "t".into(), kind: ConnectorKind::Thread { spec: "1/4-20".into(), gender: Gender::Male }, frame };
        assert!(matches!(connector_feature(&thread), Feature::Axis { radius, hole: false, .. } if radius == 3.175));
    }
}
