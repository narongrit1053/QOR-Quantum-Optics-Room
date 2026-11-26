# LabSetup

An offline, native Windows app for designing quantum-optics lab setups: optical tables and custom platforms, Thorlabs-style mounting hardware and lasers, shown in 3D and as a top view.

Rust, with a [Slint](https://slint.dev) UI and a [Bevy](https://bevy.org) 3D viewport that share one GPU device. No web view, no networking code linked in, and no rendering while nothing changes. The app goes online only through Windows' `curl.exe`, when you download STEP files or thumbnails or press **Update catalog**.

The design, decisions and measurements are in [docs/DESIGN.md](docs/DESIGN.md).

## Status

Milestones M1-M3 (core, platforms, library and placement, views) are built; M4 vendor import is in progress (see [docs/DESIGN.md](docs/DESIGN.md) §11).
LabSetup ships no vendor catalog: bring your own as a JSON file, written by hand, with an AI assistant, or for Thorlabs with [scripts/catalog/fetch-thorlabs.py](scripts/catalog/fetch-thorlabs.py), in the format in [docs/CATALOG-FORMAT.md](docs/CATALOG-FORMAT.md).
The Catalog window's **Update catalog** button fetches the Thorlabs parts in the background (one request per 30 s, one to two hours); see [catalog/README.md](catalog/README.md).

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
| View cube | Drag the view cube to orbit |
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
| `LABSETUP_AUTOSAVE_DIR` | Autosave folder (default `%LOCALAPPDATA%\LabSetup\autosave`) |
| `LABSETUP_SETTINGS` | Settings file (default `%APPDATA%\LabSetup\settings.json`): display units, wall display, last folder |

## Command line

`labsetup-cli.exe` (built next to `labsetup.exe`) reads the same project files and part library without a window or GPU. Read-only: no editing, no PNG export.

```
labsetup-cli --help | --version
labsetup-cli info     <project> [--json]   title, format version, counts (platforms, parts, constraints), load-report warnings
labsetup-cli list     <project> [--json]   one line per part: id, name, vendor/part number, parent, position mm, rotation deg (Z-Y-X)
labsetup-cli bom      <project> [-o out.csv]   same CSV the app's BOM export writes; stdout when no -o
labsetup-cli validate <project>            load and report problems; exit 0 ok, 1 problems, 2 usage/IO error
labsetup-cli catalog  [--search <text>]    built-in + user catalog parts (vendor, part number, name)
labsetup-cli live     <command> ...         drive the running app (see `labsetup-cli live --help`)
labsetup-cli mcp                            MCP server on stdin/stdout: the live commands as tools for AI agents
labsetup-cli agents   install|uninstall|status   register `labsetup-cli mcp` with the coding-agent CLIs on this PC
```

Errors go to stderr with a non-zero exit code. The installer puts the LabSetup folder on `PATH`.

### Connecting an AI agent

Turn on *Allow AI agents to control LabSetup* in Settings and click *Connect AI agents…* (or run `labsetup-cli agents install`). This adds a `labsetup` MCP server to the user config of each agent CLI it finds: Claude Code, the Claude desktop app, Codex CLI, Gemini CLI, Antigravity, Cursor, GitHub Copilot CLI, Windsurf, Devin CLI, VS Code, Trae, opencode and Hermes Agent. Only that entry is touched, and each file is backed up once to `<file>.labsetup-bak`. For other MCP-capable agents, `labsetup-cli agents install` prints a JSON snippet to paste into their config. Then any agent, opened in any folder, can see and edit the project open in LabSetup; each step is one undo entry. When it connects, every agent gets the same working rules (`INSTRUCTIONS` in `crates/lab-app/src/live/mcp.rs`): do only what you asked, use only catalog and library parts (importing a real part rather than building a look-alike), ask before removing parts or downloading catalog parts, never undo your own edits, look ids up instead of guessing, and report what changed. Agents can't save; you do.

## Packaging

```powershell
powershell -ExecutionPolicy Bypass -File installer/build.ps1
```

Builds the release app (long on a cold target dir) and writes `target/package/LabSetup/` and `target/package/LabSetup-<version>-portable.zip` (exe, `dxcompiler.dll`, `library/`, `step2glb/`). Unzip anywhere and run `labsetup.exe`; it needs no network. The sidecar is taken from `tools/step2glb/build/Release` (override with `LABSETUP_STEP2GLB_DIR`). If WiX 4+ (`wix.exe`) is on `PATH` or named by `LABSETUP_WIX`, it also builds `LabSetup-<version>.msi` from `installer/labsetup.wxs` (per-machine, Start-menu shortcut, `.labsetup` association); otherwise it prints "MSI skipped: WiX not found". `LABSETUP_SKIP_BUILD=1` reuses an existing `target/release/labsetup.exe`.

Releasing: bump `version` in `Cargo.toml`, rename `## Unreleased` in `CHANGELOG.md` to the new version (the app shows that file as *What's new*), build, then attach the `.msi` to the GitHub release. The in-app update check (`crates/lab-app/src/update.rs`) only offers releases that have an `.msi` asset.

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
