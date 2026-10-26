//! Vendor STEP import (DESIGN.md §5, §13): runs the step2glb sidecar, merges and re-bases the
//! mesh, builds three LODs and stores the GLB under the library by its BLAKE3 content hash.

pub mod features;
pub mod fit;
mod glb;
mod lod;
pub mod snaps;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use glam::DVec3;
use lab_core::model::Finish;
use lab_core::part::PartDef;
use lab_geom::TriMesh;

use crate::glb::{Glb, Material};

/// LOD0 is the converter's own tessellation, kept undecimated whenever it fits: decimating a
/// fine mesh or meshing coarsely is what made curved surfaces facet and smear (POLA-152).
const LOD0_BUDGET: usize = 150_000;
const TIMEOUT: Duration = Duration::from_secs(60);
/// The coarse retry: reading a 30–50 MB STEP file alone takes about a minute.
const COARSE_TIMEOUT: Duration = Duration::from_secs(300);

pub struct ImportOptions {
    /// Up axis of the STEP model. `Y` for Thorlabs (SolidWorks), `Z` otherwise.
    pub source_up: SourceUp,
    /// Starting angular deflection in rad (default 0.1, ~5.7° per facet, as imagetostl.com uses).
    pub angular_deflection: f64,
    /// Applied to primitives that have no material (AP203 files). `None` = AnodizedBlack.
    pub finish: Option<Finish>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions { source_up: SourceUp::Y, angular_deflection: 0.1, finish: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceUp {
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Converting,
    Simplifying,
    Storing,
}

#[derive(Clone, Debug)]
pub struct Imported {
    pub asset: String,
    pub path: PathBuf,
    pub name: String,
    pub has_colours: bool,
    pub triangles: [usize; 3],
    pub size_mm: DVec3,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("The STEP converter was not found at {0}.")]
    SidecarMissing(PathBuf),
    #[error("The conversion took too long and was stopped.")]
    Timeout,
    #[error("{message}")]
    Failed { code: i32, message: String },
    #[error("The converted mesh could not be read: {0}.")]
    BadGlb(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// `LABSETUP_STEP2GLB`, else `step2glb.exe` next to the exe or in `<exe dir>/step2glb/`, else the
/// repo's `tools/step2glb/build/Release/step2glb.exe`; in a git worktree (where the build dir is
/// absent) the main checkout's copy.
pub fn step2glb_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LABSETUP_STEP2GLB") {
        return p.into();
    }
    let rel = Path::new("tools/step2glb/build/Release/step2glb.exe");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let exe_dir = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.to_path_buf()));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = &exe_dir {
        candidates.push(d.join("step2glb.exe"));
        candidates.push(d.join("step2glb").join("step2glb.exe"));
    }
    candidates.push(repo.join(rel));
    // A worktree's `.git` is a file "gitdir: <main>/.git/worktrees/<name>".
    if let Ok(git) = std::fs::read_to_string(repo.join(".git"))
        && let Some(main) = git.trim().strip_prefix("gitdir:").and_then(|g| Path::new(g.trim()).ancestors().nth(3))
    {
        candidates.push(main.join(rel));
    }
    candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| repo.join(rel))
}

/// Blocking; call it off the UI thread. Runs step2glb in a temp dir (60 s timeout, kill on
/// timeout), merges primitives by material, applies `finish` where there is none, clears
/// `doubleSided`, re-bases, builds LOD0..2 and stores the GLB by content hash under `library`
/// (an existing file with the same hash is reused).
pub fn import_step(
    step: &Path,
    library: &Path,
    opts: &ImportOptions,
    progress: &dyn Fn(Progress),
) -> Result<Imported, ImportError> {
    let exe = step2glb_path();
    if !exe.is_file() {
        return Err(ImportError::SidecarMissing(exe));
    }
    let tmp = TempDir::new()?;
    progress(Progress::Converting);

    // Coarser tessellation until LOD0 fits the budget; decimation is the last resort.
    let start = opts.angular_deflection;
    let mut tries = vec![start];
    tries.extend([0.2, 0.3, 0.5].into_iter().filter(|&a| a > start));
    let out = tmp.0.join("out.glb");
    let load = || -> Result<Merged, ImportError> {
        let glb = Glb::parse(&std::fs::read(&out)?).map_err(ImportError::BadGlb)?;
        merge(&glb, opts.finish.as_ref())
    };
    let mut merged = None;
    for a in tries {
        match run_step2glb(&exe, step, &out, a, None, opts.source_up, &tmp.0, TIMEOUT) {
            // The default 0.05 mm linear deflection meshes some detailed parts for minutes
            // (CT1, MS3-M, LX20-M; POLA-169): one coarse try, which takes seconds.
            Err(ImportError::Timeout) => {
                run_step2glb(&exe, step, &out, 0.5, Some(0.5), opts.source_up, &tmp.0, COARSE_TIMEOUT)?;
                merged = Some(load()?);
                break;
            }
            result => result?,
        }
        let m = load()?;
        let fits = m.triangles() <= LOD0_BUDGET;
        merged = Some(m);
        if fits {
            break;
        }
    }
    let mut m = merged.expect("at least one try");
    if m.parts.is_empty() {
        return Err(ImportError::Failed { code: 3, message: "The STEP file contains no shapes.".into() });
    }

    progress(Progress::Simplifying);
    let (min, max) = lod::bounds(&m.parts.iter().flat_map(|(_, t)| t.positions.iter().copied()).collect::<Vec<_>>());
    let shift = DVec3::new(-0.5 * (min.x + max.x), -0.5 * (min.y + max.y), -min.z);
    for (_, t) in &mut m.parts {
        t.positions.iter_mut().for_each(|p| *p += shift);
    }
    let lod0 = scale_lod(&m.parts, LOD0_BUDGET.min(m.triangles()));
    let n0 = count(&lod0);
    let lod1 = scale_lod(&lod0, n0 / 4);
    let lod2 = scale_lod(&lod0, n0 / 20);
    let lods = [lod0, lod1, lod2];

    progress(Progress::Storing);
    let bytes = glb::write(&m.materials, &lods);
    let hex = blake3::hash(&bytes).to_hex().to_string();
    let dir = library.join("meshes");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hex}.glb"));
    if !path.is_file() {
        let part = dir.join(format!("{hex}.glb.tmp"));
        std::fs::write(&part, &bytes)?;
        std::fs::rename(&part, &path)?;
    }
    let name = m.name.unwrap_or_else(|| step.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()));
    Ok(Imported {
        asset: format!("blake3:{hex}"),
        path,
        name,
        has_colours: m.has_colours,
        triangles: lods.each_ref().map(|l| count(l)),
        size_mm: max - min,
    })
}

/// The meshes of one LOD of a stored GLB, in part space (mm, +Z up): (baseColorFactor, mesh).
pub fn read_glb(path: &Path, lod: usize) -> Result<Vec<([f32; 4], TriMesh)>, ImportError> {
    let glb = Glb::parse(&std::fs::read(path)?).map_err(ImportError::BadGlb)?;
    let lod = lod.min(glb.scene_count().saturating_sub(1));
    Ok(glb
        .primitives(lod)
        .map_err(ImportError::BadGlb)?
        .into_iter()
        .map(|p| (p.material.map_or([1.0; 4], |m| m.color), p.mesh))
        .collect())
}

/// Writes `def` over the user-library file that already defines its id (anywhere under
/// `<library>/parts/`), else to `<library>/parts/imported/<id with ':' and '/' replaced by '_'>.toml`
/// (write to `.tmp`, then rename). Returns the path.
pub fn write_part(library: &Path, def: &PartDef) -> std::io::Result<PathBuf> {
    let dir = library.join("parts").join("imported");
    std::fs::create_dir_all(&dir)?;
    let path = find_part_file(&library.join("parts"), &def.id)
        .unwrap_or_else(|| dir.join(format!("{}.toml", def.id.replace([':', '/'], "_"))));
    let text = toml::to_string(def).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// The `.toml` under `dir` (recursively) whose `id` is `id`.
fn find_part_file(dir: &Path, id: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_part_file(&path, id) {
                return Some(found);
            }
        } else if path.extension().is_some_and(|e| e == "toml")
            && let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = text.parse::<toml::Table>()
            && value.get("id").and_then(|v| v.as_str()) == Some(id)
        {
            return Some(path);
        }
    }
    None
}

fn run_step2glb(
    exe: &Path,
    step: &Path,
    out: &Path,
    deflection: f64,
    linear: Option<f64>,
    up: SourceUp,
    tmp: &Path,
    timeout: Duration,
) -> Result<(), ImportError> {
    let err_path = tmp.join("stderr.txt");
    let mut cmd = Command::new(exe);
    cmd.arg(step).arg(out).arg("-a").arg(deflection.to_string());
    if let Some(l) = linear {
        cmd.arg("-l").arg(l.to_string());
    }
    cmd.arg("--source-up")
        .arg(if up == SourceUp::Y { "y" } else { "z" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_path)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ImportError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if status.success() {
        return Ok(());
    }
    let code = status.code().unwrap_or(-1);
    let stderr = std::fs::read_to_string(&err_path).unwrap_or_default();
    let line = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let message = match code {
        2 => "This is not a readable STEP file.".to_string(),
        3 => "The STEP file contains no shapes.".to_string(),
        4 => "The shapes in this STEP file could not be meshed.".to_string(),
        5 => "The converted mesh could not be written to the temporary folder.".to_string(),
        6 => "The STEP converter crashed on this file.".to_string(),
        _ => format!("The STEP converter failed (code {code}): {line}"),
    };
    Err(ImportError::Failed { code, message })
}

struct Merged {
    materials: Vec<Material>,
    /// (material index, mesh) — one entry per material.
    parts: Vec<(usize, TriMesh)>,
    has_colours: bool,
    name: Option<String>,
}

impl Merged {
    fn triangles(&self) -> usize {
        self.parts.iter().map(|(_, m)| m.triangle_count()).sum()
    }
}

fn merge(glb: &Glb, finish: Option<&Finish>) -> Result<Merged, ImportError> {
    let prims = glb.primitives(0).map_err(ImportError::BadGlb)?;
    let fallback = finish_material(finish.unwrap_or(&Finish::AnodizedBlack));
    let mut m = Merged { materials: Vec::new(), parts: Vec::new(), has_colours: true, name: glb.first_name() };
    for p in prims {
        if p.mesh.indices.is_empty() {
            continue;
        }
        let mat = p.material.unwrap_or_else(|| {
            m.has_colours = false;
            fallback
        });
        let i = match m.materials.iter().position(|x| *x == mat) {
            Some(i) => i,
            None => {
                m.materials.push(mat);
                m.parts.push((m.materials.len() - 1, TriMesh::default()));
                m.materials.len() - 1
            }
        };
        m.parts[i].1.append(&p.mesh);
    }
    Ok(m)
}

fn finish_material(f: &Finish) -> Material {
    match f {
        Finish::Stainless => Material { color: [0.75, 0.75, 0.75, 1.0], metallic: 1.0, roughness: 0.3 },
        Finish::Painted { rgb } => {
            // baseColorFactor is linear; the finish colour is sRGB.
            let lin = |c: u8| {
                let c = c as f32 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            Material { color: [lin(rgb[0]), lin(rgb[1]), lin(rgb[2]), 1.0], metallic: 0.0, roughness: 0.5 }
        }
        _ => Material { color: [0.05, 0.05, 0.05, 1.0], metallic: 0.6, roughness: 0.45 },
    }
}

fn count(parts: &[(usize, TriMesh)]) -> usize {
    parts.iter().map(|(_, m)| m.triangle_count()).sum()
}

/// Decimates every material's mesh to its share of `target`; drops meshes that vanish.
fn scale_lod(parts: &[(usize, TriMesh)], target: usize) -> Vec<(usize, TriMesh)> {
    let total = count(parts).max(1);
    parts
        .iter()
        .map(|(i, m)| (*i, lod::decimate(m, m.triangle_count() * target / total)))
        .filter(|(_, m)| !m.indices.is_empty())
        .collect()
}

/// A private temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!("labsetup-import-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_core::part::{ConnectorDef, ConnectorKind, Frame, Geometry};

    /// LOD0 is step2glb's 0.1 rad mesh itself, not a decimated copy (POLA-152: imagetostl.com
    /// writes 111,558 triangles for this file from the same OpenCascade mesher at 0.1 rad).
    #[test]
    fn km100_lod0_is_undecimated() {
        let step = Path::new("D:/Try/LabSetup/Test GLB/ThorLabs - KM100.stp");
        if !step2glb_path().is_file() || !step.is_file() {
            eprintln!("skipped: step2glb or the sample is missing");
            return;
        }
        let lib = TempDir::new().unwrap();
        let a = import_step(step, &lib.0, &ImportOptions::default(), &|_| {}).unwrap();
        assert!((100_000..=LOD0_BUDGET).contains(&a.triangles[0]), "{:?}", a.triangles);
        let mut mesh = TriMesh::default();
        read_glb(&a.path, 0).unwrap().iter().for_each(|(_, m)| mesh.append(m));
        let t = Instant::now();
        let features = crate::features::detect(&mesh);
        eprintln!("triangles {:?}, {} features in {:?}", a.triangles, features.len(), t.elapsed());
    }

    #[test]
    fn imports_fmmp1() {
        let step = Path::new("D:/dev/step-samples/FMMP1_M-Step.step");
        if !step2glb_path().is_file() || !step.is_file() {
            eprintln!("skipped: step2glb or the sample is missing");
            return;
        }
        let lib = TempDir::new().unwrap();
        let opts = ImportOptions::default();
        let a = import_step(step, &lib.0, &opts, &|_| {}).unwrap();
        let [t0, t1, t2] = a.triangles;
        assert!(t0 <= LOD0_BUDGET && t1 < t0 && t2 < t1, "{:?}", a.triangles);
        assert!((a.size_mm.z - 75.9).abs() <= 0.5, "{}", a.size_mm);
        assert!(a.asset.starts_with("blake3:") && a.path.is_file());
        let lod0 = read_glb(&a.path, 0).unwrap();
        assert_eq!(lod0.iter().map(|(_, m)| m.triangle_count()).sum::<usize>(), t0);
        assert!(!a.has_colours);
        let b = import_step(step, &lib.0, &opts, &|_| {}).unwrap();
        assert_eq!(a.asset, b.asset);

        let def = PartDef {
            id: "thorlabs:FMMP1/M".into(),
            version: 1,
            vendor: "Thorlabs".into(),
            part_number: "FMMP1/M".into(),
            name: a.name.clone(),
            category: "imported".into(),
            tags: vec![],
            geometry: Geometry::Mesh { asset: a.asset.clone() },
            connectors: vec![ConnectorDef {
                name: "seat".into(),
                kind: ConnectorKind::HoleGridSeat,
                frame: Frame { origin: DVec3::ZERO, z: -DVec3::Z, x: DVec3::X },
            }],
            optical_ports: vec![],
            meta: [("finish".to_string(), "anodized_black".to_string())].into(),
            features: Vec::new(),
        };
        let p = write_part(&lib.0, &def).unwrap();
        assert!(p.ends_with("parts/imported/thorlabs_FMMP1_M.toml"));
        assert_eq!(toml::from_str::<PartDef>(&std::fs::read_to_string(&p).unwrap()).unwrap(), def);
        // A part saved by hand elsewhere under parts/ is rewritten in place, not duplicated.
        let own = lib.0.join("parts").join("mine.toml");
        std::fs::rename(&p, &own).unwrap();
        assert_eq!(write_part(&lib.0, &def).unwrap(), own);
        assert!(!p.exists());
    }
}
