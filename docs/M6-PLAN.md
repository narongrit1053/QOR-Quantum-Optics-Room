# M6 plan: Beams and sets

New milestone after M5 (board request on POLA-87, 2026-10-21): an **interactive laser** (beam propagation and beam size), **premade 3D models** the user never has to import, and **component sets** (base, post holder, post, mount, optic) that move and rotate together and whose parts can each change length and size. Built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus the one check in its section), other flaws go to the Defect log (POLA-74, one line each), at most 2 attempts at any failure. Read this file's top part and your own section; open only the files it names.

**Exit criterion (POLA-93 walks it once):** on a table, drop the generic laser head and two 1" mirror sets and a 1" lens set from the built-in library (no import). The beam is drawn from the laser, reflects off both mirrors and narrows to a focus after the lens. Turning a mirror in its set (inspector rotation, or the gizmo) moves the beam live. The inspector shows the beam diameter at the selected optic. Changing the post length of one mirror set from 50 to 75 mm lifts its mount and mirror by 25 mm; its base stays put; the beam no longer hits it and the trace shows that. Every part looks like the real part, not a primitive.

## What already exists (do not rebuild)

- **Optical ports** (`lab-core/src/part.rs` `OpticalPort { name, kind, clear_aperture, frame }`) are already on the built-in parts. Kinds in `library/parts/**`: `source` (laser head `output`, +Y), `mirror`, `lens`, `splitter` (BS and PBS cubes), `waveplate`, `iris`, `dump`, `fiber`, `detector`. Port frame `z` = surface normal / optical axis in the part frame. Lens focal length is in `[meta] focal_length_mm = "100"` (a string). DESIGN.md §10 "Beam paths" is the intent.
- **Sets = mated chains.** The stack builder (`lab-app/src/stack.rs`, `place.rs`) places base → post holder → post → mount → optic as one `Command::PlaceAssembly`; each child has `parent: Parent::Node(..)` and `attach: Attach::Mate { self_conn, parent_conn, slide, twist }`, posed by `lab_mates::mate_pose(parent_frame, child_frame, slide, twist)`. Node positions are relative to the parent, so moving or rotating the root carries the whole set. The inspector already edits `node.rotation` (→ `twist` for a mated node), `node.slide` (post height in the holder) and `node.param.*` (→ `Command::SetParams`) in `lab-app/src/edit.rs` (~1491–1615).
- **The gap for sets:** connector frames are static TOML (`thorlabs:TR1.5` `top` at z = 38.1). Overriding a post's `length` re-meshes it but its `top` connector, and so the mount and optic, stay where they were.
- **Parametric generators** (`lab-parts/src/lib.rs` `generate(generator, params) -> Arc<PartMesh>`, cached by param hash): `post`, `pedestal_post`, `post_holder`, `base`, `clamping_fork`, `kinematic_mount`, `fixed_mount`, `rotation_mount`, `iris`, `periscope`, `mirror`, `lens`, `waveplate`, `cube`, `beam_dump`, `fiber_coupler`, `modulator_box`, `photodiode`, `camera`, `spcm`, `crystal_oven`, `laser_head`, `riser`, room fixtures. Today they are boxes and cylinders.
- **Viewport requests** (`lab-viewport/src/lib.rs`): `LoadDocument`, `ApplyChanges { snapshot, changes }`, `SetCatalog(Arc<Catalog>)`, `SetBlueprint`, `Isolate`, …; the gizmo (`gizmo.rs`) previews drags locally and commits on release.

## Decisions (recorded on POLA-87)

- **Premade models are better parametric generators, not bundled vendor CAD.** DESIGN.md §9.5: the installer never redistributes vendor files. Same generator names, same params, same part-local frames (origin bottom centre, +Y optical axis), so every TOML, connector and saved file keeps working; only the meshes gain the real silhouette.
- **Beam model:** sequential ray trace of the beam axis plus a Gaussian q-parameter per segment (ABCD, 1/e² radius `w`). Mirrors reflect, lenses are thin lenses (`f` from `focal_length_mm`, default 100 mm), splitters split into transmitted and reflected (50/50; PBS polarization is deferred), waveplates and irises pass (iris clips nothing in v1), dumps, detectors and fibers stop the beam. A ray misses a port when it hits the port plane outside `clear_aperture / 2` (no aperture = 12.7 mm radius). Max 64 bounces per source, max segment 5 m, then stop.
- **Beam params live on the laser node**: `Node.params` keys `beam_wavelength_nm` (default 780), `beam_waist_mm` (1/e² radius at the exit, default 0.5), `beam_m2` (default 1.0), `beam_on` (flag, default true). No schema change: `params` is already serialized.
- **The viewport traces for itself** from its snapshot + catalog (so a gizmo drag updates the beam live without the app), and the app traces the same way for the inspector. One function in `lab-optics` serves both.
- **Connector frames follow params** through one pure function in `lab-mates` (section D); after any edit, mated descendants are re-posed in the same command, so undo is one step.

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 1 | POLA-88 | A. lab-optics | Engineer Opus | `crates/lab-optics/**` (new), its line in the root `Cargo.toml` `members` and `Cargo.lock` |
| 1 | POLA-89 | C. Premade models | Engineer Opus Low | `crates/lab-parts/src/lib.rs` (generator bodies and new private mesh helpers only) |
| 1 | POLA-90 | D. Component sets | Engineer Opus Low | `crates/lab-mates/src/lib.rs` (`connector_frame`), the re-pose in `crates/lab-app/src/edit.rs`, the Set section in `crates/lab-ui/ui/inspector.slint` + its model in `crates/lab-app/src/ui_models.rs` |
| 2 | POLA-91 | Merges | Engineer Sonnet | merge commits, `Integration fix:` commits |
| 3 | POLA-92 | B. Interactive beams | Engineer Opus | `crates/lab-viewport/src/beams.rs` (new) + its plugin/request lines in `lib.rs`/`host.rs`, `crates/lab-app/src/beams.rs` (new), the Beam section in `inspector.slint`, `view.beams` action, `lab-viewport` and `lab-app` `Cargo.toml` (add `lab-optics`) |
| 4 | POLA-93 | Merges + walkthrough | Engineer Sonnet | merge commit, evidence |

Stage 1 tasks own disjoint files. B waits for stage 1 because it renders A's output and its inspector section sits next to D's.

## A. lab-optics (POLA-88, Engineer Opus)

New crate `crates/lab-optics`, depends only on `lab-core` and `glam` (both already in `Cargo.lock`; `--offline`). Pure, deterministic, no Bevy.

```rust
pub struct Port { pub node: NodeId, pub port: String, pub kind: PortKind, pub origin: DVec3, pub normal: DVec3, pub radius: f64, pub focal_length: Option<f64> }
pub enum PortKind { Source, Mirror, Lens, Splitter, Pass, Stop }   // waveplate, iris -> Pass; dump, detector, fiber -> Stop
pub struct Source { pub node: NodeId, pub origin: DVec3, pub dir: DVec3, pub wavelength_nm: f64, pub waist_mm: f64, pub m2: f64 }
pub struct Segment { pub source: NodeId, pub start: DVec3, pub end: DVec3, pub w_start: f64, pub w_end: f64,
                     pub waist: Option<(DVec3, f64)>, pub hit: Option<(NodeId, String)>, pub depth: u32 }
pub struct Trace { pub segments: Vec<Segment> }
impl Trace { pub fn radius_at(&self, node: NodeId) -> Option<f64> }   // 1/e² radius where the beam hits that node (first hit)

pub fn trace(sources: &[Source], ports: &[Port]) -> Trace;
/// World pose of a node: compose node position/rotation up the parent chain, then the platform
/// frame (position, rotation_z, top_height) or the room. Same convention as lab-viewport/src/sync.rs.
pub fn world_pose(doc: &Document, id: NodeId) -> Option<DAffine3>;
/// Sources and ports of every node, in world space. `part` looks a catalog def up by id.
pub fn scene(doc: &Document, part: &dyn Fn(&str) -> Option<PartDef>) -> (Vec<Source>, Vec<Port>);
pub fn trace_document(doc: &Document, part: &dyn Fn(&str) -> Option<PartDef>) -> Trace;  // scene + trace
```

- Gaussian: `q = z + i·zR`, `zR = π w0² / (M² λ)`; free space `q += d`; thin lens `1/q -= 1/f`; `w(q) = sqrt(-λ M² / (π · Im(1/q)))`. Units mm (λ in mm). `waist` is the segment's waist position and radius when the waist lies inside it (lens focus), else `None`.
- Intersection: nearest port plane hit along the ray (ignore hits closer than 1e-6 mm and the port it left from); a hit outside `radius` is no hit. Mirror reflect `d - 2(d·n)n`; splitter emits transmitted and reflected children (`depth + 1`).
- `scene` reads `beam_*` params from the laser node (defaults above) and skips it when `beam_on` is false.
- **Works when:** unit tests in the crate pass: (1) a source and one 45° mirror give 2 segments with the reflected one at 90°; (2) a collimated 0.5 mm beam through f = 100 mm focuses with the waist ~100 mm after the lens and `w0' ≈ λf/(π w)` within 5 %; (3) a mirror outside the aperture is missed; (4) `world_pose` of a child mated on a rotated platform matches a hand-computed pose. **Check:** `cargo test -p lab-optics --offline` (no app build needed: lab-app does not use it yet).

## C. Premade models (POLA-89, Engineer Opus Low)

Rework the generator bodies in `lab-parts/src/lib.rs` so each built-in part is recognizable as its real Thorlabs counterpart: post (chamfered ends, flats), post holder (tube with slot and thumbscrew, base flange), base / clamping fork (slot shape), pedestal post, kinematic mount (two plates, three adjusters with knobs, optic bore, springs omitted), fixed and rotation mounts (graduated ring), iris (lever), periscope, cube (cube + diagonal face visible through glass finish), beam dump, fiber coupler, photodiode, camera, SPCM, crystal oven, laser head (body, heat sink fins, exit aperture at the `axis_height`). Use the existing builder (`cyl_z`, boxes, `optic`) plus at most two new private helpers (a lathe of a profile, a box with a chamfer); no new dependencies.

- Keep every generator name, param name and default, the part-local frame and the outer dimensions (± 1 mm) so connectors and ports in the TOMLs stay right. Mesh budget ≤ 5k triangles per part (2k parts must still render, DESIGN.md §1).
- **Works when:** a unit test in `lab-parts` generates every generator with its default params, asserts a non-empty mesh under 5k triangles and a bounding box within 1 mm of the old one. **Check:** that test, plus `cargo build --package lab-app --offline` and one look at a KM100 stack in the running app.

## D. Component sets (POLA-90, Engineer Opus Low)

```rust
// lab-mates
/// A connector's frame for the given param overrides: for generators `post`, `pedestal_post`,
/// `post_holder` and `riser`, a connector whose origin sits on the part's top (z == default
/// length / height) moves to the overridden length / height. Every other frame is returned unchanged.
pub fn connector_frame(def: &PartDef, conn: &str, params: &Params) -> Option<Frame>;
```

1. Use `connector_frame` (with the node's `params`) everywhere a mate pose is computed in `lab-app` (`edit.rs`, `stack.rs`, `place.rs` read connector frames today; grep `.frame` on connectors).
2. **Re-pose in the same command:** after `node.param.*`, `node.slide` and `node.rotation` edits, recompute `mate_pose` for every mated descendant of the edited node (breadth-first, catalog frames via `connector_frame`) and emit one compound command (existing `Command::PlaceAssembly` inverse pattern or a `Vec` of `SetTransform`/`SetParams`; whatever `lab-core` already offers; no new `Command` variant unless none fits). One undo step reverts the whole edit.
3. **Relative rotation:** rotating a mated node (inspector field or gizmo ring about its mate axis) changes only its `twist`; its descendants turn with it, its ancestors do not. Rotating the root turns the whole set. Fix it where the gizmo commit path does not do that today.
4. **Set section in the inspector:** when the selected node belongs to a set (has a mated parent or mated children), show a "Set" list of the chain root → leaves, one row per member: name, its editable length/height/diameter params (whichever its generator has, via the existing `node.param.*` fields), slide and twist. Clicking a row selects that member; a "Select set" button selects the root.
- **Works when:** a unit test in `lab-mates` for `connector_frame` on a TR-series post (50 → 75 moves `top` by +25) and one test in `lab-app` (where the stack tests live, `stack.rs` `km100_stack_reaches_75mm` pattern) that sets the post length of a placed KM100 stack and asserts the mount moved up 25 mm and the base did not. **Check:** those tests and `cargo build --package lab-app --offline`.

## B. Interactive beams (POLA-92, Engineer Opus)

- **Viewport** `beams.rs`: on `LoadDocument`, `ApplyChanges` and `SetCatalog`, call `lab_optics::trace_document(snapshot, catalog lookup)` and draw each segment as a truncated cone (radius `w_start` → `w_end`, drawn radius `max(w, 0.3 mm)`; 12 sides), unlit emissive, colour from wavelength (visible: hue; IR > 700 nm and UV < 400 nm: the false-colour accent; DESIGN.md §7 says never colour alone, so IR beams are also dashed or striped). During a gizmo drag re-trace with the previewed pose so the beam moves live. Beams are not pickable and respect isolate/blueprint. A new request `SetBeams(bool)` hides them. No timer: tracing runs only on those events (idle stays near zero).
- **App** `beams.rs`: trace the document after each executed command (same function) and keep the `Trace` in `AppState` for the inspector.
- **Inspector Beam section:** on a laser node, editable wavelength (nm), waist (mm), M², On/Off (`node.param.beam_*`, existing `SetParams` path). On an optic hit by a beam: "Beam Ø at this part: 2w = … mm", and the distance from the source. On a node not hit: "No beam".
- **Toggle:** action `view.beams` ("Show beams", in the command palette and the view menu), default on, saved in `Settings` as `show_beams` (`#[serde(default = true)]`).
- **Works when:** the exit walkthrough at the top of this file works in the running app. **Check:** `cargo build --package lab-app --offline` and that walkthrough once (one screenshot under `D:/Try/LabSetup-wt/evidence/POLA-92/`).

## Merges (Engineer Sonnet)

- **POLA-91:** in `D:\Try\LabSetup` on `draft` (check it is clean), `git merge --no-ff draft-POLA-88`, `draft-POLA-89`, `draft-POLA-90` with messages `Merge <KEY>: <title>`. Root `Cargo.toml`/`Cargo.lock` get only A's lines. If D's `edit.rs` and C's `lab-parts` clash with nothing, there is nothing to resolve; fix only mechanical mismatches (`Integration fix:` commit). Run `cargo build --workspace --offline` once. Remove the three worktrees (keep branches).
- **POLA-93:** merge `draft-POLA-92`, run `cargo build --workspace --offline` and `cargo test -p lab-optics -p lab-mates -p lab-parts --offline` once, then the exit walkthrough once with one screenshot under `D:/Try/LabSetup-wt/evidence/M6/`. Every step that fails is one Defect-log line; do not fix features. Report on POLA-93.
