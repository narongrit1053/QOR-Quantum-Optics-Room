# M4 plan: Vendor import

Milestone M4 of DESIGN.md §11, built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus its one happy-path check), and other flaws go to the Defect log. Read this file and your own section; open only the files your task touches.

**Exit criterion (§11):** import a KM100 + one laser head and mount them correctly.

## What already exists (do not rebuild)

- `tools/step2glb` (C++/OpenCascade sidecar, DESIGN §13): built at `tools/step2glb/build/Release/step2glb.exe` with its 34 DLLs. Options `-a <rad>`, `--source-up y|z`; output mm, +Z up; exit codes 2 (bad file), 3 (no shapes), 5 (bad output dir), 6 (crash); writes `<out>.partial`, then renames. Sample files: `D:/dev/step-samples/{FMMP1A3_M,FMMP1_M,RS99_M}-Step.step` (Thorlabs, Y-up; RS99/M has colours, the other two none).
- `tools/step2glb/viewport-spike.diff`: the working M0 recipe for GLBs in the viewport (features, transform, wake-up). Read it, do not apply it blindly.
- `PartDef` / `Geometry::Mesh { asset }` / `ConnectorDef` / `OpticalPort` (`lab-core/src/part.rs`), `Finish` (`lab-core/src/model.rs:439`), `TriMesh` (`lab-geom/src/mesh.rs`).
- `Catalog::load_default()` reads the built-in library, then `%APPDATA%\LabSetup\library` (`lab_catalog::user_dir()`), user overriding built-in by id. The app loads it once in `main.rs:72` and sends `ViewportRequest::SetCatalog`; a new catalog makes the viewport redraw the document.
- `dialogs.rs` `show_file_dialog` (native open/save dialogs), the library sidebar (`lab-app/src/library.rs`), the generic laser head (`library/parts/lasers/heads/generic-laser-head.toml`: `hole_grid_seat` at the bottom centre, `source` port at the exit).

## Decisions (recorded on POLA-33)

- **No meshopt:** it is not in the offline registry. LOD0 ≤ 30k triangles comes from step2glb's angular deflection (0.5 rad default; if over budget, re-run at 0.8, then 1.2 rad); LOD1 (~25%) and LOD2 (~5%) come from a pure-Rust **vertex-clustering** decimator in `lab-import`. No quantization (logged as a shortcut).
- **No `gltf` crate in `lab-import`:** step2glb writes a small, known GLB subset (triangle primitives, POSITION/NORMAL, u16/u32 indices, `baseColorFactor` materials, node TRS/matrix). Read and write that subset by hand with `serde_json`.
- **Content hash = BLAKE3** (`blake3` is already in `Cargo.lock` via Bevy; sha2 is not in the registry). Asset ids are `blake3:<64 hex>`; files are `<library>/meshes/<hex>.glb`. DESIGN §4.1's `sha256:` example is updated in Hardening.
- **Re-basing:** every import is translated so its bounding-box bottom centre is the part origin (the PartDef convention); no rotation. Direction of a laser beam is recorded in its port frame, not by rotating the mesh.
- **Bevy features:** POLA-58 alone adds `bevy_gltf` and `reflect_auto_register` to the workspace Bevy list (DESIGN §13, board-approved). This is the only feature change in M4; expect a ~30 min rebuild once.
- **Test files:** no KM100 or laser-head STEP exists on this machine. Engineers check with the three samples (FMMP1/M stands in for a mount, RS99/M for a coloured part). If `D:/dev/step-samples` gains `KM100*.step` or a MOGLabs/Toptica head, use it. The real KM100 + laser-head walkthrough is part of the M5 Hardening §12 run.

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 2 | POLA-57 | `lab-import`: STEP → stored mesh with LODs | Engineer Opus | `crates/lab-import/**` (new crate), its line in the workspace `Cargo.toml` members/dependencies |
| 2 | POLA-58 | GLB parts in the viewport | Engineer Opus Low | Bevy feature list in workspace `Cargo.toml`, `lab-viewport/src/glb.rs` (new), the `Geometry::Mesh` arm in `scene.rs`, asset sources in `host.rs`, `lab_catalog::mesh_path` |
| 3 | POLA-59 | Import flow UI | Engineer Opus Low | `lab-ui/ui/import.slint` (new), `lab-app/src/import.rs` (new), import button + dialog host in `app.slint`, `controller.rs` wiring, `library.rs` reload, `place.rs` finish for meshes |
| 3 | POLA-60 | Connector wizard | Engineer Opus | `lab-import/src/fit.rs` (new), `lab-viewport/src/inspect.rs` (new), `InspectPart`/`InspectMarkers`/`PartPoint` in `lab-viewport/src/lib.rs`, `lab-ui/ui/wizard.slint` (new), `lab-app/src/wizard.rs` (new) |
| 3 | POLA-61 | Offscreen thumbnails | Engineer Opus Low | `lab-viewport/src/thumbnail.rs` (new), `RenderThumbnail`/`ThumbnailReady` in `lib.rs`, `lab-app/src/thumbs.rs` (new), thumbnail image in the library sidebar item (`lab-ui`) |
| 4 | POLA-62 | Laser-head template | Engineer Opus Low | "Laser head" template in `wizard.rs`/`wizard.slint`, `riser` generator in `lab-parts`, laser placement in `lab-app/src/place.rs` |

Tiers checked: the defaults hold. POLA-57 (new crate, GLB parsing, decimation, process control) and POLA-60 (geometry fitting plus a new viewport mode) are the hard ones; POLA-58 has a working spike to follow; POLA-61 is a contained Bevy render-to-image with the exact mechanism given below.

**Merges (Engineer Sonnet).** Stage 2 touches the workspace `Cargo.toml` twice (members/deps from POLA-57, Bevy features from POLA-58): keep both. Stage 3 adds variants to `lab-viewport/src/lib.rs` (POLA-60, POLA-61) and items to `app.slint`/`controller.rs` (all three): keep both sides (`Integration fix:` commit if needed).

## Shared interfaces

### `crates/lab-import/src/lib.rs` (POLA-57; depends on `lab-core`, `lab-geom`, `serde_json`, `toml`, `blake3`, `thiserror`; no Bevy, no UI)

```rust
pub struct ImportOptions {
    /// Up axis of the STEP model. `Y` for Thorlabs (SolidWorks), `Z` otherwise.
    pub source_up: SourceUp,
    /// Starting angular deflection in rad (default 0.5).
    pub angular_deflection: f64,
    /// Applied to primitives that have no material (AP203 files). `None` = AnodizedBlack.
    pub finish: Option<lab_core::model::Finish>,
}
impl Default for ImportOptions { /* Y, 0.5, None */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceUp { Y, Z }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress { Converting, Simplifying, Storing }

#[derive(Clone, Debug)]
pub struct Imported {
    pub asset: String,           // "blake3:<64 hex>"
    pub path: std::path::PathBuf, // <library>/meshes/<hex>.glb
    pub name: String,            // first STEP product/node name, else the file stem
    pub has_colours: bool,       // false = the finish was applied
    pub triangles: [usize; 3],   // LOD0, LOD1, LOD2
    pub size_mm: glam::DVec3,    // bounding-box extents after re-basing
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("The STEP converter was not found at {0}.")] SidecarMissing(std::path::PathBuf),
    #[error("The conversion took longer than 60 seconds and was stopped.")] Timeout,
    #[error("{message}")] Failed { code: i32, message: String }, // step2glb's stderr line, made user-readable per exit code
    #[error("The converted mesh could not be read: {0}.")] BadGlb(String),
    #[error("{0}")] Io(#[from] std::io::Error),
}

/// `LABSETUP_STEP2GLB`, else `<exe dir>/step2glb/step2glb.exe`, else
/// `<CARGO_MANIFEST_DIR>/../../tools/step2glb/build/Release/step2glb.exe`.
pub fn step2glb_path() -> std::path::PathBuf;

/// Blocking; call it off the UI thread. Runs step2glb in a temp dir (60 s timeout, kill on
/// timeout), merges primitives by material, applies `finish` where there is none, clears
/// `doubleSided`, re-bases, builds LOD0..2 and stores the GLB by content hash under `library`
/// (an existing file with the same hash is reused).
pub fn import_step(step: &Path, library: &Path, opts: &ImportOptions, progress: &dyn Fn(Progress))
    -> Result<Imported, ImportError>;

/// The meshes of one LOD of a stored GLB, in part space (mm, +Z up): (baseColorFactor, mesh).
pub fn read_glb(path: &Path, lod: usize) -> Result<Vec<([f32; 4], lab_geom::TriMesh)>, ImportError>;

/// Writes `def` to `<library>/parts/imported/<id with ':' and '/' replaced by '_'>.toml`
/// (write to `.tmp`, then rename). Returns the path.
pub fn write_part(library: &Path, def: &lab_core::part::PartDef) -> std::io::Result<PathBuf>;
```

**Stored GLB layout:** three scenes, scene *i* = LOD *i*, each one root node with one mesh (one primitive per material). Materials: `baseColorFactor`, `metallicFactor`, `roughnessFactor`; `doubleSided` false. Units mm, +Z up, origin at the bottom centre.

### `lab-catalog` (POLA-58)

```rust
/// The stored GLB for `blake3:<hex>`: the user library first, then the built-in one.
pub fn mesh_path(asset: &str) -> Option<(Source, PathBuf)>;
```

### `crates/lab-viewport/src/lib.rs` (each task adds only its own lines; a new request gets its `FrameRequest` twin and is forwarded like the others)

```rust
pub enum ViewportRequest {
    /* … existing … */
    /// POLA-60. Show only catalog part `id` at the origin, unrotated, framed; the document is
    /// hidden but kept. `None` returns to the document and restores the camera.
    InspectPart(Option<String>),
    /// POLA-60. Connector/port frames (part space, mm) drawn as small axis arrows while inspecting.
    InspectMarkers(Vec<lab_core::part::Frame>),
    /// POLA-61. Render catalog part `part` offscreen (256×256, transparent background, 3/4 view)
    /// and write a PNG to `path`. Queued; at most one per frame.
    RenderThumbnail { part: String, path: std::path::PathBuf },
}

pub enum ViewportEvent {
    /* … existing … */
    /// POLA-60. A click on the inspected part: hit point and surface normal, part space, mm.
    PartPoint { point: glam::DVec3, normal: glam::DVec3 },
    /// POLA-61. The PNG for `part` is written (`ok` false on failure; the app skips it then).
    ThumbnailReady { part: String, path: std::path::PathBuf, ok: bool },
}
```

### `lab-app` (stage 3, used by POLA-62)

```rust
// lab-app/src/library.rs (POLA-59): reload the catalog from disk, send SetCatalog, refresh the
// sidebar model. Called after an import and after the wizard saves.
pub fn reload_catalog(state: &AppState) /* use the existing state type */;

// lab-app/src/wizard.rs (POLA-60): open the connector wizard on a user-library part.
pub fn open(state: &AppState, part_id: &str);
```

POLA-59's finish screen has a "Define connectors…" button that calls a Slint callback `import-define-connectors(string)` (declared by POLA-59 in `app.slint`); POLA-60 connects it to `wizard::open` in `controller.rs`. Until then the button only closes the dialog.

---

## Stage 2

### POLA-57 lab-import: STEP → stored mesh with LODs (Engineer Opus)
- New workspace crate `crates/lab-import` with the API above.
- Run step2glb with `std::process::Command` (no console window: `CREATE_NO_WINDOW`), args `<step> <tmp>/out.glb -a <deflection> --source-up <y|z>`; poll `try_wait` for 60 s, then `kill`. Map exit codes 2/3/5/6 to plain-language `Failed` messages ("This is not a readable STEP file.", "The STEP file contains no shapes.", …).
- Parse the GLB (subset above; apply node transforms), merge primitives by material, apply `finish` (AnodizedBlack ≈ `[0.05,0.05,0.05,1]` metallic 0.6 rough 0.45; Stainless `[0.75,0.75,0.75,1]` metallic 1 rough 0.3; Painted uses its rgb; others like AnodizedBlack). If LOD0 > 30k triangles, re-run step2glb at 0.8 then 1.2 rad; if still over, decimate LOD0 to 30k too.
- Decimator (`lod.rs`): vertex clustering on a uniform grid sized to reach the target triangle count (binary-search the cell size, at most 8 tries), drop degenerate triangles, recompute flat-ish normals per cluster. One unit test: a generated sphere of ~20k triangles reduces to ≤ 25% / ≤ 5% and keeps its bounding box within 2%.
- Write the GLB (layout above), hash with BLAKE3, store, return `Imported`. `write_part` as specified.
- `ImportError` messages are user-facing (shown in the import dialog): one sentence, no codes unless unknown.

**Works when:** a unit test (ignored if step2glb is missing) imports `FMMP1_M-Step.step` with `SourceUp::Y` into a temp library and gets LOD0 ≤ 30k, LOD1 < LOD0, LOD2 < LOD1, a 75.9 ± 0.5 mm tall `size_mm`, and `read_glb(path, 0)` returns the mesh; importing it twice yields the same asset id.

### POLA-58 GLB parts in the viewport (Engineer Opus Low)
- Add `bevy_gltf` and `reflect_auto_register` to the workspace Bevy features (only these two).
- `host.rs`: before the asset plugin is built, register two asset sources, `user://` → `lab_catalog::user_dir()` and `builtin://` → `lab_catalog::builtin_dir()` (`AssetSourceBuilder::platform_default`); create the user dir if missing.
- `lab_catalog::mesh_path` as specified.
- `glb.rs` + the `Geometry::Mesh` arm in `scene.rs`: for a `Shape::Part` node whose def is a mesh, spawn a child per LOD under the node entity: `WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(i).from_asset("<src>://meshes/<hex>.glb")))`, transform as in the spike (rotation × `Quat::from_rotation_x(-FRAC_PI_2)`, scale `MM`), each with `VisibilityRange` (LOD0 0–1.5 m, LOD1 1.5–4 m, LOD2 > 4 m, short crossfade margins). A missing file: log a warning, show nothing for that node (no panic).
- On `WorldInstanceReady`, tag every spawned mesh entity with the node's `PickId` and `SceneEntity` so picking, hover/selection tint, outline (POLA-50) and isolate (POLA-52) treat it like a generated part.
- Wake the on-demand loop from asset events (`AssetEvent::<Gltf>`/`<Mesh>` `LoadedWithDependencies`, plus `WorldInstanceReady`), then two settle frames; do not keep the loop alive during the load.

**Works when:** a hand-written user-library TOML pointing at a GLB written by POLA-57's test (or a step2glb output stored by hand under `meshes/`) can be placed on a table, renders upright at true scale against the 25 mm grid, can be picked and outlined, and the app returns to 0 frames after the load.

---

## Stage 3

### POLA-59 Import flow UI (Engineer Opus Low)
- Toolbar/library button "Import STEP…" → `show_file_dialog` with filter `*.step;*.stp` → modal `import.slint` dialog:
  1. Form: vendor (Thorlabs / MOGLabs / Toptica / Other; Thorlabs → up axis Y, else Z, editable), part number (default from the file name), name, category (default `imported`, editable path).
  2. Progress: `import_step` on a worker thread (`std::thread::spawn`), `Progress` mapped to "Converting… / Simplifying… / Saving…" via `slint::invoke_from_event_loop`; Cancel closes the dialog (the worker finishes and its result is dropped).
  3. Error: the `ImportError` text with "Choose another file".
  4. When `has_colours` is false the form shows a finish picker (Anodized black, Stainless, Painted) before conversion; send it as `ImportOptions::finish`.
  5. Done: writes `PartDef { id: "<vendor lowercase>:<part number>", geometry: Mesh { asset }, connectors: [hole_grid_seat at origin, z = -Z], meta: { "finish": …, "source": file name } }` with `write_part` into `user_dir()`, calls `reload_catalog`, selects the new part in the sidebar, and offers "Place" (closes) and "Define connectors…".
- `place.rs`: for `Geometry::Mesh` use the `meta["finish"]` value for `Node::finish` (fallback AnodizedBlack).

**Works when:** importing `RS99_M-Step.step` shows progress, then the part appears in the library under its category and can be dragged onto a table; picking a `.txt` renamed to `.step` shows "This is not a readable STEP file." and the app keeps running.

### POLA-60 Connector wizard (Engineer Opus)
- `wizard.slint`: a side panel (replaces the inspector while open) listing the part's connectors and optical ports, with Add / Remove / Save / Cancel. While open the viewport is in `InspectPart(Some(id))` and shows `InspectMarkers` of the current list.
- Click on the part (`PartPoint`): the panel asks "Hole / bore here" or "Optical face here".
  - **Hole/bore:** `fit::fit_hole(&lod0, point, normal) -> Option<HoleFit { axis: Frame, diameter: f64 }>`: take LOD0 triangles within 6 mm of the point, keep those whose normal is nearly perpendicular to the clicked face normal, the axis is the eigenvector of the smallest eigenvalue of Σ n·nᵀ, the centre a least-squares circle fit in the plane perpendicular to it; +Z points out of the material (toward the side the hole opens to). Suggest the thread by diameter (M4 3.3, 8-32 3.5, M6 5.0, ¼-20 5.1 mm tap drill; bores ≥ 6 mm → `bore`/`optic_bore`), user confirms kind, spec and gender. No fit → "No round hole found here."
  - **Optical face:** frame at the point, z = normal; user picks the kind (mirror, lens, …) and optional clear aperture.
- Save: rewrite the def with `write_part`, `reload_catalog`, `InspectPart(None)`. Opened from POLA-59's "Define connectors…" and from a user-library part's context menu ("Edit connectors…").
- Unit test in `fit.rs`: a synthetic plate with a Ø3.5 mm hole along +Z fits axis within 1° and diameter within 0.1 mm.

**Works when:** after importing FMMP1/M, clicking its tapped base hole suggests the thread, saving gives it an `8-32 female` (or the suggested) connector, and the part then snaps onto a post like the built-in KM100 does.

### POLA-61 Offscreen thumbnails (Engineer Opus Low)
- `thumbnail.rs`: on `RenderThumbnail`, spawn the part (generated or GLB) on its own `RenderLayers` layer with a dedicated camera (perspective, 3/4 view framing its bounds, transparent clear colour) rendering to a 256×256 `Image` target; once loaded (for GLBs wait for `WorldInstanceReady`), capture it with Bevy's `Screenshot` on that camera and, in the `ScreenshotCaptured` observer, write the PNG with the `png` crate (already in `Cargo.lock`; add it to `lab-viewport` deps), send `ThumbnailReady`, despawn everything. One at a time; the on-demand loop runs only while one is in flight.
- `lab-app/src/thumbs.rs`: at start, for each catalog part without `<user_dir>/thumbs/<id with ':' and '/' → '_'>.png`, queue `RenderThumbnail` (built-in thumbs also go to the user dir, the install dir is read-only); after an import or wizard save, re-render that part. The sidebar item shows the PNG (`slint::Image::load_from_path`) or the current placeholder.

**Works when:** on first start the library sidebar fills in thumbnails for the built-in parts, the second start renders none (files exist), and an imported part gets its thumbnail right after the import.

---

## Stage 4

### POLA-62 Laser-head template (Engineer Opus Low)
- In the wizard, a template choice "Generic / Laser head". Laser head asks, in order: click the beam exit point (`PartPoint`), direction (default the clicked face normal; buttons ±X/±Y to override), wavelength (nm). Save writes: `hole_grid_seat` at the origin (z = −Z), an optical port `{ name: "output", kind: "source", frame: exit point, z = direction }`, `category = "lasers/heads"`, tags `["laser", "source"]`, `meta.wavelength_nm`. Same shape as `generic-laser-head.toml`.
- `lab-parts`: a `riser` generator (box `length`, `width`, `height`; `hole_grid_seat` at the bottom) and `library/parts/posts/riser.toml`.
- `place.rs`: placing a part that has a `source` port on a platform: exit height `h` = port origin z. If the beam height `H` in effect (the stack builder's setting) exceeds `h` by more than 0.5 mm, place a riser of the head's footprint and height `H − h` and the head on top of it (two nodes, one undo step); if `h > H + 0.5` place it as is and show the existing toast "Beam at h mm, above the H mm beam height".

**Works when:** importing a STEP (any sample; a MOGLabs/Toptica head if present) with the Laser head template at an exit 40 mm up, then placing it on a table at 75 mm beam height, gives a 35 mm riser with the head on top and its `output` port at 75.0 mm.

---

## M4 done

After stage 4 merges, the Lead checks the exit criterion from the reports: a vendor mount imports with LODs and gets a thread connector that mates with a post (POLA-57/59/60), and an imported head mounts with its beam at the table's beam height (POLA-62), with thumbnails and GLB picking present. Hardening (M5) runs it with a real KM100 and laser-head STEP.
