//! Part definition library (DESIGN.md §4.1, §5): loads TOML part definitions from the built-in
//! and user libraries and answers search queries.
//!
//! shortcut: an in-memory token index instead of §5's SQLite FTS5 (rusqlite is not in the offline
//! registry); a linear scan over a few hundred parts is well under a millisecond. Upgrade if the
//! library grows to tens of thousands of parts.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use lab_core::part::{ConnectorKind, PartDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    User,
}

#[derive(Clone, Debug)]
pub struct LibraryDir {
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub struct CatalogWarning {
    pub path: PathBuf,
    pub message: String,
}

struct Entry {
    def: PartDef,
    source: Source,
    /// Lowercase searchable words.
    words: Vec<String>,
}

#[derive(Default)]
pub struct Catalog {
    /// Sorted by category, then name.
    entries: Vec<Entry>,
    by_id: HashMap<String, usize>,
}

impl Catalog {
    /// Reads `<dir>/parts/**/*.toml`; later dirs override earlier ones by id. Bad files are
    /// skipped with a warning, never a panic.
    pub fn load(dirs: &[LibraryDir]) -> (Catalog, Vec<CatalogWarning>) {
        let mut warnings = Vec::new();
        let mut defs: HashMap<String, (PartDef, Source)> = HashMap::new();
        for dir in dirs {
            let mut files = Vec::new();
            collect_toml(&dir.path.join("parts"), &mut files);
            files.sort();
            for path in files {
                let parsed = std::fs::read_to_string(&path)
                    .map_err(|e| format!("The file could not be read: {e}."))
                    .and_then(|text| {
                        toml::from_str::<PartDef>(&text)
                            .map_err(|e| format!("The part definition is invalid: {}.", e.message()))
                    });
                match parsed {
                    Ok(def) => {
                        defs.insert(def.id.clone(), (def, dir.source));
                    }
                    Err(message) => warnings.push(CatalogWarning { path, message }),
                }
            }
        }
        (Self::build(defs.into_values().collect()), warnings)
    }

    /// [`builtin_dir`] then [`user_dir`] (if it exists).
    pub fn load_default() -> (Catalog, Vec<CatalogWarning>) {
        let mut dirs = vec![LibraryDir { path: builtin_dir(), source: Source::BuiltIn }];
        if let Some(path) = user_dir().filter(|p| p.is_dir()) {
            dirs.push(LibraryDir { path, source: Source::User });
        }
        Self::load(&dirs)
    }

    pub fn from_defs(defs: Vec<PartDef>) -> Catalog {
        Self::build(defs.into_iter().map(|d| (d, Source::BuiltIn)).collect())
    }

    fn build(defs: Vec<(PartDef, Source)>) -> Catalog {
        let mut entries: Vec<Entry> = defs
            .into_iter()
            .map(|(def, source)| Entry { words: words_of(&def), def, source })
            .collect();
        entries.sort_by(|a, b| {
            (&a.def.category, &a.def.name, &a.def.id).cmp(&(&b.def.category, &b.def.name, &b.def.id))
        });
        entries.dedup_by(|b, a| a.def.id == b.def.id);
        let by_id = entries.iter().enumerate().map(|(i, e)| (e.def.id.clone(), i)).collect();
        Catalog { entries, by_id }
    }

    pub fn get(&self, id: &str) -> Option<&PartDef> {
        self.by_id.get(id).map(|&i| &self.entries[i].def)
    }

    pub fn source(&self, id: &str) -> Option<Source> {
        self.by_id.get(id).map(|&i| self.entries[i].source)
    }

    /// Sorted by category, then name.
    pub fn all(&self) -> Vec<&PartDef> {
        self.entries.iter().map(|e| &e.def).collect()
    }

    /// Sorted, unique, full paths.
    pub fn categories(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.entries.iter().map(|e| e.def.category.as_str()).collect();
        set.into_iter().map(str::to_owned).collect()
    }

    /// `"posts"` matches `"posts"` and `"posts/holders"`, not `"postsx"`.
    pub fn in_category(&self, prefix: &str) -> Vec<&PartDef> {
        let prefix = prefix.trim_end_matches('/');
        self.all()
            .into_iter()
            .filter(|d| {
                d.category
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
            .collect()
    }

    /// Case-insensitive; every whitespace token must prefix-match a word of name, vendor, part
    /// number, id, tags, category or connector kinds. Exact part-number match ranks first, then
    /// name-prefix matches, then the `all()` order. Empty query = all().
    pub fn search(&self, query: &str, limit: usize) -> Vec<&PartDef> {
        let query = query.trim().to_lowercase();
        let tokens: Vec<&str> = query.split_whitespace().collect();
        let mut hits: Vec<(u8, &PartDef)> = self
            .entries
            .iter()
            .filter(|e| tokens.iter().all(|t| e.words.iter().any(|w| w.starts_with(t))))
            .map(|e| {
                let rank = if !query.is_empty() && e.def.part_number.to_lowercase() == query {
                    0
                } else if !query.is_empty() && e.def.name.to_lowercase().starts_with(&query) {
                    1
                } else {
                    2
                };
                (rank, &e.def)
            })
            .collect();
        hits.sort_by_key(|(rank, _)| *rank); // stable: keeps all() order within a rank
        hits.into_iter().take(limit).map(|(_, d)| d).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Whole lowercase field values plus their pieces split at anything but letters, digits, `-`, `.`.
fn words_of(def: &PartDef) -> Vec<String> {
    let mut fields: Vec<&str> = vec![&def.name, &def.vendor, &def.part_number, &def.id, &def.category];
    fields.extend(def.tags.iter().map(String::as_str));
    for c in &def.connectors {
        match &c.kind {
            ConnectorKind::Thread { spec, .. } => fields.extend(["thread", spec.as_str()]),
            ConnectorKind::Bore { .. } => fields.push("bore"),
            ConnectorKind::OpticBore { .. } => fields.push("optic_bore"),
            ConnectorKind::HoleGridSeat => fields.push("hole_grid_seat"),
            ConnectorKind::CageRod { .. } => fields.push("cage_rod"),
            ConnectorKind::LensTube { thread, .. } => fields.extend(["lens_tube", thread.as_str()]),
        }
    }
    let mut words = BTreeSet::new();
    for field in fields {
        let field = field.to_lowercase();
        for w in field.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '.')) {
            if !w.is_empty() {
                words.insert(w.to_owned());
            }
        }
        words.insert(field);
    }
    words.into_iter().collect()
}

fn collect_toml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_toml(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml")) {
            out.push(path);
        }
    }
}

/// `LABSETUP_LIBRARY_DIR`, else `<exe dir>/library`, else `<CARGO_MANIFEST_DIR>/../../library`.
pub fn builtin_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LABSETUP_LIBRARY_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("library")))
        .filter(|p| p.is_dir())
    {
        return dir;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library")
}

/// `%APPDATA%\LabSetup\library`.
pub fn user_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("LabSetup").join("library"))
}

/// The stored GLB for `blake3:<hex>`: the user library first, then the built-in one.
pub fn mesh_path(asset: &str) -> Option<(Source, PathBuf)> {
    // The hex check keeps a hand-edited asset id from reaching outside `meshes/`.
    let hex = asset.strip_prefix("blake3:").filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let file = format!("meshes/{hex}.glb");
    user_dir()
        .map(|d| (Source::User, d.join(&file)))
        .into_iter()
        .chain([(Source::BuiltIn, builtin_dir().join(&file))])
        .find(|(_, p)| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KM100: &str = include_str!("testdata/km100.toml");

    #[test]
    fn user_overrides_builtin_and_search_finds_it() {
        let root = std::env::temp_dir().join(format!("lab-catalog-test-{}", std::process::id()));
        let builtin = root.join("builtin");
        let user = root.join("user");
        for (dir, text) in [
            (&builtin, KM100.to_owned()),
            (&user, KM100.replace("name = \"Kinematic", "name = \"User Kinematic")),
        ] {
            std::fs::create_dir_all(dir.join("parts/mounts")).unwrap();
            std::fs::write(dir.join("parts/mounts/km100.toml"), text).unwrap();
        }
        std::fs::write(builtin.join("parts/broken.toml"), "id = ").unwrap();

        let (cat, warnings) = Catalog::load(&[
            LibraryDir { path: builtin.clone(), source: Source::BuiltIn },
            LibraryDir { path: user.clone(), source: Source::User },
        ]);
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(cat.len(), 1);
        let def = cat.get("thorlabs:KM100").unwrap();
        assert!(def.name.starts_with("User Kinematic"));
        assert_eq!(cat.source("thorlabs:KM100"), Some(Source::User));
        assert_eq!(def.connectors.len(), 2);
        assert_eq!(cat.categories(), vec!["mounts/kinematic".to_owned()]);
        assert_eq!(cat.in_category("mounts").len(), 1);
        assert_eq!(cat.in_category("mount").len(), 0);
        for q in ["km1", "thorlabs mirror", "8-32", "kinematic", ""] {
            assert_eq!(cat.search(q, 10).len(), 1, "query {q:?}");
        }
        assert!(cat.search("thorlabs lens", 10).is_empty());
    }
}
