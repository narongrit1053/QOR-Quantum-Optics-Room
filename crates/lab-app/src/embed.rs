//! Parts and meshes embedded in a project file, so it opens on another machine (POLA-68).

use std::collections::BTreeSet;

use lab_catalog::{Catalog, Source, mesh_path, user_dir};
use lab_core::Document;
use lab_core::part::{Geometry, PartDef};
use lab_file::{EmbeddedFile, atomic_write};

/// `parts/<id with ':' and '/' → '_'>.toml`.
fn toml_name(id: &str) -> String {
    format!("parts/{}.toml", id.replace([':', '/', '\\'], "_"))
}

/// For every distinct `node.part` from the user library: its def as TOML and, for a mesh part,
/// the GLB. Plus `thumbnail.png` if `thumbnail` is Some.
pub fn collect(doc: &Document, catalog: &Catalog, thumbnail: Option<Vec<u8>>) -> Vec<EmbeddedFile> {
    let mut out = Vec::new();
    let ids: BTreeSet<&str> = doc.nodes.iter().map(|n| n.part.as_str()).collect();
    let mut meshes = BTreeSet::new();
    for id in ids {
        if catalog.source(id) != Some(Source::User) {
            continue;
        }
        let Some(def) = catalog.get(id) else { continue };
        let Ok(text) = toml::to_string(def) else { continue };
        out.push(EmbeddedFile { name: toml_name(id), data: text.into_bytes() });
        if let Geometry::Mesh { asset } = &def.geometry
            && let Some(hex) = asset.strip_prefix("blake3:")
            && meshes.insert(hex.to_string())
            && let Some((_, path)) = mesh_path(asset)
            && let Ok(data) = std::fs::read(&path)
        {
            out.push(EmbeddedFile { name: format!("parts/meshes/{hex}.glb"), data });
        }
    }
    if let Some(data) = thumbnail {
        out.push(EmbeddedFile { name: "thumbnail.png".into(), data });
    }
    out
}

/// Trust boundary: each TOML must parse as a PartDef; each GLB's BLAKE3 must equal its name.
/// Unknown ids go to `<user_dir>/parts/embedded/`, meshes to `<user_dir>/meshes/` (skipped if
/// present). The caller reloads the catalog when the count is non-zero. Returns how many parts
/// were added and the ids skipped because a local part with that id exists.
pub fn unpack(catalog: &Catalog, extras: &[EmbeddedFile]) -> (usize, Vec<String>) {
    let Some(user) = user_dir() else { return (0, Vec::new()) };
    for extra in extras {
        let Some(hex) = extra.name.strip_prefix("parts/meshes/").and_then(|f| f.strip_suffix(".glb")) else {
            continue;
        };
        let target = user.join("meshes").join(format!("{hex}.glb"));
        if target.is_file() || !blake3::hash(&extra.data).to_hex().eq_ignore_ascii_case(hex) {
            continue;
        }
        let _ = std::fs::create_dir_all(user.join("meshes"));
        let _ = atomic_write(&target, &extra.data);
    }
    let (mut added, mut skipped) = (0, Vec::new());
    for extra in extras.iter().filter(|e| e.name.starts_with("parts/") && e.name.ends_with(".toml")) {
        let Some(def) = std::str::from_utf8(&extra.data).ok().and_then(|t| toml::from_str::<PartDef>(t).ok()) else {
            continue;
        };
        if catalog.get(&def.id).is_some() {
            if catalog.get(&def.id) != Some(&def) {
                skipped.push(def.id);
            }
            continue;
        }
        let file = extra.name.trim_start_matches("parts/");
        let dir = user.join("parts").join("embedded");
        let _ = std::fs::create_dir_all(&dir);
        if atomic_write(&dir.join(file), &extra.data).is_ok() {
            added += 1;
        }
    }
    (added, skipped)
}
