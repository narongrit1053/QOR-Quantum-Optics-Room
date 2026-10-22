//! M1–M5: loading the M0 (schema 1) demo fixture written by the untouched M0 code.

use lab_core::glam::{DVec2, DVec3};
use lab_core::sample::demo_document;
use lab_core::*;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/demo_v1.json");

fn load_fixture() -> (Document, LoadReport) {
    Document::from_json(FIXTURE).expect("the v1 fixture loads")
}

#[test]
fn m1_fixture_loads_and_validates() {
    let (doc, report) = load_fixture();
    assert_eq!(report.migrated_from, Some(1));
    assert_eq!(report.repairs, vec!["Converted from an older format.".to_string()]);
    assert_eq!(doc.schema_version, SCHEMA_VERSION);
    assert_eq!(doc.validate(), Ok(()));
    assert_eq!(doc.room, Room::default());
    assert_eq!(doc.next_free_id(), 41);
    let table = &doc.platforms[0];
    assert_eq!(table.name, "Table A");
    assert_eq!(table.kind, PlatformKind::OpticalTable);
    assert_eq!(table.surface, Surface::Stainless);
    assert_eq!(table.outline, Outline::Rectangle { size: DVec2::new(1200.0, 900.0) });
    assert_eq!(table.support, Support::Legs { size: 120.0, inset: 120.0 });
    let grid = table.hole_grid.as_ref().unwrap();
    assert_eq!(grid.origin, GridOrigin::Auto);
    assert_eq!(grid.pitch, DVec2::splat(25.0));
    assert_eq!(grid.standard(), GridStandard::Metric);
    // The migrated document is the M1 demo, which since M6 traces its beams instead of drawing
    // them (POLA-101): same platform, same nodes minus the drawn beams.
    let demo = demo_document();
    assert_eq!(doc.platforms, demo.platforms);
    let names = |d: &Document| d.nodes.iter().filter(|n| n.part != "beam").map(|n| (n.id, n.name.clone())).collect::<Vec<_>>();
    assert_eq!(names(&doc), names(&demo));
}

#[test]
fn m2_node_positions_are_unchanged() {
    let raw: Value = serde_json::from_str(FIXTURE).unwrap();
    let (doc, _) = load_fixture();
    let nodes = raw["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), doc.nodes.len());
    for (v, node) in nodes.iter().zip(&doc.nodes) {
        assert_eq!(v["id"].as_u64(), Some(node.id.0));
        let p: Vec<f64> = v["position"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        assert_eq!(DVec3::new(p[0], p[1], p[2]), node.position, "{}", node.name);
        assert_eq!(v["name"].as_str(), Some(node.name.as_str()));
    }

}

#[test]
fn m3_holes_and_labels_match_the_m0_grid() {
    let (doc, _) = load_fixture();
    let holes = compute_holes(&doc.platforms[0]).unwrap();
    assert_eq!(holes.len(), 1728);
    assert_eq!((holes.cols, holes.rows), (48, 36));
    let (size, margin, pitch) = (DVec2::new(1200.0, 900.0), 12.5, 25.0);
    for row in 0..36u32 {
        for col in 0..48u32 {
            // M0 `HoleGrid::hole_position`.
            let m0 =
                DVec2::new(-size.x / 2.0 + margin + col as f64 * pitch, -size.y / 2.0 + margin + row as f64 * pitch);
            let label = HoleRef { col, row };
            let p = holes.hole_at(label).expect("every M0 hole exists");
            assert!((p - m0).abs().max_element() <= 1e-9, "{label}: {p} vs {m0}");
            // The hole at the M0 position carries the M0 label.
            assert_eq!(holes.label(holes.nearest(m0).unwrap()), Some(label));
        }
    }
    assert!(!holes.contains_label(HoleRef { col: 48, row: 0 }));
    assert!(!holes.contains_label(HoleRef { col: 0, row: 36 }));
}

#[test]
fn m4_load_errors() {
    let mut newer: Value = serde_json::from_str(FIXTURE).unwrap();
    newer["schema_version"] = Value::from(4);
    match Document::from_json(&newer.to_string()) {
        Err(LoadError::NewerSchema { found: 4, supported: 3 }) => {}
        other => panic!("{other:?}"),
    }

    let mut missing: Value = serde_json::from_str(FIXTURE).unwrap();
    missing.as_object_mut().unwrap().remove("schema_version");
    assert!(matches!(Document::from_json(&missing.to_string()), Err(LoadError::NotAProject(_))));

    let truncated = &FIXTURE[..FIXTURE.len() / 2];
    match Document::from_json(truncated) {
        Err(LoadError::Json(e)) => {
            assert!(e.line() > 1);
            assert!(LoadError::Json(e).to_string().contains("line"));
        }
        other => panic!("{other:?}"),
    }

    let mut invalid: Value = serde_json::from_str(&demo_document().to_json().unwrap()).unwrap();
    invalid["platforms"][0]["outline"] = serde_json::json!({ "Rectangle": { "size": [10.0, 900.0] } });
    match Document::from_json(&invalid.to_string()) {
        Err(LoadError::Invalid(problems)) => {
            assert_eq!(problems, vec!["Platform “Table A”: size must be between 50 and 20000 mm".to_string()]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn m5_round_trip_is_a_fixed_point() {
    let (doc, _) = load_fixture();
    let first = doc.to_json().unwrap();
    let (again, report) = Document::from_json(&first).unwrap();
    assert_eq!(report, LoadReport::default());
    assert_eq!(again, doc);
    assert_eq!(again.to_json().unwrap(), first);
}

#[test]
fn repairs_are_reported() {
    let mut value: Value = serde_json::from_str(&demo_document().to_json().unwrap()).unwrap();
    value["next_id"] = Value::from(5);
    let (doc, report) = Document::from_json(&value.to_string()).unwrap();
    assert_eq!(report.migrated_from, None);
    assert_eq!(report.repairs.len(), 1);
    assert_eq!(doc.next_free_id(), demo_document().next_free_id());
}

#[test]
fn v2_nodes_migrate_to_parents() {
    let mut value: Value = serde_json::from_str(&demo_document().to_json().unwrap()).unwrap();
    value["schema_version"] = Value::from(2);
    let table = demo_document().platforms[0].id.0;
    for node in value["nodes"].as_array_mut().unwrap() {
        let n = node.as_object_mut().unwrap();
        for key in ["parent", "attach", "params", "locked"] {
            n.remove(key);
        }
        n.insert("platform".into(), Value::from(table));
    }
    let (doc, report) = Document::from_json(&value.to_string()).unwrap();
    assert_eq!(report.migrated_from, Some(2));
    assert_eq!(report.repairs, vec!["Converted from an older format.".to_string()]);
    assert_eq!(doc, demo_document());
}
