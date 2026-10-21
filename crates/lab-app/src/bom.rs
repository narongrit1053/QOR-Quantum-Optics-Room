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
    let mut add = |vendor: String, part_number: String, name: String, spec: String| {
        map.entry((vendor.clone(), part_number.clone(), spec.clone()))
            .or_insert(BomRow { vendor, part_number, name, spec, quantity: 0 })
            .quantity += 1;
    };
    for node in &doc.nodes {
        let spec = node.params.iter().map(|(k, v)| format!("{k}={}", fmt_param(v))).collect::<Vec<_>>().join("; ");
        match catalog.get(&node.part) {
            Some(def) => {
                let vendor = if def.vendor.is_empty() { "Generic".to_string() } else { def.vendor.clone() };
                let number = if def.part_number.is_empty() {
                    node.part.split_once(':').map_or(node.part.as_str(), |(_, id)| id).to_string()
                } else {
                    def.part_number.clone()
                };
                add(vendor, number, def.name.clone(), spec);
            }
            None => add("Unknown".into(), node.part.clone(), node.part.clone(), spec),
        }
    }
    for platform in &doc.platforms {
        let (min, max) = platform.outline.bounds();
        let spec = format!("{}×{}×{} mm", max.x - min.x, max.y - min.y, platform.thickness);
        // Platforms carry no preset id or vendor in the model.
        add("Generic".into(), "custom".into(), platform.name.clone(), spec);
    }
    map.into_values().collect()
}

fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() }
}

/// RFC 4180: header `Vendor,Part number,Name,Spec,Quantity`, fields with `,` `"` or newlines
/// quoted, CRLF line ends, UTF-8 with BOM (Excel). Pure.
pub fn to_csv(rows: &[BomRow]) -> String {
    let mut out = String::from("\u{feff}Vendor,Part number,Name,Spec,Quantity\r\n");
    for r in rows {
        let q = r.quantity.to_string();
        let line = [&r.vendor, &r.part_number, &r.name, &r.spec, &q].map(|f| field(f)).join(",");
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
                id: NodeId(doc.nodes.len() as u64 + 1),
                name: part.into(),
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
        let r = rows(&doc, &catalog);
        assert_eq!(r.len(), 4);
        assert_eq!(r.iter().find(|r| r.part_number == "KM100").unwrap().quantity, 2);
        let csv = to_csv(&r);
        assert!(csv.contains("\"Post, 1\"\"\""));
        assert!(csv.starts_with("\u{feff}Vendor,"));
    }
}
