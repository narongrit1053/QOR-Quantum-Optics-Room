# LabSetup

An offline, native Windows app for designing quantum-optics lab setups: optical tables and custom platforms, Thorlabs-style mounting hardware and lasers, shown in 3D and as a top view.

Rust, with a [Slint](https://slint.dev) UI and a [Bevy](https://bevy.org) 3D viewport that share one GPU device. No web view, no network access, and no rendering while nothing changes.

The design, decisions and measurements are in [docs/DESIGN.md](docs/DESIGN.md).

## Status

Milestone M1 (core and platforms) is built and verified; the board signs it off. On top of M0's demo table, viewport and picking it adds:
- optical tables, breadboards and custom platforms from Thorlabs-sized presets, with rectangular, rounded, L-shaped or free polygon outlines (filleted corners), metric, imperial or custom hole grids and exclusion zones;
- an editable room (rectangle or L, any polygon) with mitred walls, doors and windows, and walls that fade when they block the view;
- inspector editing in millimetres or inches, with undo and redo;
- `.labsetup` archives, Save as Folder (`project.json`), autosave and crash recovery.

See [docs/DESIGN.md §14](docs/DESIGN.md) for the M1 measurements and §13 for M0.

## Build and run

Needs the Rust toolchain (MSVC) and Visual Studio 2022 Build Tools with the C++ workload. The first build downloads prebuilt Skia binaries and takes a long time (about an hour on a laptop); later builds take seconds.

```bash
cargo run -p lab-app
```

```bash
cargo test --workspace
```

The build copies `third_party/dxc/dxcompiler.dll` next to the executable. Ship it, with its `LICENSE.txt`, beside `labsetup.exe`: without it shaders compile with the slow FXC and the viewport stays empty for 10 s or more at startup (see [third_party/dxc](third_party/dxc/README.md)).

Check that nothing network-capable is linked into the app:

```bash
powershell -ExecutionPolicy Bypass -File scripts/check-offline.ps1
```

## Controls

| Input | Action |
|---|---|
| Left-drag | Orbit (pans in the top view) |
| Right/middle-drag, Shift+left-drag | Pan |
| Wheel | Zoom toward the cursor |
| Click | Select a part, platform or wall (Esc clears) |
| `T` / `P` | Top view / 3D view |
| `F` / `Shift+F` | Frame everything / frame the selection |
| `Delete` | Delete the selection (a platform takes the parts on it) |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` / `Ctrl+Y` | Redo |
| `Ctrl+N` / `Ctrl+O` | New / Open… |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save As… (Save as Folder… is in the project menu) |
| `Enter` / `Esc` in a field | Apply / revert the value; lengths accept `mm`, `cm`, `m`, `in`, `"`, `ft`, `'` |
| `Ctrl+Enter` in the paste sheet | Apply the pasted corners |

The project menu (click the project name) has New, Open…, Open Demo, Save, Save As… and Save as Folder…. The `+` next to SCENE adds a platform from a preset.

## Environment variables

| Variable | Effect |
|---|---|
| `LABSETUP_QUALITY=high\|balanced\|low` | Override the automatic rendering quality (default: High on discrete GPUs, Balanced on integrated ones) |
| `LABSETUP_TRACE=1` | Print frame-pacing timestamps to stderr |
| `RUST_LOG=lab_viewport=debug` | Log every rendered viewport frame with its resolution |
| `WGPU_POWER_PREF=low` | Use the integrated GPU (the default prefers a discrete one) |
| `LABSETUP_AUTOSAVE_DIR` | Autosave folder (default `%LOCALAPPDATA%\LabSetuputosave`) |
| `LABSETUP_SETTINGS` | Settings file (default `%APPDATA%\LabSetup\settings.json`): display units, wall display, last folder |

## Layout

| Crate | Purpose |
|---|---|
| `crates/lab-geom` | Pure geometry: polygons, fillets, triangulation, extrusion, mitred walls with openings. |
| `crates/lab-core` | Document model (schema 2), outlines, holes, room, validation, commands with undo/redo, presets, migration, demo setup. No GPU or UI dependencies. |
| `crates/lab-file` | `.labsetup` archives and folders, atomic saves, autosave and recovery, settings, the I/O worker thread. |
| `crates/lab-viewport` | Bevy viewport: on-demand rendering host, incremental scene sync, hole shader, room walls and fade, orbit camera, picking. |
| `crates/lab-ui` | Slint UI: design tokens, components, main window. |
| `crates/lab-app` | The executable: session and edit pipeline, view models, file dialogs and flows, wiring the document, UI and viewport together. |
| `scripts/dev` | Helpers for checking the running app (screenshots, posted input, idle measurement, the M1 end-to-end run). |
