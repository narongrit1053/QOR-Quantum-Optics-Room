//! Bill of materials: the project's parts grouped by vendor, part number and spec, exported as CSV.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use lab_catalog::Catalog;
use lab_core::model::Document;
use lab_core::part::ParamValue;
use lab_file::atomic_write;
use lab_ui::AppWindow;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::controller;
use crate::dialogs::{self, DialogKind, DialogResult};

pub struct BomRow {
    pub vendor: String,
    pub part_number: String,
    pub name: String,
    /// Node names that differ from the part's default name, sorted, joined with `, `.
    pub labels: String,
    pub spec: String,
    pub quantity: u32,
}

fn fmt_param(value: &ParamValue) -> String {
    match value {
        ParamValue::Number(v) => v.to_string(),
        ParamValue::Text(s) => s.clone(),
        ParamValue::Flag(b) => b.to_string(),
    }
}

/// One row per (vendor, part_number, spec); sorted by vendor, part number, spec. Pure.
pub fn rows(doc: &Document, catalog: &Catalog) -> Vec<BomRow> {
    let mut map: BTreeMap<(String, String, String), BomRow> = BTreeMap::new();
    let mut labels: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let mut add = |vendor: String, part_number: String, name: String, spec: String, label: Option<&str>| {
        let key = (vendor.clone(), part_number.clone(), spec.clone());
        if let Some(l) = label.filter(|l| *l != name) {
            labels.entry(key.clone()).or_default().push(l.to_string());
        }
        map.entry(key)
            .or_insert(BomRow { vendor, part_number, name, labels: String::new(), spec, quantity: 0 })
            .quantity += 1;
    };
    for node in &doc.nodes {
        // Beams are drawn, not bought.
        let def = catalog.get(&node.part).or_else(|| legacy(&node.part).and_then(|id| catalog.get(id)));
        if node.part == "beam" || def.is_some_and(|d| d.category.starts_with("beam")) {
            continue;
        }
        let spec = node.params.iter().map(|(k, v)| format!("{k}={}", fmt_param(v))).collect::<Vec<_>>().join("; ");
        let id = node.part.split_once(':').map_or(node.part.as_str(), |(_, id)| id);
        match def {
            Some(def) => {
                let vendor = if def.vendor.is_empty() { "Generic".to_string() } else { def.vendor.clone() };
                let number = if def.part_number.is_empty() {
                    def.id.split_once(':').map_or(def.id.as_str(), |(_, id)| id).to_string()
                } else {
                    def.part_number.clone()
                };
                add(vendor, number, def.name.clone(), spec, Some(&node.name));
            }
            None => add("Generic".into(), id.to_string(), readable(id), spec, Some(&node.name)),
        }
    }
    for platform in &doc.platforms {
        let (min, max) = platform.outline.bounds();
        let size = format!("{}×{}", max.x - min.x, max.y - min.y);
        let spec = format!("{size}×{} mm", platform.thickness);
        // Platforms carry no preset id or vendor in the model.
        add("Generic".into(), String::new(), format!("{} {size}", platform.name), spec, None);
    }
    for (key, mut l) in labels {
        l.sort();
        if let Some(row) = map.get_mut(&key) {
            row.labels = l.join(", ");
        }
    }
    map.into_values().collect()
}

/// Built-in parts for the `generic:*` ids of older documents (the demo).
fn legacy(id: &str) -> Option<&'static str> {
    Some(match id {
        "generic:post-holder-40" => "thorlabs:PH40/M",
        "generic:base-25x75" => "thorlabs:BA1/M",
        "generic:kinematic-mount-1in" => "thorlabs:KM100/M",
        "generic:half-wave-plate-1in" => "generic:hwp-1in",
        "generic:lens-1in-f100" => "generic:lens-1in",
        _ => return None,
    })
}

/// `post-holder-40` → `Post holder 40`.
fn readable(id: &str) -> String {
    let text = id.replace(['-', '_'], " ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() }
}

/// RFC 4180: header `Vendor,Part number,Name,Labels,Spec,Quantity`, fields with `,` `"` or newlines
/// quoted, CRLF line ends, UTF-8 with BOM (Excel). Pure.
pub fn to_csv(rows: &[BomRow]) -> String {
    let mut out = String::from("\u{feff}Vendor,Part number,Name,Labels,Spec,Quantity\r\n");
    for r in rows {
        let q = r.quantity.to_string();
        let line = [&r.vendor, &r.part_number, &r.name, &r.labels, &r.spec, &q].map(|f| field(f)).join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

/// Save dialog (`*.csv`, default `<project name> BOM.csv`), atomic write via lab-file's helper, toast.
pub fn export(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (suggested, initial_dir) = {
        let a = app.borrow();
        (format!("{} BOM.csv", a.session.title()), a.settings.last_dir.clone())
    };
    let (ui_weak, app) = (ui.as_weak(), app.clone());
    dialogs::show_file_dialog_local(
        dialogs::owner_hwnd(ui.window()),
        DialogKind::SaveCsv { suggested },
        initial_dir,
        move |result| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let text = match result {
                DialogResult::Chosen(path) => {
                    let csv = {
                        let a = app.borrow();
                        to_csv(&rows(&a.session.doc, &a.catalog))
                    };
                    match atomic_write(&path, csv.as_bytes()) {
                        Ok(()) => format!("Exported {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into())),
                        Err(_) => "The BOM could not be written.".to_string(),
                    }
                }
                DialogResult::Cancelled => return,
                DialogResult::Failed(message) => message,
            };
            controller::show_toast(&ui, &app.borrow(), &text, false);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::glam::{DQuat, DVec3};
    use lab_core::model::{Attach, Finish, Node, NodeId, Parent, Shape};
    use lab_core::part::{Geometry, PartDef};

    fn def(id: &str, vendor: &str, number: &str, name: &str) -> PartDef {
        PartDef {
            id: id.into(),
            version: 1,
            vendor: vendor.into(),
            part_number: number.into(),
            name: name.into(),
            category: "x".into(),
            tags: vec![],
            geometry: Geometry::Mesh { asset: "a".into() },
            connectors: vec![],
            optical_ports: vec![],
            meta: Default::default(),
        }
    }

    #[test]
    fn groups_and_quotes() {
        let catalog = Catalog::from_defs(vec![
            def("generic:post", "", "", "Post, 1\""),
            def("thorlabs:KM100", "Thorlabs", "KM100", "Mount"),
        ]);
        let mut doc = Document::new("t");
        let mut add = |part: &str, len: Option<f64>| {
            let mut n = Node {
                note: String::new(),
                id: NodeId(doc.nodes.len() as u64 + 1),
                name: if part == "thorlabs:KM100" { "Mount".into() } else { part.into() },
                part: part.into(),
                parent: Parent::Room,
                attach: Attach::Free,
                position: DVec3::ZERO,
                rotation: DQuat::IDENTITY,
                shape: Shape::Part,
                finish: Finish::Stainless,
                params: Default::default(),
                pickable: true,
                locked: false,
            };
            if let Some(l) = len {
                n.params.insert("length".into(), ParamValue::Number(l));
            }
            doc.nodes.push(n);
        };
        add("generic:post", Some(50.0));
        add("generic:post", Some(75.0));
        add("thorlabs:KM100", None);
        add("thorlabs:KM100", None);
        add("mystery:x", None);
        doc.nodes[3].name = "M3".into();
        let r = rows(&doc, &catalog);
        assert_eq!(r.iter().find(|r| r.part_number == "KM100").unwrap().labels, "M3");
        assert_eq!(r.len(), 4);
        assert!(r.iter().any(|r| r.vendor == "Generic" && r.name == "X"));
        assert_eq!(r.iter().find(|r| r.part_number == "KM100").unwrap().quantity, 2);
        let csv = to_csv(&r);
        assert!(csv.contains("\"Post, 1\"\"\""));
        assert!(csv.starts_with("\u{feff}Vendor,Part number,Name,Labels,Spec,Quantity\r\n"));
    }

    #[test]
    fn demo_has_no_beams_or_unknowns() {
        let catalog = Catalog::load_default().0;
        let mut doc = lab_core::sample::demo_document();
        let mut ph3 = doc.nodes.iter().find(|n| n.part != "beam").unwrap().clone();
        ph3.part = "thorlabs:PH3".into();
        ph3.params.clear();
        doc.nodes.push(ph3);
        let r = rows(&doc, &catalog);
        let seen: Vec<_> = r.iter().map(|r| (&r.vendor, &r.part_number, &r.name)).collect();
        assert!(r.iter().all(|r| r.vendor != "Unknown" && r.part_number != "beam"), "{seen:?}");
        assert!(r.iter().any(|r| r.vendor == "Thorlabs" && r.part_number == "PH3"), "{seen:?}");
        assert!(r.iter().any(|r| r.vendor == "Thorlabs" && r.part_number == "PH40/M"), "{seen:?}");
    }
}
