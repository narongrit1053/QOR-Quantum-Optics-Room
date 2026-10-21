# M5 plan: Polish & ship

Milestone M5 of DESIGN.md §11, built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus its one happy-path check), and other flaws go to the Defect log (POLA-74). Read this file and your own section; open only the files your task touches. Stages 4–8 are Hardening and are planned in their own issues.

**Exit criterion (§11):** meets the §1 non-functional targets on the lab PC (measured in Hardening 4, POLA-72).

## What already exists (do not rebuild)

- **Theme:** `lab-ui/ui/theme.slint` is one `export global Theme` of dark-only `out property` constants (surfaces, text, accent/status, fields, geometry, type, motion). About 26 colour literals live outside it (overlays 15, import 3, inspector 3, components 2, fields 2, app 1). Icons are text glyphs in `Theme.font-symbol`; a few inline `Path`s exist. No font or icon files are bundled.
- **Title bar:** `lab-app/src/main.rs` `style_title_bar(window)` sets the DWM dark mode, caption, border and text colours once, for dark only.
- **Actions:** `AppWindow` has one dispatcher callback `action(string)`. `controller.rs` `on_action` matches about 30 string ids (`file.*` goes to `files::handle_file_action`, the rest to `edit::action_edit`). Shortcuts are hard-coded in the `shortcuts` FocusScope (`app.slint:376-433`) and the viewport `keys` FocusScope (`app.slint:481-520`). There is no action registry.
- **Offscreen render:** `lab-viewport/src/thumbnail.rs` renders to an `Image` target, captures it with `Screenshot::image`, and writes the PNG in `write_png`. `host.rs` `apply_render_target` forces every `Camera3d` without `ThumbCamera` onto the Slint texture, so any new offscreen camera must be excluded there too. View modes: `ViewMode { Perspective, Top, Split }`. The main camera is the `OrbitCamera` without `TopPane`.
- **Settings:** `lab-file/src/settings.rs` `Settings { display_unit, wall_display, last_dir }` (all `#[serde(default)]`), `load_settings()` / `save_settings()`.
- **Project file:** `lab-file/src/project.rs` writes a zip with one entry `project.json` (or `<dir>/project.json`). `zip.rs` `write_zip(&[ZipEntryOut])` already takes several entries; `read_entry(archive, name, max_size)` reads one. `SaveOptions { backup_schema }` derives `Default`. `lab-app/src/files.rs` `submit_save` builds `SaveOptions` (line 419).
- **Catalog:** `Catalog::get(id)`, `Catalog::source(id) -> Option<Source>`, `user_dir()`, `builtin_dir()` (`LABSETUP_LIBRARY_DIR`, else `<exe dir>/library`, else the repo `library/`), `mesh_path(asset)`; `lab-app/src/library.rs` `reload_catalog(state)`. `lab_import::step2glb_path()` finds `<exe dir>/step2glb/step2glb.exe`.
- **Menus:** the project menu (`overlays.slint` `ProjectMenu`, opened from the project name) lists New, Open…, Open Demo, Save, Save As…, Save as Folder…. Any `file.*` id reaches `files::handle_file_action`.

## Decisions (recorded on POLA-34)

- **Inter is bundled from the Slint crate:** `i-slint-common-1.18.1/sharedfontique/Inter-VariableFont.ttf` (OFL) is copied to `library/fonts/Inter-VariableFont.ttf` with its `OFL.txt`, and imported in `app.slint` (`import "../../../library/fonts/Inter-VariableFont.ttf";`, path relative to the .slint file). It is compiled into the binary, so nothing loads at run time.
- **No Lucide:** the icon set is not on this machine and the build is offline. Icons are hand-drawn 24×24, 2 px stroke `Path` commands in a new `lab-ui/ui/icons.slint` (only the ~15 icons the app uses). Swapping in real Lucide is logged as a deferred defect.
- **Light mode covers the UI chrome and the title bar, not the 3D viewport.** The viewport keeps its studio look in every mode (a dark render inside light panels is normal in CAD tools). No viewport request for themes.
- **No cargo-wix, WiX v4+ if present.** cargo-wix is not in the offline registry and drives WiX 3 (`candle`/`light`), which is not installed; neither is WiX 4+ or dotnet. `installer/build.ps1` always builds the portable zip and builds the MSI only when `wix.exe` (WiX 4+) is on `PATH` or `$env:LABSETUP_WIX` names it; otherwise it prints "MSI skipped: WiX not found" and exits 0. A missing WiX is a Defect-log line, not a blocker.
- **Project thumbnail = a PNG render of the main camera taken at save time** through POLA-66's export request (512×288). Autosave writes no thumbnail.
- **Embedded parts are unpacked into the user library on open** (`<user_dir>/parts/embedded/`, `<user_dir>/meshes/`), then the catalog reloads. No per-project catalog layer. A local part with the same id wins (the file's copy is skipped and reported once).

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 2 | POLA-64 | Design-system pass: tokens, light/dark, high contrast | Engineer Sonnet | `theme.slint`, colour literals in every `.slint`, `icons.slint` (new), `library/fonts/` (new), `ThemeMode` in `lab-core/src/prefs.rs`, `theme` in `Settings`, `style_title_bar`, `view.theme.*` arms in `controller.rs`, theme button in the `app.slint` header |
| 2 | POLA-65 | Command palette (Ctrl+K) | Engineer Opus Low | `lab-app/src/actions.rs` (new), `lab-app/src/palette.rs` (new), `lab-ui/ui/palette.slint` (new), palette host + Ctrl+K + ⌕ header button in `app.slint`, `view.split` arm in `controller.rs` |
| 2 | POLA-66 | PNG export | Engineer Opus Low | `ExportImage`/`ImageExported` in `lab-viewport/src/lib.rs` + `host.rs` forwarding, `lab-viewport/src/export.rs` (new), the camera exclusion in `apply_render_target`, `lab-app/src/export_png.rs` (new), `lab-ui/ui/export.slint` (new), `file.export_png` |
| 2 | POLA-67 | BOM CSV export | Engineer Sonnet | `lab-app/src/bom.rs` (new), `file.export_bom` |
| 3 | POLA-68 | Project file embeds parts and a thumbnail | Engineer Opus Low | `lab-file/src/project.rs` (`EmbeddedFile`, `SaveOptions::extras`, `LoadedProject::extras`), `lab-app/src/embed.rs` (new), the save/load hooks in `files.rs` |
| 3 | POLA-69 | MSI installer and portable zip | Engineer Sonnet | `installer/**` (new), `README.md` "Packaging" section |

Tiers checked: the defaults hold. POLA-64 is wide but mechanical (move literals into tokens, add two palettes); POLA-67 is a pure function plus one menu item; POLA-69 is a script. POLA-65, POLA-66 and POLA-68 each add a small subsystem across two or three crates (Opus Low). Nothing in M5 needs Engineer Opus.

**Merges (Engineer Sonnet).** All four stage-2 tasks add to `app.slint` and three add to `controller.rs` (match arms) and the `ProjectMenu` in `overlays.slint` (POLA-64 recolours it, POLA-66/67 add items): keep both sides. `lab-app/src/lib.rs` (or `main.rs`) gets one `mod` line per new module. POLA-64 replaces literals in files the others touch: after merging, grep the merged `.slint` files for `#[0-9A-Fa-f]{6}` outside `theme.slint` and move any newcomer to a token in the `Integration fix:` commit.

## Shared interfaces

### Action ids (the palette lists them, the handlers own them)

| Id | Handler (owner) |
|---|---|
| `view.theme.dark`, `view.theme.light`, `view.theme.contrast` | `controller.rs` (POLA-64) |
| `view.split` | `controller.rs` (POLA-65): same as the UI's view mode 2 |
| `file.export_png` | `files::handle_file_action` → `export_png::start(ui, app)` (POLA-66) |
| `file.export_bom` | `files::handle_file_action` → `bom::export(ui, app)` (POLA-67) |
| `palette.open` | `controller.rs` (POLA-65) |

Every other id in `actions.rs` already exists. A new action anywhere later must get a row in `ACTIONS`.

### `lab-core/src/prefs.rs` + `lab-file/src/settings.rs` (POLA-64)

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode { #[default] Dark, Light, HighContrast }

pub struct Settings { /* … existing … */ #[serde(default)] pub theme: ThemeMode }
```

### `lab-ui/ui/theme.slint` (POLA-64)

```slint
export global Theme {
    in-out property <int> mode: 0;   // 0 dark (default), 1 light, 2 high contrast
    // every existing colour/brush property keeps its name and becomes
    // `out property <color> text: mode == 1 ? #1D1D1F : mode == 2 ? #FFFFFF : #F5F5F7;`
    out property <length> icon-size: 16px;
}
```

Other tasks use only token names, never colour literals. New tokens a later task needs are added to `theme.slint` with all three values.

### `lab-app/src/actions.rs` (POLA-65)

```rust
pub struct ActionInfo {
    pub id: &'static str,       // the string sent to `on_action`
    pub title: &'static str,    // "Export PNG…"
    pub group: &'static str,    // "File", "Edit", "View", "Tools", "Platform", "Room"
    pub keywords: &'static str, // extra search words, space separated
    pub shortcut: &'static str, // "Ctrl+S", "" if none
}
pub const ACTIONS: &[ActionInfo] = &[ /* every id the .slint files emit + the table above */ ];

/// Case-insensitive; ranks prefix of title > word prefix > subsequence over title+group+keywords.
/// Empty query returns all, in table order. Pure; one unit test.
pub fn search(query: &str) -> Vec<&'static ActionInfo>;
```

### `lab-viewport/src/lib.rs` (POLA-66; each adds its `FrameRequest` twin, forwarded like the others)

```rust
pub enum ViewportRequest {
    /* … existing … */
    /// Render the main camera (exactly as on screen: its transform and projection, the document
    /// as shown, no gizmos, hover or measure overlay) offscreen at `width`×`height`
    /// (clamped to 16..=8192), opaque background, MSAA 4, and write a PNG to `path`.
    /// Queued with thumbnails; at most one per frame.
    ExportImage { path: std::path::PathBuf, width: u32, height: u32 },
}
pub enum ViewportEvent {
    /* … existing … */
    /// Always sent once per `ExportImage`, `ok` false on any failure (the app shows an error toast).
    ImageExported { path: std::path::PathBuf, ok: bool },
}
```

In Split mode the main camera is the 3D pane, so it is what gets exported. Perspective keeps the on-screen vertical FOV and widens/narrows horizontally to the requested aspect; Top (orthographic) keeps the on-screen vertical extent.

### `lab-app/src/export_png.rs` (POLA-66, used by POLA-68)

```rust
/// Opens the export dialog (size), then the save dialog, then sends `ExportImage`.
pub fn start(ui: &AppWindow, app: &Rc<RefCell<AppState>>);
/// Sends `ExportImage` and calls `done(ok)` on the UI thread when `ImageExported` for `path`
/// arrives. If the viewport is not `Ready`, calls `done(false)` at once.
pub fn render_to(app: &Rc<RefCell<AppState>>, path: PathBuf, width: u32, height: u32,
                 done: Box<dyn FnOnce(bool)>);
```

`bridge.rs` routes `ImageExported` to the pending `done` (a `Vec<(PathBuf, Box<dyn FnOnce(bool)>)>` in `AppState`, matched by path).

### `lab-file/src/project.rs` (POLA-68)

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedFile {
    /// `thumbnail.png`, `parts/<file>.toml` or `parts/meshes/<64 hex>.glb`. Nothing else is
    /// written or read.
    pub name: String,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, Default)]
pub struct SaveOptions { pub backup_schema: Option<u32>, pub extras: Vec<EmbeddedFile> }
pub struct LoadedProject { /* … existing … */ pub extras: Vec<EmbeddedFile> }
```

---

## Stage 2

### POLA-64 Design-system pass (Engineer Sonnet)
- `theme.slint`: add `mode` and give every colour three values. Dark = today's values. Light: window `#F5F5F7`, panels `#FFFFFFE6`, text `#1D1D1F`/`#6E6E73`/`#AEAEB2`, hairlines `#0000001A`, same accent. High contrast: black surfaces, white text, `#FFD60A` accent, 2 px opaque borders (`border-width` token 1px → 2px in mode 2), no translucency. Spacing: snap `gap`/`inset` to the 8-pt grid (8, 16); radii to DESIGN §7 (panels 10 px, controls 6 px).
- Replace every colour literal outside `theme.slint` with a token (add tokens where none fits). `app.slint` sets `default-font-family: "Inter"` after importing the font file (Decisions).
- `icons.slint`: `export component Icon inherits Path` with `in property <string> name` (or one component per icon) drawing 24×24 stroke icons: search, sun-moon, undo, redo, save, folder-open, file-plus, image-down, table (BOM), cube (3D), square (Top), columns (Split), ruler, move, rotate, cursor, chevron-down, x, plus, trash. Replace the `font-symbol` glyphs with them. Colour = `Theme.text` unless the caller passes one.
- **Never colour alone:** every status colour (`ok`, `danger`, `warning`, beam) shown in the UI must have an icon or text next to it; fix the places that do not (toast, inspector error, validation fields).
- Header button (sun-moon icon) cycles Dark → Light → High contrast → Dark, through `view.theme.*`. Applying a mode: set `Theme.mode`, call `style_title_bar(window, mode)` (now takes the mode: light → `DWMWA_USE_IMMERSIVE_DARK_MODE` 0 and the light window/text colours, contrast → black/white), save `Settings.theme`. On start, apply the saved mode before the window shows.

**Works when:** the app starts in the saved mode, the header button switches all panels, menus, dialogs and the title bar between the three modes at run time, text renders in Inter, and `grep -E '#[0-9A-Fa-f]{6}' crates/lab-ui/ui/*.slint` finds literals only in `theme.slint`.

### POLA-65 Command palette (Engineer Opus Low)
- `actions.rs` as specified; fill `ACTIONS` from every `action("…")` id in the `.slint` files (grep them) plus the ids table above, with the shortcuts the FocusScopes implement.
- `palette.slint`: a centred floating panel (top third of the window, 560 px wide): search field, up to 10 rows (title, group in secondary text, shortcut right-aligned), the selected row highlighted. Model `[PaletteItem { id: string, title: string, group: string, shortcut: string }]`; callbacks `palette-query(string)`, `palette-run(string)`, `palette-close()`.
- `palette.rs`: `connect(ui, app, models)`: on query, `actions::search` → model; on run, close the palette and call the existing `on_action` path with the id (expose it from `controller.rs` as `pub(crate) fn run_action(ui, app, models, id: &str)` if it is private).
- `app.slint`: Ctrl+K in the `shortcuts` FocusScope (also while a field is focused) and the ⌕ "Ctrl+K" header button open it; Up/Down move, Enter runs the selected row, Esc or a click outside closes and returns focus to the viewport.
- `view.split` arm: set the UI view mode to 2 and send `SetViewMode(Split)` the way `view-mode-requested(2)` does.

**Works when:** Ctrl+K opens the palette, typing "top" shows "Top view" first, Enter switches to the top view; typing "save" and Enter saves; `actions::search` has one unit test (prefix beats subsequence, empty query returns all).

### POLA-66 PNG export (Engineer Opus Low)
- `lab-viewport/src/export.rs`: like `thumbnail.rs`, but the camera copies the main `OrbitCamera` entity's `Transform` and `Projection` (adjusted to the aspect, see above), sees the default render layers (the scene; not the gizmo/overlay layers), clears to the viewport background colour (opaque), renders into `Image::new_target_texture(width, height, Rgba8UnormSrgb, None)`, waits 2 settle frames, captures with `Screenshot::image`, writes with the shared `write_png` (move it to `pub(crate)` if needed), sends `ImageExported`, despawns. Add its marker to the `Without<…>` in `apply_render_target`. Keeps the on-demand loop alive only while a job is in flight. Share the one-job-at-a-time queue with thumbnails or keep a separate one; never two offscreen jobs in the same frame.
- `export.slint`: modal sheet "Export image": width and height fields (default = viewport size × 2), presets "Screen", "2×", "4K (3840×2160)", Cancel / Export….
- `export_png.rs`: Export… → `show_file_dialog` save, filter `*.png`, default name `<project name>.png` → `render_to` → toast "Exported <file name>" or error toast "The image could not be written.".
- Project menu: "Export PNG…" (`file.export_png`) under Save as Folder…, separated.

**Works when:** in 3D and in Top view, Export PNG… at 3840×2160 writes a PNG of that size that shows the same view as the screen (no gizmo), and the app returns to 0 frames afterwards.

### POLA-67 BOM CSV export (Engineer Sonnet)
- `bom.rs`:

```rust
pub struct BomRow { pub vendor: String, pub part_number: String, pub name: String, pub spec: String, pub quantity: u32 }
/// One row per (vendor, part_number, spec); sorted by vendor, part number, spec. Pure.
pub fn rows(doc: &lab_core::model::Document, catalog: &lab_catalog::Catalog) -> Vec<BomRow>;
/// RFC 4180: header `Vendor,Part number,Name,Spec,Quantity`, fields with `,` `"` or newlines
/// quoted, CRLF line ends, UTF-8 with BOM (Excel). Pure.
pub fn to_csv(rows: &[BomRow]) -> String;
/// Save dialog (`*.csv`, default `<project name> BOM.csv`), atomic write via lab-file's helper, toast.
pub fn export(ui: &AppWindow, app: &Rc<RefCell<AppState>>);
```

- Rows: every `Node` whose `part` is in the catalog: vendor = `def.vendor` (empty → "Generic"), part number = `def.part_number` (empty → the id after `:`), name = `def.name`, spec = the node's `params` as `key=value` joined by `; ` in key order (empty for fixed parts), so a 50 mm and a 75 mm post are separate rows. Unknown part ids: vendor "Unknown", part number = the id. Each `Platform` adds one row: vendor "Generic" (or its preset's vendor if the platform model has one), part number = preset id or "custom", name = platform name, spec = outline size `L×W×T mm`. Room walls, doors and windows are not BOM rows.
- Project menu: "Export BOM (CSV)…" (`file.export_bom`).

**Works when:** a unit test with two posts of different length, two identical KM100s and an unknown id gives 4 rows with the right quantities and a field containing a comma is quoted; exporting the demo project writes a CSV that opens in Excel with one row per part type.

---

## Stage 3

### POLA-68 Project file embeds parts and a thumbnail (Engineer Opus Low)
- `lab-file`: `save_project` writes `extras` after `project.json` (archive: zip entries; folder: files under the folder, creating `parts/` and `parts/meshes/`, deleting stale `parts/**` files the new save does not list). `load_project` returns every entry matching the three name patterns in `extras` (others are ignored), each ≤ 64 MB, total ≤ 512 MB (else `FileErrorKind` "too large", the project still loads without extras). The round-trip check still covers `project.json` only. Autosave passes no extras. One test: save with a TOML, a GLB and a thumbnail to archive and to folder, load, get the same extras back.
- `embed.rs`:

```rust
/// For every distinct `node.part` whose `catalog.source(id)` is `Source::User`: the def as
/// `parts/<id with ':' and '/' → '_'>.toml` (toml::to_string of the PartDef) and, for
/// `Geometry::Mesh { asset }`, `parts/meshes/<hex>.glb` from `mesh_path(asset)`. Plus
/// `thumbnail.png` if `thumbnail` is Some.
pub fn collect(doc: &Document, catalog: &Catalog, thumbnail: Option<Vec<u8>>) -> Vec<EmbeddedFile>;
/// Trust boundary: each TOML must parse as a PartDef; each GLB's BLAKE3 must equal its name.
/// Unknown ids are written to `<user_dir>/parts/embedded/<file>.toml`, meshes to
/// `<user_dir>/meshes/<hex>.glb` (skip if present), then `reload_catalog`. Returns how many
/// parts were added and the ids skipped because a local part with that id exists.
pub fn unpack(state: &AppState, extras: &[EmbeddedFile]) -> (usize, Vec<String>);
```

- `files.rs`: on save (not autosave), `export_png::render_to(<temp>/thumb.png, 512, 288, …)`, then `submit_save` with `extras = collect(…, thumbnail bytes or None)`. On load, `unpack` before the document is sent to the viewport; skipped ids → one toast "N parts in this file differ from your library; your versions are used."

**Works when:** a project with an imported STEP part saved as `.labsetup`, then opened with a fresh `%APPDATA%` (set `APPDATA` to an empty temp dir for the second run), shows the imported part with its mesh instead of a box; the archive lists `project.json`, `parts/…toml`, `parts/meshes/<hex>.glb`, `thumbnail.png`.

### POLA-69 MSI installer and portable zip (Engineer Sonnet)
- `installer/build.ps1`: `cargo build --release --package lab-app --offline` (shared target dir; it is long, allow the time), then stage `target/release/labsetup.exe`, `library/` (parts, meshes, fonts with OFL.txt), `tools/step2glb/build/Release/step2glb.exe` + its DLLs → `step2glb/`, and `third_party/dxc/dxcompiler.dll` if the app loads it from the exe dir, into `target/package/LabSetup/`; zip it to `target/package/LabSetup-<version>-portable.zip`.
- `installer/labsetup.wxs` (WiX 4+ schema): per-machine install to `ProgramFiles64Folder\LabSetup`, the same layout, Start-menu shortcut, `.labsetup` file association (open with `"labsetup.exe" "%1"`), upgrade code fixed, version from `lab-app/Cargo.toml`. `build.ps1` runs `wix build` on it only when WiX is found (Decisions).
- README "Packaging": how to run it and what it produces.

**Works when:** `installer/build.ps1` produces the zip; unzipped into an empty folder outside the repo with the network adapter state unchanged, `labsetup.exe` starts, the library shows the built-in parts with thumbnails, and Import STEP… converts `RS99_M-Step.step` (the sidecar is found next to the exe). If WiX is present, the MSI installs and the Start-menu shortcut starts the app; if not, one Defect-log line.

---

## M5 build done

After stage 3 merges, Hardening 1 (POLA-70) runs every gate and the §12 walkthrough, which now ends with PNG and BOM export from the palette and a reopened project with embedded parts.
