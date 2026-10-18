# LabSetup

An offline, native Windows app for designing quantum-optics lab setups: optical tables and custom platforms, Thorlabs-style mounting hardware and lasers, shown in 3D and as a top view.

Rust, with a [Slint](https://slint.dev) UI and a [Bevy](https://bevy.org) 3D viewport that share one GPU device. No web view, no network access, and no rendering while nothing changes.

The design, decisions and measurements are in [docs/DESIGN.md](docs/DESIGN.md).

## Status

Milestone M0 (architecture spike) is done: a demo table with a 780 nm beam path, orbit/pan/zoom, an animated 3D ↔ top view switch, click selection with an inspector, and a scene list. See [docs/DESIGN.md §13](docs/DESIGN.md) for measured results.

## Build and run

Needs the Rust toolchain (MSVC) and Visual Studio 2022 Build Tools with the C++ workload. The first build downloads prebuilt Skia binaries and takes a long time (about an hour on a laptop); later builds take seconds.

```bash
cargo run -p lab-app
```

```bash
cargo test --workspace
```

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
| Click | Select a part (Esc clears) |
| `T` / `P` | Top view / 3D view |
| `F` | Frame everything |

## Environment variables

| Variable | Effect |
|---|---|
| `LABSETUP_QUALITY=high\|balanced\|low` | Override the automatic rendering quality (default: High on discrete GPUs, Balanced on integrated ones) |
| `LABSETUP_TRACE=1` | Print frame-pacing timestamps to stderr |
| `RUST_LOG=lab_viewport=debug` | Log every rendered viewport frame with its resolution |

## Layout

| Crate | Purpose |
|---|---|
| `crates/lab-core` | Document model, commands with undo/redo, hole grids, demo setup. No GPU or UI dependencies. |
| `crates/lab-viewport` | Bevy viewport: on-demand rendering host, scene building, orbit camera, picking. |
| `crates/lab-ui` | Slint UI: design tokens, components, main window. |
| `crates/lab-app` | The executable, which wires the document, UI and viewport together. |
