//! Loading projects: schema check, step-wise migration of older formats, repair and validation
//! (M1-SPEC §3.11).

use glam::DVec2;
use serde_json::{Map, Value, json};

use crate::holes::{GridOrigin, HoleGrid, Thread, lattice_origin};
use crate::model::{Document, SCHEMA_VERSION};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    /// The schema version the file was converted from, if it was older.
    pub migrated_from: Option<u32>,
    /// Notes about values that were converted or repaired.
    pub repairs: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a LabSetup project ({0})")]
    NotAProject(String),
    #[error("saved by a newer LabSetup (format {found}); this version reads up to format {supported}")]
    NewerSchema { found: u32, supported: u32 },
    #[error("the project file is damaged: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not convert from format {from}: {detail}")]
    Migration { from: u32, detail: String },
    #[error("the project contains invalid data: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

/// Converts a project value from schema `from` to `from + 1`, adding notes for changed values.
type Step = fn(&mut Value, &mut Vec<String>) -> Result<(), String>;

const MIGRATIONS: &[(u32, Step)] = &[(1, v1_to_v2)];

/// The file's schema version: an integer from 1 to SCHEMA_VERSION.
fn schema_version(value: &Value) -> Result<u32, LoadError> {
    let not_a_project = |why: &str| LoadError::NotAProject(why.to_string());
    let object = value.as_object().ok_or_else(|| not_a_project("the file does not contain a JSON object"))?;
    let version = object.get("schema_version").ok_or_else(|| not_a_project("the format version is missing"))?;
    let version = version.as_u64().ok_or_else(|| not_a_project("the format version is not a whole number"))?;
    if version == 0 {
        return Err(not_a_project("the format version is 0"));
    }
    if version > u64::from(SCHEMA_VERSION) {
        return Err(LoadError::NewerSchema {
            found: u32::try_from(version).unwrap_or(u32::MAX),
            supported: SCHEMA_VERSION,
        });
    }
    Ok(version as u32)
}

/// Brings `value` up to SCHEMA_VERSION, one step at a time; each step sets
/// `schema_version = from + 1`. Returns the original version if it was older.
pub fn migrate(value: &mut Value, notes: &mut Vec<String>) -> Result<Option<u32>, LoadError> {
    let original = schema_version(value)?;
    let mut version = original;
    for &(from, step) in MIGRATIONS {
        if from != version {
            continue;
        }
        step(value, notes).map_err(|detail| LoadError::Migration { from, detail })?;
        version = from + 1;
        if let Some(object) = value.as_object_mut() {
            object.insert("schema_version".into(), json!(version));
        }
    }
    if version != SCHEMA_VERSION {
        return Err(LoadError::Migration { from: version, detail: "no conversion is available".into() });
    }
    Ok((original != SCHEMA_VERSION).then_some(original))
}

/// Parse, check the schema, migrate, deserialize, repair, validate.
pub(crate) fn load(json: &str) -> Result<(Document, LoadReport), LoadError> {
    let mut value: Value = serde_json::from_str(json)?;
    let mut notes = Vec::new();
    let migrated_from = migrate(&mut value, &mut notes)?;
    let mut doc: Document = serde_json::from_value(value)?;
    notes.extend(doc.repair());
    doc.validate().map_err(LoadError::Invalid)?;
    Ok((doc, LoadReport { migrated_from, repairs: notes }))
}

fn pair(value: Option<&Value>) -> Option<(f64, f64)> {
    match value?.as_array()?.as_slice() {
        [x, y] => Some((x.as_f64()?, y.as_f64()?)),
        _ => None,
    }
}

fn number(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key)?.as_f64()
}

/// M0 (schema 1) to M1 (schema 2): a rectangular room becomes an outline with walls; each
/// platform gets a kind, a surface and a rectangle outline; each grid gets a 2D pitch and the
/// origin that reproduces the M0 holes and labels (Auto when it coincides, else pinned).
fn v1_to_v2(doc: &mut Value, notes: &mut Vec<String>) -> Result<(), String> {
    let doc = doc.as_object_mut().ok_or("the project is not a JSON object")?;

    let room = doc.get("room").and_then(Value::as_object).ok_or("the room is missing")?;
    let (w, h) = pair(room.get("size")).ok_or("the room size is missing or invalid")?;
    let height = number(room, "height").ok_or("the room height is missing or invalid")?;
    let wall_height = height.clamp(500.0, 10_000.0);
    if wall_height != height {
        notes.push(format!(
            "The room height of {height} mm was changed to {wall_height} mm (walls are 500–10000 mm high)."
        ));
    }
    let (hx, hy) = (w / 2.0, h / 2.0);
    doc.insert(
        "room".into(),
        json!({
            "outline": [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]],
            "wall_height": wall_height,
            "wall_thickness": 150.0,
            "openings": [],
        }),
    );

    let platforms = doc.get_mut("platforms").and_then(Value::as_array_mut).ok_or("the platform list is missing")?;
    for (k, platform) in platforms.iter_mut().enumerate() {
        let n = k + 1;
        let p = platform.as_object_mut().ok_or_else(|| format!("platform {n} is not an object"))?;
        let table = p.get("support").is_some_and(|s| s.get("Legs").is_some());
        p.insert("kind".into(), json!(if table { "OpticalTable" } else { "Breadboard" }));
        p.insert("surface".into(), json!(if table { "Stainless" } else { "BlackAnodized" }));
        let (w, h) = pair(p.get("size")).ok_or_else(|| format!("the size of platform {n} is missing or invalid"))?;
        p.remove("size");
        p.insert("outline".into(), json!({ "Rectangle": { "size": [w, h] } }));

        let grid = match p.get("hole_grid") {
            None | Some(Value::Null) => Value::Null,
            Some(Value::Object(g)) => {
                let bad = || format!("the hole grid of platform {n} is invalid");
                let pitch = number(g, "pitch").ok_or_else(bad)?;
                let margin = number(g, "margin").ok_or_else(bad)?;
                let thread = g.get("thread").cloned().ok_or_else(bad)?;
                let bounds = (DVec2::new(-w / 2.0, -h / 2.0), DVec2::new(w / 2.0, h / 2.0));
                // The thread does not affect where the lattice sits.
                let probe = HoleGrid {
                    pitch: DVec2::splat(pitch),
                    thread: Thread::M6,
                    margin,
                    origin: GridOrigin::Auto,
                    exclusions: Vec::new(),
                };
                let m0 = DVec2::new(-w / 2.0 + margin, -h / 2.0 + margin);
                let auto = lattice_origin(&probe, bounds);
                let origin =
                    if (auto - m0).abs().max_element() <= 1e-9 { json!("Auto") } else { json!({ "At": [m0.x, m0.y] }) };
                json!({
                    "pitch": [pitch, pitch],
                    "thread": thread,
                    "margin": margin,
                    "origin": origin,
                    "exclusions": [],
                })
            }
            Some(_) => return Err(format!("the hole grid of platform {n} is invalid")),
        };
        p.insert("hole_grid".into(), grid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1(platform_extra: Value) -> Value {
        let mut platform = json!({
            "id": 1,
            "name": "Board",
            "size": [600.0, 610.0],
            "thickness": 12.7,
            "top_height": 12.7,
            "position": [0.0, 0.0],
            "rotation_z": 0.0,
            "hole_grid": { "pitch": 25.0, "margin": 12.5, "thread": "M6" },
            "support": "Fixed",
        });
        if let (Some(p), Some(extra)) = (platform.as_object_mut(), platform_extra.as_object()) {
            for (k, v) in extra {
                p.insert(k.clone(), v.clone());
            }
        }
        json!({
            "schema_version": 1,
            "name": "Old",
            "room": { "size": [6000.0, 5000.0], "height": 20000.0 },
            "platforms": [platform],
            "nodes": [],
            "next_id": 2,
        })
    }

    #[test]
    fn v1_breadboard_with_an_uneven_grid_is_pinned() {
        // 610 mm does not fit whole pitches symmetrically: the M0 origin is pinned.
        let value = v1(json!({}));
        let (doc, report) = load(&value.to_string()).unwrap();
        assert_eq!(report.migrated_from, Some(1));
        assert_eq!(report.repairs.len(), 1, "{:?}", report.repairs);
        assert_eq!(doc.room.wall_height, 10_000.0);
        let p = &doc.platforms[0];
        assert_eq!(p.kind, crate::model::PlatformKind::Breadboard);
        assert_eq!(p.surface, crate::model::Surface::BlackAnodized);
        assert_eq!(p.hole_grid.as_ref().unwrap().origin, GridOrigin::At(DVec2::new(-287.5, -292.5)));
        let holes = crate::holes::compute_holes(p).unwrap();
        assert_eq!(holes.hole_at(crate::holes::HoleRef { col: 0, row: 0 }), Some(DVec2::new(-287.5, -292.5)));
        assert_eq!((holes.cols, holes.rows), (24, 24));
    }

    #[test]
    fn v1_platform_without_grid() {
        let value = v1(json!({ "hole_grid": null }));
        let (doc, _) = load(&value.to_string()).unwrap();
        assert_eq!(doc.platforms[0].hole_grid, None);
    }

    #[test]
    fn broken_v1_values_fail_the_migration() {
        let mut value = v1(json!({}));
        value["room"] = json!({ "size": [1.0] });
        assert!(matches!(load(&value.to_string()), Err(LoadError::Migration { from: 1, .. })));
        let value = v1(json!({ "hole_grid": 5 }));
        let err = load(&value.to_string()).unwrap_err();
        assert_eq!(err.to_string(), "could not convert from format 1: the hole grid of platform 1 is invalid");
    }

    #[test]
    fn schema_checks() {
        let check = |text: &str| load(text).unwrap_err();
        assert!(matches!(check("[1, 2]"), LoadError::NotAProject(_)));
        assert!(matches!(check(r#"{"schema_version": "2"}"#), LoadError::NotAProject(_)));
        assert!(matches!(check(r#"{"schema_version": 0}"#), LoadError::NotAProject(_)));
        assert!(matches!(check(r#"{"schema_version": -1}"#), LoadError::NotAProject(_)));
        assert!(matches!(check(r#"{"schema_version": 1.5}"#), LoadError::NotAProject(_)));
        assert!(matches!(
            check(r#"{"schema_version": 99999999999}"#),
            LoadError::NewerSchema { found: u32::MAX, supported: 2 }
        ));
        // Current schema, wrong shape: a damaged file.
        assert!(matches!(check(r#"{"schema_version": 2}"#), LoadError::Json(_)));
    }
}
