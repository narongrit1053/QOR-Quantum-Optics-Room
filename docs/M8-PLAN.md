# M8 plan: Constraints, view cube, smart snap points

New milestone after M7 and Defect round 1 (board request on POLA-112, 2026-10-22). Built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus the one check in its section), other flaws go to the Defect log (POLA-74, one line each), at most 2 attempts at any failure. Read this top part and your own section; open only the files it names. Branch from the current `draft`.

The request: (1) pick a **face or axis** of a 3D model and add **Fusion 360 / Inventor-style constraints** between two picked features; (2) a **view cube** in the top-right of the viewport that turns the camera to each side, with orbiting around the selected item (nothing selected: room centre; table or breadboard selected: centre of its top face); (3) on import, **smart snap points** from detected holes, axes and planes instead of the bounding-box centre. The faceted/smeared look of imported STEP parts is explicitly "later" (Defect log).

**Exit criterion (stage 4 walks it once):** import a Thorlabs KM100 STEP: the wizard opens with detected holes and faces already listed, and dropping the part on the table seats it on a detected bottom hole, not the bbox centre. Drop a post holder and a post. Press J, click the post's side (an axis is highlighted), click the holder's bore (axis highlighted): the post jumps concentric into the holder. Add a second constraint (post end face → holder bottom face, Mate, offset 10 mm): the post sits 10 mm above; change the offset in the inspector to 20 mm: it moves; undo restores 10 mm. Move the holder: the post follows. Click the view cube's Front face: the camera turns to a front view in 0.35 s around the selected post; select nothing, click Top: the view centres on the room; select the table, orbit: the pivot is the table top centre. Save, reopen: the constraints are still there.

## What already exists (do not rebuild)

- **Picking** (`lab-viewport/src/picking.rs:54-105`): Bevy `MeshRayCast` against entities with `PickId(SelectionTarget)`; it keeps only the entity. The connector wizard's inspect mode (`inspect.rs:132-147`) already turns a hit into `ViewportEvent::PartPoint { point, normal }` in part space (mm, +Z up) and draws arrows with `Gizmos<OnTop>` (`inspect.rs:177`, `gizmo.rs:479`). Spaces: `scene.rs:34 to_render`, `:51 from_render`. Only imported LOD0 has `PickId` (`viewport/glb.rs:120-125`) and LOD0 is hidden beyond ~1.5 m.
- **CPU meshes**: `lab_geom::TriMesh` (`lab-geom/src/mesh.rs:12`); generated parts `lab_parts::generate(..) -> Arc<PartMesh { pieces, .. }>`; imported parts `wizard::part_mesh(def) -> TriMesh` (`lab-app/src/wizard.rs:178`, LOD0 merged).
- **Hole fit**: `lab_import::fit::fit_hole(mesh, point, normal) -> Option<HoleFit { axis: Frame, diameter }>` (`fit.rs:20`, local RANSAC within 6 mm) and `suggest(diameter) -> (kind, spec)` (`fit.rs:201`).
- **Mates**: `Attach::{Free, Hole, Mate { self_conn, parent_conn, slide, twist }}` (`lab-core/src/model.rs:393`), node pose relative to its parent, so children follow. `lab_mates::mate_pose`, `seat_pose(part, hole, angle)` (uses the connector named `seat`, else the part origin). `Command::SetAttach { id, parent, attach, position, rotation }`, `Command::Batch`. Inspector mate editing: `lab-app/src/edit.rs` ~1478-1622.
- **Import**: `lab_import::import_step` (`lab-import/src/lib.rs:103`) re-bases the part on its bbox bottom centre (`:138-142`); `lab-app/src/import.rs:145-165` writes a `PartDef` with one `seat` connector at the origin; `write_part` writes the TOML.
- **Camera** (`lab-viewport/src/camera.rs`): `View { focus, yaw, pitch, distance }`, `OrbitCamera { current, target, tween, reduce_motion, .. }`, `ease_to(view)` (0.35 s), pitch clamped to `MIN_PITCH..MAX_PITCH` (`:28-29`), orbit pivots on `current.focus` (`drag()` `:422-436`), `ViewportRequest::Frame(..)`. New `ViewportRequest` variants need an arm in `handle_input`'s match (`:417`). UI → viewport: Slint callback → `controller.rs` → `AppState::send(ViewportRequest)`.
- **Platforms**: `doc.platform_pose(id)` (origin on the top surface), `doc.platform_room_aabb(id)` (max z = top), `outline.bounds()`.

## Decisions

- **No B-rep.** step2glb merges faces, so features are inferred from triangles: a **plane** is the coplanar region grown from the clicked triangle (normals within 2°, plane distance within 0.05 mm); an **axis** is a cylinder fit (holes and shafts, i.e. concave and convex). One function serves the click picker and the import detector.
- **A constraint stores its feature geometry inline** (in the part's own frame), not by name: generated parts have no named features and inline geometry survives catalog edits. Imported parts additionally get `PartDef.features` from import so the picker can snap to a clean detected feature.
- **Constraint set per node, one parent.** A constrained node gets `Attach::Constrained { constraints }`, all to its single parent (another node or a platform); the parent becomes `Parent::Node`/`Parent::Platform`. Like Fusion's joints, each constraint removes some degrees of freedom; free ones keep the node's current pose (minimal move). Solve = repeat sequential projection (each constraint moves the child the least to satisfy itself) up to 20 rounds, stop below 1e-6 mm / 1e-6 rad. Conflicting constraints just converge to a compromise; no error UI in v1.
- **Kinds (v1):** `Mate` (plane–plane, normals opposed, `offset` along the normal), `Flush` (plane–plane, normals same, `offset`), `Concentric` (axis–axis coincident, `flip` reverses direction, `angle` twist about the axis is free unless `lock_rotation`), `Angle` (plane–plane or axis–axis, `angle` between them). Kind defaults from the pair: plane+plane → Mate, axis+axis → Concentric, plane+axis → rejected with toast "Pick two faces or two axes".
- **No schema bump:** new enum variant and `#[serde(default)]` field only; old files load unchanged.
- **View cube is drawn by Slint** (no Bevy fonts/render layers): the viewport reports the camera orientation, Rust projects the six cube faces into a 2D polygon model, Slint draws and labels them, clicks are hit-tested in Rust. Top-right of the viewport area, left of the inspector, ~96 px. Six faces + a Home (iso) button; edges/corners are a later item.
- **Orbit pivot follows selection without re-centring** the view: orbit rotates eye *and* focus around the pivot. Pivot = selected node's world origin; selected platform → top-surface centre; nothing / room → room centre. A view-cube click eases to that face's direction with `focus = pivot`. Pitch range becomes ±(π/2 − 0.01) so Bottom works.
- **Imported shading** (LOD1/2 normal smearing in `lab-import/src/lod.rs::cluster`) stays deferred, per the request.

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 1 | POLA-113 | A. Feature detection + smart import snap | Engineer Opus | `crates/lab-core/src/part.rs` (`Feature`, `PartDef.features`), `crates/lab-import/src/features.rs` (new) + its `mod`/`pub use` in `lib.rs`, `crates/lab-app/src/import.rs` (features + seat), `crates/lab-app/src/wizard.rs` (list detected rows) |
| 1 | POLA-114 | B. Constraint model + solver | Engineer Opus | `Attach::Constrained`, `Constraint`, `ConstraintKind` in `crates/lab-core/src/model.rs`, `crates/lab-mates/src/constraints.rs` (new) + its `mod` line, any `match attach` arm that stops compiling (minimal arm, no behaviour change) |
| 1 | POLA-115 | C. View cube + orbit pivot | Engineer Opus Low | `crates/lab-viewport/src/camera.rs`, new `ViewportRequest`/`ViewportEvent` variants in `crates/lab-viewport/src/lib.rs`, `crates/lab-ui/ui/view_cube.slint` (new) + its instance in `app.slint`, `crates/lab-app/src/view_cube.rs` (new) + `mod` line, its arms in `controller.rs` |
| 2 | POLA-116 | Merge stage 1 | Engineer Sonnet | merge commits, `Integration fix:` commits |
| 3 | POLA-117 | D. Constraint tool UI (pick, apply, inspector) | Engineer Opus | `crates/lab-viewport/src/picking.rs` (feature-pick mode), `crates/lab-viewport/src/feature_pick.rs` (new, highlight), `crates/lab-app/src/constrain.rs` (new), action rows + controller arms, Constraints section in `inspector.slint` + `ui_models.rs`/`edit.rs` |
| 4 | POLA-118 | Merge D, walkthrough, release `m8-draft` | Engineer Sonnet | merge commit, evidence, release per FIRST-DRAFT.md |

Stage-1 tasks own disjoint files. Shared rows (`ACTIONS`, controller `match`, `app.slint`) are appended at the end of their block; Sonnet keeps both sides at merge.

## A. Feature detection + smart import snap (Engineer Opus)

```rust
// lab-core/src/part.rs  (part frame, mm, +Z up)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Feature {
    Plane { origin: DVec3, normal: DVec3 },                 // normal points out of the material
    Axis { origin: DVec3, dir: DVec3, radius: f64, hole: bool }, // hole = concave (bore), else shaft
}
// PartDef gains:  #[serde(default, skip_serializing_if = "Vec::is_empty")] pub features: Vec<Feature>,

// lab-import/src/features.rs
/// The feature under a click: the cylinder through the hit if the surface is curved there
/// (fit_hole generalised to convex too), else the coplanar region's plane. `point`/`normal` in part space.
pub fn pick_feature(mesh: &TriMesh, point: DVec3, normal: DVec3) -> Option<Feature>;
/// Whole-mesh detection: planar regions ≥ 25 mm² (largest 12), cylinders with radius 0.5–30 mm
/// (seeded from triangles whose normal is ⟂ to a candidate axis; dedupe axes within 0.2 mm / 1°).
pub fn detect(mesh: &TriMesh) -> Vec<Feature>;
```
- Weld vertices (0.001 mm) before region growing; the merged mesh is unwelded.
- Import (`lab-app/src/import.rs`): `def.features = detect(&mesh)`. Holes (`Axis { hole: true }`) become connectors through `fit::suggest` (names `hole_1`, …) exactly like the wizard's Hole button. **Seat:** the hole on the bottom face (axis ‖ Z within 2°, its bottom within 0.5 mm of z = 0) nearest the bbox centre in XY with an M4–M6 / 8-32–1/4-20 size → `seat` origin at its axis, z = 0, `z: -Z`; none → today's origin seat.
- Wizard: detected connectors appear as normal rows the user can delete; no new UI widgets.
- Check: one unit test in `features.rs`: a generated box with a Ø6 mm through hole (`lab_parts` `base`/`riser` or a hand-built mesh) → `detect` returns the hole axis within 0.1 mm and the top plane; `pick_feature` on a side wall returns that wall's plane.

## B. Constraint model + solver (Engineer Opus)

```rust
// lab-core/src/model.rs
pub enum Attach { …, Constrained { constraints: Vec<Constraint> } }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub child: Feature,    // in the child's part frame
    pub parent: Feature,   // in the parent's frame (part frame for a node, platform frame for a platform)
    #[serde(default)] pub offset: Mm,      // Mate/Flush: along the normal; Concentric: along the axis
    #[serde(default)] pub angle: f64,      // radians; Angle kind, or Concentric twist when lock_rotation
    #[serde(default)] pub flip: bool,
    #[serde(default)] pub lock_rotation: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintKind { Mate, Flush, Concentric, Angle }

// lab-mates/src/constraints.rs
/// Child pose relative to its parent satisfying `cons`, moving the least from `current`.
pub fn solve(current: (DVec3, DQuat), cons: &[Constraint]) -> (DVec3, DQuat);
/// Default kind for a picked pair; None for plane+axis.
pub fn default_kind(child: &Feature, parent: &Feature) -> Option<ConstraintKind>;
```
- `Command`: no new variant; the UI uses `SetAttach { attach: Attach::Constrained { .. }, position, rotation }` with the solved pose, so undo is one step. Add `Command::constrain(doc, id, parent, constraints) -> Command` helper next to `Command::mate` that calls `solve` from the node's current pose (re-expressed in the new parent's frame).
- Existing `match attach` sites that stop compiling get a `Constrained { .. }` arm that behaves like `Free` (no slide/twist UI, gizmo moves are allowed and keep the attach; D re-solves).
- Check: unit tests in `constraints.rs`: (1) Concentric of a post axis to a bore axis puts the axes coincident and keeps the post's height; (2) adding a Mate end-face → bottom-face with offset 10 then 20 moves the post by exactly 10 mm along the axis; (3) serde round trip of `Attach::Constrained` through the project file format.

## C. View cube + orbit pivot (Engineer Opus Low)

- Viewport → app: `ViewportEvent::CameraOrient { yaw: f32, pitch: f32 }`, sent when it changes by > 0.002 rad (throttle, not every frame when idle).
- App → viewport: `ViewportRequest::ViewDirection { yaw: f32, pitch: f32 }` (ease, `focus = pivot`, keep distance; Top/Bottom also ease, perspective stays perspective) and `ViewportRequest::Pivot(Option<Vec3>)` is **not** needed: the viewport computes the pivot itself from `Selection` (node → its world origin; platform → top-surface centre from the platform root transform; none / room → room box centre).
- Orbit (`drag()`): rotate eye and focus about the pivot by the yaw/pitch deltas (yaw about world up through the pivot; pitch about the camera right axis through the pivot), then store the new focus/yaw/pitch. Pitch range ±(π/2 − 0.01) for drag and view-cube clicks. Top view mode (orthographic) is untouched.
- `lab-app/src/view_cube.rs`: `pub fn faces(yaw: f32, pitch: f32, size: f32) -> Vec<CubeFace { label, points: [(f32,f32);4], front_facing, yaw, pitch }>` (front-facing, back to front) and `pub fn hit(faces, x, y) -> Option<(f32,f32)>`. Labels Top/Bottom/Front/Back/Left/Right; Front looks along +Y of the room (match the existing default yaw convention; record which in the code).
- `view_cube.slint`: draws the front-facing faces as `Path` polygons (Theme surface colours, hovered face accent), a label at each face centre, a small Home button below (eases to the default perspective view around the pivot). Placed top-right of `viewport`, left of the inspector (same x rule as the view switcher), hidden in Top/Split mode.
- Check: run the app once, click Front, Top, Home; orbit with a node selected and confirm it stays in place on screen while the room turns around it.

## Stage 2: Merge stage 1 (Engineer Sonnet)

Merge `draft-POLA-113`, `draft-POLA-114`, `draft-POLA-115` into `draft` (`--no-ff`), keeping both sides of shared-row conflicts. `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import --offline` once. Report merge commits and results.

## D. Constraint tool UI (Engineer Opus)

- Action `tool.constrain` "Constrain" shortcut J (palette + shortcut sheet pick it up from `ACTIONS`). Esc cancels. Status hint text in the existing toast/hint area: "Pick a face or axis on the part to move" → "Pick a face or axis to constrain to".
- Feature-pick mode in the viewport: a click with the tool active sends `ViewportEvent::FeaturePoint { target: SelectionTarget, point: DVec3, normal: DVec3 }` (part or platform frame, mm), like `inspect.rs` does. Picking must work at any distance for imported parts: put `PickId` on every LOD or pick the visible one (today only LOD0, hidden beyond ~1.5 m).
- App (`constrain.rs`): node hit → snap to the nearest `PartDef.features` entry within 2 mm / 3° if any, else `lab_import::features::pick_feature` on the CPU mesh (generated: merged `PartMesh` pieces; imported: `wizard::part_mesh`). Platform hit → its top plane, or the nearest hole's axis (`Axis { hole: true }`, radius from the grid's hole diameter) when the hit is within one hole radius. Send `ViewportRequest::HighlightFeature(Option<(SelectionTarget, Feature)>)` (draw with `Gizmos<OnTop>`: plane = outlined square 20 mm, axis = line ±30 mm).
- Second pick: `default_kind`; plane+axis → toast, stay on step 2. Reject picking the moving part's own descendant (cycle) with a toast. If the moving node is already `Constrained` to the same parent, append; otherwise replace its attach. Emit `Command::constrain(..)`, label "Constrain", select the moved node, end the tool.
- Inspector: for a `Constrained` node, a "Constraints" section listing each row (kind dropdown limited to the valid kinds for its pair, offset mm, angle °, flip, lock rotation, delete). Every edit re-solves from the current pose and commits one `SetAttach`. Deleting the last row turns the node `Free` at its current pose.
- Gizmo drag of a constrained node: after the drag, re-solve so it stays constrained (it slides along its free DOF).
- Check: run the app once and do the exit criterion's constrain steps (post into holder, offset 10 → 20, undo).

## Stage 4: Merge D, walkthrough, release (Engineer Sonnet)

Merge `draft-POLA-117` into `draft`, `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import -p lab-app --offline` once. Walk the exit criterion once and report each step met / not met (not met → Defect log, no fix loop). Then the release per FIRST-DRAFT.md with tag `m8-draft`.
