# LabSetup — System Design (v1)

> Working name **LabSetup** (crate prefix `lab-`); rename later.

## Context

You need a desktop tool to design and document quantum-optics lab setups: optical tables (standard or custom platforms), mostly Thorlabs mounting hardware, lasers (MOGLabs, Toptica, …), shown as a full 3D room/table model and as a top view of the whole setup or one plate. Hard constraints: **fully offline**, **Windows first**, **native, no web view, idle = near-zero CPU/GPU**, and an **Apple-like** look: clean, calm, smooth motion. `D:\Try\LabSetup` is empty, so this is a new project.

Decided with you:
- **Rust**, not Go. Go has no mature native GPU 3D/UI stack. Rust has wgpu, Bevy and Slint.
- **Slint** for the UI chrome, with a **Bevy 0.20** 3D viewport rendered into a shared wgpu texture. This follows Slint's official `examples/bevy/slint-hosts-bevy`, which pairs Bevy 0.20 with Slint's `unstable-wgpu-30` feature.
- **Hybrid models**: generic parts are generated from parameters. Vendor parts come from STEP files you download yourself; the app converts them into a local library. Nothing vendor-owned ships in the installer.

---

## 1. Requirements

### Functional (v1)
| # | Requirement |
|---|---|
| F1 | Room: walls/floor (polygon + height), door openings, simple fixtures (19" racks, desks, shelves, laser curtains) |
| F2 | Platforms: preset optical tables and breadboards, plus **custom platforms**: polygon outline, thickness, hole grid (metric M6/25 mm, imperial ¼-20/1", or custom pitch/thread/origin), exclusion zones, legs or isolators, and stacking (raised breadboards, overhead shelves) |
| F3 | Built-in library: posts, post holders, bases/clamping forks, pedestal posts, kinematic/fixed mounts (Ø½", Ø1", Ø2"), mirrors, lenses, PBS/BS cubes, waveplates and rotation mounts, irises, beam dumps, fiber couplers, AOM/EOM boxes, photodiodes, cameras, SPCMs, crystal ovens, periscopes, generic laser heads |
| F4 | Vendor parts: import STEP → mesh + LODs → define mounting connectors and optical ports → saved to your library |
| F5 | Placement: drag from library, **snap to hole grid**, **mate to compatible connectors** (mount → post → holder → base), rotate in steps, numeric editing in the inspector |
| F6 | **Beam-height stack builder**: pick optic + mount + target beam height, and the solver picks a post/holder combination and places the whole stack in one drag |
| F7 | Views: 3D perspective (whole room / table / selection), **top orthographic view** (whole setup or isolated plate), split view, animated view transitions, frame-selection, measure tool |
| F8 | Project save/load, undo/redo, autosave + crash recovery, PNG export, BOM CSV export |

### Non-functional
| Area | Target |
|---|---|
| Offline | No network crates in the dependency tree (enforced in CI), no telemetry, all fonts/icons/HDRI bundled |
| Idle cost | No frames rendered when nothing changes → ~0% GPU, <1% CPU |
| Performance | 60 fps at 1080p on an integrated GPU (Iris Xe class) with 2,000 placed parts; 10,000-part stress scene stays interactive (≥30 fps on a discrete GPU) |
| Startup | ≤ 2 s cold to interactive (catalog index cached; meshes lazy-loaded) |
| Memory | ≤ 600 MB for a 2,000-part scene |
| Footprint | App binary ~40–70 MB (Bevy + Slint/Skia); built-in parametric library is small; imported vendor meshes live in the user library |
| Platform | Windows 10/11, wgpu → DX12 (Vulkan fallback) |
| Robustness | Atomic saves (temp file + rename), schema-versioned files with migrations, STEP import isolated in a separate process |

### Assumptions
- One user per machine. Projects are shared as files (network drive, OneDrive, git). No real-time collaboration.
- Internal units are **mm and radians**; the UI can display mm or inches.
- Scale: a single table holds ~100–500 parts, a full room ~2,000. The design targets 10,000.
- Beam tracing and quantum simulation come later, but **the data model reserves optical ports now** so later features don't need a migration.

---

## 2. High-level architecture

```
┌──────────────────────────── lab-app.exe (one process) ─────────────────────────────┐
│                                                                                    │
│  ┌──────────── Slint UI (main thread, owns window + event loop) ─────────────┐     │
│  │ Toolbar · Library sidebar · Scene tree · Inspector · Command palette       │     │
│  │                  ┌───────────── Viewport element ─────────────┐            │     │
│  │                  │  shared wgpu texture  ◄── Bevy renders     │            │     │
│  │                  │  pointer/keys ───────────► forwarded       │            │     │
│  └──────────────────┴────────────────────────────────────────────┴────────────┘     │
│          ▲ view-models                         │ input            ▲ texture         │
│          │                                     ▼                  │                 │
│  ┌───────┴──────────────┐  Commands   ┌────────────────────────────┴────────────┐   │
│  │ App state            │◄────────────│ lab-viewport (Bevy App, ticked on demand)│   │
│  │  lab-core::Document  │             │  doc→ECS sync · cameras · picking ·      │   │
│  │  UndoStack · Bus     │────────────►│  snap preview · gizmos · LOD · materials │   │
│  └───────┬──────────────┘  ChangeSet  └──────────────────────────────────────────┘   │
│          │ uses                                                                     │
│  ┌───────┴──────────┐  ┌──────────────┐  ┌──────────────┐  ┌─────────────────────┐  │
│  │ lab-catalog      │  │ lab-parts    │  │ lab-mates    │  │ lab-optics (future) │  │
│  │ TOML defs+SQLite │  │ parametric   │  │ connectors,  │  │ rays, Gaussian ABCD,│  │
│  │ FTS + asset store│  │ mesh gen     │  │ snap, solver │  │ Jones, mode network │  │
│  └──────────────────┘  └──────────────┘  └──────────────┘  └─────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────────────┘
        │ spawns on import only
        ▼
  step2glb.exe (C++ / OpenCascade sidecar): STEP → XCAF (names, colors) → BRepMesh → GLB
```

**Core rule: the `Document` is the single source of truth.** Bevy's ECS is a disposable *projection* of it.
- Every edit is a `Command` applied to the Document, which pushes an inverse onto the undo stack and emits a `ChangeSet`.
- The viewport and the UI view-models update only from `ChangeSet`s. Neither one mutates the Document directly.
- The viewport talks to the app state **only through channels**: Commands go in one direction, ChangeSets the other.

Why: Bevy changes its API every ~4 months and the Slint↔wgpu interop is marked unstable. Keeping `lab-core`, `lab-catalog`, `lab-parts` and `lab-mates` free of any GPU/UI dependency means a framework upgrade, or a framework swap, touches only `lab-viewport` and `lab-ui`. It also makes the core logic unit-testable without a GPU.

### Data flow: "drag a mirror stack onto the table"
1. Slint forwards pointer events to Bevy.
2. Bevy ray-picks the table surface; `lab-mates::snap()` (a pure function) returns the nearest hole frame; Bevy renders a ghost preview.
3. On drop, the viewport sends `Command::PlaceAssembly{template, target: HoleRef(C,14), beam_height: 75 mm}`.
4. The app state applies it: the solver resolves post/holder lengths, nodes are added, an undo entry is pushed, a ChangeSet is emitted.
5. Bevy spawns the entities; the scene tree and inspector view-models update; one new frame is rendered, then the app goes idle again.

---

## 3. Workspace layout

```
LabSetup/
├─ Cargo.toml                 # workspace; pinned bevy = "0.20", slint (unstable-wgpu-30)
├─ deny.toml                  # cargo-deny: ban reqwest/hyper/ureq/tokio-net etc. (offline guarantee)
├─ crates/
│  ├─ lab-core/               # units, math (glam f64), Document, ids (ULID), Command/Undo, serde, migrations
│  ├─ lab-mates/              # connector types & compatibility, hole-grid snap, beam-height solver
│  ├─ lab-catalog/            # part defs (TOML), SQLite index + FTS5, content-addressed mesh store, built-in/user overlay
│  ├─ lab-parts/              # parametric generators: post, holder, base, mount, optic, table, breadboard, room
│  ├─ lab-import/             # runs step2glb, GLB post-process (meshopt LODs, quantize), thumbnail render
│  ├─ lab-viewport/           # Bevy plugin: sync, materials, hole-grid shader, cameras, picking, gizmos, outline, LOD
│  ├─ lab-ui/                 # .slint files, design tokens, components, view-models
│  ├─ lab-app/                # binary: Slint host + Bevy embedding (adapted from slint-hosts-bevy), wiring
│  └─ lab-optics/             # (placeholder) future beam/quantum engine
├─ tools/step2glb/            # C++17 + OpenCascade 7.x, CMake → step2glb.exe
├─ library/                   # built-in catalog: parts/**/*.toml, thumbs/, env/studio.ktx2, fonts/, icons/
└─ installer/                 # cargo-wix (MSI) + portable zip
```

---

## 4. Data model (`lab-core`, `lab-mates`)

### 4.1 Part definition (catalog, human-editable TOML)
```toml
id          = "thorlabs:KM100"            # vendor:part_number — stable key
version     = 1
vendor      = "Thorlabs"
part_number = "KM100"
name        = "Kinematic Mirror Mount, Ø1\" Optics"
category    = "mounts/kinematic"
tags        = ["mirror", "kinematic", "1in"]

[geometry]                                 # either a mesh asset…
kind  = "mesh"
asset = "sha256:9f2c…"                     # GLB in content-addressed store, LOD0..2 inside
# …or parametric:  kind = "parametric", generator = "post", params = { diameter = 12.7, length = 50, thread = "M4" }

[[connectors]]                             # mating interfaces (frames are local, +Z = mating axis)
name = "base_tap";   kind = "thread";    spec = "8-32";  gender = "female"
frame = { origin = [0, -20.3, 0], z = [0, -1, 0], x = [1, 0, 0] }

[[connectors]]
name = "optic_seat"; kind = "optic_bore"; diameter = 25.4
frame = { origin = [0, 0, 0], z = [0, 0, 1], x = [1, 0, 0] }

[[optical_ports]]                          # reserved for beam tracing (unused in v1 rendering)
name = "surface"; kind = "mirror"; clear_aperture = 22.9
frame = { origin = [0, 0, 0], z = [0, 0, 1], x = [1, 0, 0] }

[meta]
datasheet = "docs/KM100.pdf"               # optional, stored locally
```
Connector kinds in v1: `thread{spec, gender}` (M4, M6, 8-32, ¼-20), `bore{diameter}` (post in holder, with slide range), `optic_bore{diameter}`, `hole_grid_seat` (anything that bolts to a table), `cage_rod{spacing}` (30/60 mm), `lens_tube{thread}` (SM05/SM1/SM2). Mating rule: kinds and specs must match, frames are made coincident (Z opposed), and rotation about Z stays free. Where a range applies (post sliding in a holder), translation along Z is free within that range.

### 4.2 Project document
```
Project { schema_version, meta{name, author, display_units, default_beam_height} ,
          rooms[], platforms[], nodes[], assemblies[], layers[] }

Room      { id, outline: Polygon2, height, openings[], fixtures[NodeId] }
Platform  { id, kind: Table|Breadboard|Custom, outline: Polygon2 (+fillets), thickness,
            hole_grid{pitch_xy, thread, origin, margin, exclusions: [Polygon2], extra_holes[]},
            support: Legs{n, height, isolator} | OnPlatform{parent, standoffs} | Fixed,
            transform (in room), material }
Node      { id: ULID, part: PartRef{id, version}, parent: Platform|Node,
            attach: HoleRef{row,col} | Mate{self_conn, parent_conn, slide, twist} | Free(Transform),
            overrides{label, color, params}, layer, locked }
Assembly  { id, name, template nodes (relative), parameters (e.g. beam_height) }
```
- Positions are stored **relative to the parent** (hole index + offset, or mate). Moving a table carries everything on it, and the file stays meaningful to a human ("row C, col 14").
- Documents use f64 in mm; geometry is converted to f32 per platform for rendering (µm precision at room scale).

### 4.3 Commands & undo
`enum Command { AddNode, RemoveNodes, SetAttach, SetTransform, Mate, Unmate, SetParam, AddPlatform, EditPlatform, Group, Ungroup, PlaceAssembly, … }`. Each command applies and returns its inverse. A continuous drag is one transaction (coalesced). History depth 500. Autosave every 60 s to `%LOCALAPPDATA%\LabSetup\autosave\`, with a recovery prompt on the next start.

### 4.4 Beam-height solver (`lab-mates`)
Inputs: mount (optical-axis offset above its base tap), target beam height H, platform top Z, and the catalog of posts/holders/bases/spacers.
Solve: pick holder + post such that `holder_seat + post_insertion_range ∋ H − mount_offset − base_thickness`, preferring shortest post, then most adjustment margin. Output an assembly plus a warning if no combination fits. This is pure, deterministic and unit-tested.

### 4.5 Project file
`*.labsetup` = zip containing `project.json` (serde, schema-versioned), `parts/` (embedded copies of every non-built-in part definition and mesh used, so a file opens on any machine), and `thumbnail.png`. Trade-off: zip is not diff-friendly, so offer **"Save as folder"** (same layout, unzipped) for git users.

---

## 5. Catalog & asset pipeline (`lab-catalog`, `lab-parts`, `lab-import`, `step2glb`)

**Storage**
- Built-in library: `<install>/library` (read-only).
- User library: `%APPDATA%\LabSetup\library` (writable). It overrides the built-in library by `id`.
- `parts/**/*.toml` is the source of truth. `catalog.sqlite` (rusqlite + FTS5) is a rebuildable index (name, vendor, part number, tags, category, connector kinds) for instant search.
- Meshes: `meshes/<sha256>.glb` (deduplicated, content-addressed). Thumbnails: `thumbs/<id>.png`, rendered offscreen by Bevy at import time.

**Parametric generators** (`lab-parts`, pure Rust): lathe/extrude/boolean-free procedural meshes for posts, holders, bases, forks, generic mounts, optics (with correct clear aperture), tables (top surface is a single quad, see 6.2), breadboards, room walls, racks and generic laser heads. Meshes are generated on demand and cached by a parameter hash. Correct dimensions matter more than surface detail.

**STEP import (vendor parts)**
1. The user picks a STEP file downloaded from Thorlabs/MOGLabs/Toptica.
2. `step2glb.exe` (a separate process with a 60 s timeout, so a crash cannot take down the app) reads it with OpenCascade `STEPCAFControl_Reader` (keeps part names and colours), meshes it with `BRepMesh_IncrementalMesh` (≈0.05 mm linear / 0.3 rad angular deflection) and writes GLB via `RWGltf_CafWriter`.
3. `lab-import` (Rust) merges by material, then uses `meshopt` to build LOD0 ≤ 30k tris, LOD1 ~25%, LOD2 ~5%, quantizes, hashes and stores the result.
4. **Connector wizard** in the app: click a tapped hole and the app fits the cylinder axis from the mesh and asks for the thread; click an optical face to define an optical port. Templates speed this up, e.g. "Laser head" asks for the beam exit point, direction and wavelength.
5. The TOML definition is written to the user library and the index is updated.

OpenCascade is LGPL-2.1 with an exception. It is dynamically linked inside the sidecar only, so the main binary stays lean and pure Rust.

---

## 6. 3D viewport (`lab-viewport`, Bevy 0.20)

### 6.1 Embedding & power
- Use Slint's `slint-hosts-bevy` pattern: Bevy renders into a wgpu texture shared with Slint (Skia renderer, `unstable-wgpu-30`).
- **The Bevy app is ticked only when needed**: a ChangeSet arrived, the camera moved, the hover/gizmo changed, or an animation is running. Otherwise there are zero frames, which meets the "no unnecessary resource" requirement. *(Measured in M0: 0.00% CPU and GPU while idle.)*
- **Windows backend is Direct3D 12.** Skia's wgpu interop supports only DX12 on Windows; automatic backend selection picked Vulkan on Intel UHD and failed at startup.
- **One frame in flight** (two textures: one on screen, one rendering). With three, the viewport rendered ahead and the UI dropped every other frame on GPU-bound machines.
- **Adaptive resolution.** Frames during motion (drags, zoom, camera animations) render at a scale chosen to keep a ~60 Hz cycle (0.4–1.0); 120 ms after motion stops, one full-resolution frame follows.
- **Explicit UI wake-up.** The viewport thread calls `slint::invoke_from_event_loop` after each event; relying on an async channel's waker stalled for ~600 ms.
- **Quality levels** (High: MSAA 4× + shadows; Balanced: MSAA 4×, no shadows; Low: neither), chosen automatically from the GPU type, with MSAA counts checked against the adapter (2× is unsupported on Intel UHD and crashed rendering).
- Split view uses two cameras rendering to two textures.

### 6.2 Rendering
| Concern | Approach |
|---|---|
| Look | Studio lighting: bundled small HDRI environment map, one directional key light with soft cascaded shadows, SSAO, MSAA 4×, AgX tonemapping. Materials: black-anodized Al, stainless posts, matte table steel, glass optics (specular transmission), coated mirrors |
| Hole grids | **Procedural fragment shader** on one quad per platform (SDF circles on the pitch grid, plus exclusion masks). An 8,000-hole table costs a few triangles; the same shader gives crisp holes in the top view |
| Many parts | Bevy's automatic batching/GPU instancing (posts and holders repeat hundreds of times); `VisibilityRange` LOD switching |
| Picking | `MeshPickingPlugin` against LOD2/proxy meshes (fast), hover highlight, selection outline post-process |
| Navigation | Custom orbit controller fed by input forwarded from Slint (Bevy's built-in `PanOrbitCamera` reads Bevy window input, which does not exist here): left-drag orbits, right/middle/Shift-drag pans, wheel zooms toward the cursor; drags track 1:1, view changes ease out; `F` frames everything; view cube later |
| Top view | Orthographic camera straight down; optional "blueprint" material swap (flat, outlined); labels and dimensions drawn by Slint as an overlay synced to projected positions |
| Gizmo | Custom constrained gizmo: translate on the platform plane with hole snap (Shift = free, Ctrl = fine), rotate about the vertical axis in 15° steps (configurable), raise/lower along mate ranges |

---

## 7. UI & design language (`lab-ui`, Slint)

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ LabSetup · Rb87-MOT.labsetup     ⟨ 3D │ Top │ Split ⟩     ⌕ Ctrl+K   ◐   ─ ☐ ✕ │  title bar coloured like the app
├──────────────┬─────────────────────────────────────────────────┬──────────────┤
│ ⌕ Search…    │                                                 │ KM100        │
│ LIBRARY      │                                                 │ Thorlabs     │
│ ▸ Tables     │               3D viewport (Bevy)          ⬚ cube│ ───────────  │
│ ▸ Posts      │                                                 │ Hole  C · 14 │
│ ▸ Mounts     │                                                 │ Beam  75 mm  │
│ ▸ Optics     │                                                 │ Rotate 45°   │
│ ▸ Lasers     │      ╭──────────────────────────────────────╮   │ ───────────  │
│ SCENE        │      │ ↖ Select  ✥ Move  ⟳ Rotate  ⟷ Measure│   │ Connectors   │
│ ▾ Table A    │      ╰──────────────────────────────────────╯   │ Notes        │
└──────────────┴─────────────────────────────────────────────────┴──────────────┘
```
- **Layout**: full-bleed viewport; translucent sidebars (library + scene tree on the left, inspector on the right) that can collapse; a floating pill toolbar; command palette (Ctrl+K) for every action.
- **Tokens**: 8-pt spacing grid; radius 10 px for panels and 6 px for controls; one accent colour (follows the Windows accent, default #0A84FF); dark mode default (labs are dim) plus light mode.
- **Type & icons**: **Inter** (OFL) bundled. SF Pro is licensed for Apple platforms only, so it cannot be used. Icons: Lucide (ISC), as SVG.
- **Motion**: 150–250 ms ease-out/spring for panels, 350 ms for camera moves; respects the Windows "reduce animations" setting.
- **Title bar**: native caption with dark mode and an explicit caption colour equal to the window background (DWM attributes), which also overrides the "accent colour on title bars" setting. *(M0: Mica was dropped. The client area is fully covered by the 3D viewport, so a Mica backdrop is never visible, and an explicit caption colour replaces it in the title bar.)*
- **Lab-specific**: never encode meaning by colour alone, because laser goggles distort colour perception; offer a high-contrast mode; IR wavelengths get false-colour beams later.
- Accessibility via Slint's AccessKit integration.

---

## 8. Trade-offs

| Decision | Chosen | Alternative | Why / cost |
|---|---|---|---|
| Language | Rust | Go | Rust has wgpu/Bevy/Slint; Go has no native 3D+UI stack of this quality. Cost: steeper language |
| UI | Slint + Bevy texture | Bevy-only (Feathers), Bevy+egui | Best polish and animation. Cost: unstable wgpu interop, so Slint and Bevy must be upgraded together |
| Model ownership | Document → ECS projection | ECS as the model | Insulates from Bevy churn and is testable. Cost: a sync layer |
| STEP | OpenCascade sidecar | pure-Rust `truck`/`foxtrot` | OpenCascade is the most robust STEP reader; pure-Rust options have known triangulation gaps. Cost: C++ build and ~30–50 MB of DLLs (only needed for import) |
| Generic parts | Parametric | Bundled meshes | Tiny, exact, no licence risk. Cost: less visual detail |
| Holes | Shader | Geometry | Constant cost per table. Cost: custom WGSL |
| Catalog | TOML + SQLite index | DB only | Human-editable and diffable, with fast search. Cost: reindex step |
| Project file | Zip (+ folder option) | Plain JSON | Self-contained (embeds custom parts). Cost: not diffable by default |

## 9. Risks & what to revisit

1. **Slint↔Bevy wgpu lockstep.** Pin both versions and upgrade together. Fallback: Bevy-only UI with Feathers; the clean core/UI split keeps that swap contained to `lab-ui` and `lab-app`.
2. **Directional shadows on integrated GPUs.** Measured on Intel UHD (DX12): shadows cut animation from ~21 to ~4–5 fps, even with one cascade at 1024 px and 2×2 filtering. Integrated GPUs therefore default to Balanced (no shadows). Profile with PIX before M5; candidates are baked/contact shadows or a cheaper shadow path.
3. **First-run shader compilation** takes ~5 s on Intel UHD (synchronous pipeline compilation), which misses the 2 s startup target. Revisit with asynchronous compilation behind a placeholder frame.
4. **STEP robustness and size.** Sidecar isolation, timeout, a user-adjustable decimation slider, and per-part triangle budgets.
5. **Vendor CAD terms.** Users import files they downloaded themselves; the installer never redistributes vendor files.
6. **Bevy release churn (~quarterly).** All Bevy code lives in `lab-viewport`; upgrade on our schedule, not each release.
7. **Revisit as it grows**: a dedicated 2D vector renderer for PDF/SVG drawing export, multi-user merge of project files, and a scripting/plugin API.

## 10. Future features (already provided for in the design)
- **Beam paths** (`lab-optics`): optical ports → sequential ray trace across mirrors/BS/PBS/lenses; Gaussian q-parameter (ABCD) for waist and divergence; Jones calculus for polarization; beams rendered as tubes sized by waist.
- **Quantum link**: extract the linear-optics mode network (BS, PBS, phases, waveplates, detectors and their efficiencies) from the scene, then export a unitary/circuit description (JSON) for your simulation tools (e.g. Python/QuTiP).
- Drawing export (SVG/PDF top view with labels), inventory/BOM with prices, DXF import of custom platform outlines.

## 11. Roadmap

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0 Spikes** | Workspace skeleton; Slint window hosting a Bevy texture (from `slint-hosts-bevy`); orbit camera; pick a cube; on-demand ticking; Mica test; `step2glb` converts 3 real Thorlabs STEP files | Idle GPU ≈ 0% in Task Manager; picking works; GLB opens in the viewport |
| **M1 Core + platforms** | `lab-core` Document/Commands/undo/save-load; platforms with hole-grid shader (metric/imperial/custom); room box | Round-trip save/load tests pass; a custom polygon table renders with correct holes |
| **M2 Library + placement** | `lab-catalog` (TOML + SQLite FTS), `lab-parts` generators, drag-drop, hole snap, mating, beam-height stack builder | Build a 20-component table at 75 mm beam height without typing coordinates |
| **M3 Views** | Top view, split, isolate plate, measure, animated transitions, selection outline, inspector editing | Switch 3D↔Top smoothly; isolate one plate |
| **M4 Vendor import** | Import flow, LODs, connector wizard, laser-head template (MOGLabs/Toptica) | Import a KM100 + one laser head and mount them correctly |
| **M5 Polish & ship** | Design-system pass, light/dark, command palette, autosave/recovery, PNG + BOM export, MSI installer, performance pass | Meets the §1 non-functional targets on your lab PC |

## 12. Verification
- **Unit tests** (no GPU): `lab-core` checks that every Command's inverse restores state, serde round-trips and migrations work; `lab-mates` checks snap math, connector compatibility and solver cases (including "no fit"); `lab-catalog` parses every built-in TOML and tests FTS search; `lab-parts` checks generated mesh bounds against nominal dimensions.
- **Offline guarantee**: `cargo deny check` bans network crates in CI; a manual run with the network adapter disabled; Windows firewall log shows no outbound attempts.
- **Performance harness**: synthetic 2k- and 10k-part scenes; measure fps (PresentMon), idle CPU/GPU, cold-start time and memory against the §1 targets.
- **End-to-end**: build a sample "Rb-87 MOT + SPDC source" table → save → reopen → top view of the isolated plate → export PNG/BOM, then compare against expectations.

## 13. M0 results (2026-10-18)

Measured on the development laptop (Intel UHD Graphics, DX12, 1440×900 viewport, debug build with optimised dependencies).

| Check | Result |
|---|---|
| Slint window hosting a Bevy 0.20 viewport through a shared wgpu 30 device | Works (Slint 1.18.1, `unstable-wgpu-30`) |
| Idle cost after startup and after interaction | **0.00% CPU, 0.00% GPU** over 10–12 s |
| Click picking (3D and top view) → inspector and scene list | Works, including at reduced render resolution |
| Animated 3D ↔ top (orthographic) transition | ~42 fps with adaptive resolution (was 3.9 fps at full resolution with shadows) |
| Unit tests | 22 passing (`lab-core` 15, `lab-viewport` 7) |
| Offline check (`scripts/check-offline.ps1`) | 419 runtime crates, none network-capable |
| Working set | ~515 MB (debug build) |
| First full build | 59 min (incremental builds: 20–30 s) |

Not done in M0: a release build (binary size, memory) and a GPU profile of the shadow cost.

### step2glb spike

`tools/step2glb` (C++17, CMake, official OpenCascade 7.9.3 prebuilt for MSVC x64) converts a STEP file into a GLB in millimetres, +Z up. `tools/step2glb/README.md` has the build steps (`fetch-occt.ps1`, then two CMake commands; checked with a clean build), the options and the exit codes. Results on the same laptop, release build, default deflection (0.05 mm / 0.3 rad), `--source-up y`, median of three runs:

| Thorlabs file | STEP | Source | Faces | Triangles | GLB | Colours | Wall time | Peak memory |
|---|---|---|---|---|---|---|---|---|
| FMMP1A3/M | 0.48 MB | AP203, inch | 116 | 4,560 | 0.14 MB | none | 0.38 s | 41 MB |
| FMMP1/M | 4.3 MB | AP203, inch | 819 | 53,784 | 1.6 MB | none | 4.5 s | 172 MB |
| RS99/M | 12.7 MB | AP214, inch | 2,534 | 83,008 | 2.5 MB | 8 (12 primitives) | 5.2 s | 210 MB |

- **Units.** All three files are in inches; the reader converts to millimetres. Mesh extents match the nominal sizes (FMMP1A3/M is 38.10 mm = 1.5" wide; FMMP1/M stands 75.9 mm tall).
- **Orientation.** STEP files do not say which axis is up, and these Thorlabs files are modelled Y-up (SolidWorks). `--source-up y` rotates them to +Z up, with FMMP1/M's base at z = 0. The importer should pass it for Thorlabs parts; other vendors still need checking.
- **Time** is dominated by the B-rep transfer (3.3–3.7 s of ~4.5 s on the two larger files); meshing takes 0.4–0.6 s. That is well inside the 60 s timeout.
- **Triangles depend on the angular deflection, not the linear one.** RS99/M: 83k at 0.05 mm, 79k at 0.2 mm; 56k at 0.5 rad, 48k at 0.8 rad (FMMP1/M: 54k → 42k → 24k). Two of three parts exceed the 30k LOD0 budget, so the meshopt step in `lab-import` (§5 step 3) is required, and the default angular deflection could go to 0.5 rad.
- **Names and colours.** Each file is a single part named after its STEP product (`FMMP1_M-E0W`; RS99/M is `6826-E0W`), with no assembly structure. AP203 files carry no colours, so their GLBs have no material and the importer has to assign a finish. All primitives are `doubleSided` (OpenCascade's default); `lab-import` should clear that for closed solids to keep back-face culling.
- **Bad input** exits non-zero with one line on stderr within 0.15 s: missing, empty, non-STEP, ZIP-packed, truncated and syntax-error files exit 2; a file with no shapes exits 3; a missing output folder exits 5. Crashes are reported (exit 6) instead of opening the Windows error dialog. Output goes to `<name>.partial` and is renamed when complete. Paths with Thai characters work.
- **Footprint.** The sidecar folder is 62 MB: `step2glb.exe` and 34 DLLs (25 OpenCascade toolkits, 9 third-party). That is above the 30–50 MB estimated in §8: ~25 MB is FreeImage, ffmpeg and OpenVR, which `TKService` links even though step2glb never uses them. A from-source OpenCascade build without those options would remove them.

**GLB in the viewport (M0 exit criterion): works.** A throwaway copy of the workspace (outside the repo) loaded the three GLBs onto the demo table. They render upright, at true scale against the 25 mm hole grid, with RS99/M's colours, and the app goes idle afterwards (0.00% CPU and GPU with the parts loaded). The change is saved, not merged, as `tools/step2glb/viewport-spike.diff` for M4:
1. Add the Bevy features `bevy_gltf` and **`reflect_auto_register`**. Without the second, the viewport thread panics when the glTF scene spawns ("world contains the unregistered type"): Bevy 0.20 relies on automatic type registration, and our `default-features = false` drops it. Adding both features rebuilt most of Bevy and wgpu once (17 + 11 min with a warm cache). The offline check still passes: 426 runtime crates, none network-capable.
2. Spawn `WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(path)))` with `Transform { translation: to_render(p), rotation: rotation_to_render(r) * Quat::from_rotation_x(-FRAC_PI_2), scale: Vec3::splat(MM) }`. Bevy's glTF coordinate conversion is off by default, so the vertices arrive in document space unchanged.
3. Wake the on-demand loop when a load finishes. Asset loads complete asynchronously and nothing else triggers a frame; the spike sets `RedrawState::keep_alive` until the scene has spawned, plus two frames for mesh bounds. M4 should wake the loop from asset events instead of rendering throughout the load.
4. Still open for M4: the spawned meshes have no `PickTarget`/`SceneNode`, so GLB parts cannot be picked or highlighted yet (tag them on `WorldInstanceReady`). Working set with the three parts: 652 MB (debug build; 515 MB in M0 without them).

## Next steps
M0 is complete, including the step2glb spike above.
1. M1: platform editing (custom outlines, imperial/metric grids, SDF hole shader), save/load, and the room.
2. Release build: measure binary size, memory and startup against §1.
3. For M4, apply the step2glb findings above: the `reflect_auto_register` Bevy feature, mandatory mesh simplification, default angular deflection 0.5 rad, `--source-up y` for Thorlabs parts, and picking for GLB parts.
