# M9 plan: Fusion 360 UX (navigation, direct STEP, joints, floating part box, relationships bar)

New milestone after M8 (board request on POLA-121, 2026-10-22/23; parent POLA-124). Built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus the one check in its section), other flaws go to the Defect log (POLA-74, one line each), at most 2 attempts at any failure. Read this top part and your own section; open only the files it names. Branch from the current `draft`.

**Exit criterion (stage 4 walks it once):** (1) drag a KM100 STEP from Explorer onto the table: it appears seated on a hole within a few seconds; a second drag of the same file is instant. (2) The area around a hole on its top face picks as one flat plane. (3) Press J, click the KM100: the floating box opens; spin it, pick the mounting bore's circular edge, then the post's top edge in the main view; choose Revolute, limits 0–90°, Enter. (4) The bar shows "KM100 → Post · Revolute"; dragging the KM100 rotates it about the post only and stops at 90°. (5) Middle-drag pans, Shift+middle orbits, left-dragging the selected post holder moves it hole to hole; typing 50 + Enter while dragging moves it exactly 50 mm. (6) Right-click > Hide works on a body; Ctrl+Z shows "Undid: …". (7) Save and reopen: joints and bar unchanged; idle CPU/GPU returns to ~0 % with the floating box closed.

## What already exists (do not rebuild)

- **Actions/keys:** `ACTIONS: &[ActionInfo { id, title, group, keywords, shortcut }]` (`lab-app/src/actions.rs:16`). Keys are dispatched in Slint: global `capture-key-pressed` (`app.slint:443-534`: Ctrl+K, Ctrl+Z/Y/Shift+Z, Ctrl+N/O/S/C/X/V/D, Delete, arrows, Q/E) and the viewport `FocusScope` (`app.slint:585-643`: F, Shift+F, T, P/3, ?, M, J, Esc), both → `root.action(id)` → `controller.rs:335 run_action`. Shortcut sheet `lab-ui/ui/shortcuts.slint` (Mouse block `:63-68`), rows filled at `controller.rs:409-423` from `ACTIONS` + `NUDGE_KEYS`.
- **Camera** (`lab-viewport/src/camera.rs`): `handle_input` `:314` (pointer `:338-373`, scroll `:377-395`), `drag()` `:466` (today `pan = button != Left || shift || Top`), zoom toward cursor already (`zoom_toward` `:488`), selection pivot `orbit_pivot` `:443`. Clicks = left-up without drag → `Clicks` (`:366-371`) → `picking.rs:54 pick`; hover exists (`Selection.hovered`, `picking.rs:89-102`).
- **Body drag + hole snap:** `gizmo.rs`: `gizmo_input` `:303` (Move mode: press on the selected part = `Plane` handle `:336-345`), `Drag::update(doc, catalog, ray, shift, ctrl)` `:184` (Ctrl = 1 mm, Shift = free, `lab_mates::snap` → `Attach::Hole`) → `ViewportEvent::TransformRequested` → `lab-app/src/transform.rs:19`.
- **View cube:** `lab-app/src/view_cube.rs` (`CubeFace`, `faces`, `hit_index`, `hit`, `wire` → `ViewportRequest::ViewDirection`), `lab-ui/ui/view_cube.slint`. Six faces + Home only.
- **M8 constraints:** `Attach::Constrained { constraints }`, `Constraint`, `ConstraintKind` (`lab-core/src/model.rs:400-425`), `lab_mates::constraints::{solve, default_kind, constrain, world_pose}`, tool `lab-app/src/constrain.rs` (thread-local `FIRST`, `on_feature_point`, `resolve`/`snap`), `ViewportEvent::FeaturePoint` (`lab-viewport/src/lib.rs:271`, emitted `feature_pick.rs:86`, handled `bridge.rs:105`), `ViewportRequest::HighlightFeature` (`lib.rs:174`). Inspector rows `inspector.slint:277-311`, `constrain::rows/field_edit/action_edit`.
- **Import:** `lab_import::import_step` (`lab-import/src/lib.rs:104`), `run_step2glb` `:218`, LODs `:144-148` via `lod.rs:11 decimate` (vertex clustering, `cluster` `:39`). GLB cached by output hash `meshes/<blake3>.glb` (`:152-160`) but step2glb runs every time. App flow `lab-app/src/import.rs`: `choose_file` `:81` → `start` `:108` (thread) → `finish` `:249`; wizard `wizard.rs:127 open`. **No Explorer drag-drop anywhere.**
- **Library placement:** `library.rs:141 drag` → `ViewportRequest::DragPart(PartDrag::Move/Drop/Cancel)` → `lab-viewport/src/place.rs` (`placement` `:60`, `drag_part` `:201`, ghost `:265`) → `PlaceRequested` → `lab-app/src/place.rs:18`.
- **Undo:** `lab-core/src/history.rs` (`undo_label`/`redo_label` `:184/188`), `controller.rs:688 undo_redo` toasts "Undo <label>". Toast: `controller.rs:246 show_toast(ui, app, text, undo)`.
- **Hide/isolate:** no per-node hide. Platform isolate `ViewportRequest::Isolate(Option<PlatformId>)` → `isolate.rs:43`. Walls: `WallDisplay { Auto, Always, Hidden }` (`prefs.rs:18`, `room.rs:287-310`). `Node.locked` (`model.rs:452`). No viewport context menu (only `PartMenu` on library rows, `app.slint:1137`).
- **Rendering:** Bevy in its own thread rendering into a wgpu texture shown by Slint; `host.rs:311 run_on_demand` blocks when idle (`RedrawState::keep_alive`). Offscreen cameras already exist: `thumbnail.rs` (`RenderTarget::Image`), `export.rs`.
- File format: serde on `Attach`, `SCHEMA_VERSION = 3` (`model.rs:16`).

## Decisions

- **Joints replace M8 constraints for new work; constraints keep loading and working.** J becomes **Joint**; the M8 Constrain tool stays in the palette with no key.
- **One joint per child node, one parent** (same rule as M8): `Attach::Joint { joint }`. Fusion-style closed form, no iterative solve: child pose in parent frame = `P · T(offset, angle, flip) · D(values) · C⁻¹`, where `P`/`C` are the parent/child joint frames and `D` applies the kind's free DOF values. Drag changes `values` only, clamped to limits.
- **Ground = the existing `Node.locked`** (menu label Ground/Unground). **Rigid Group = Rigid joints** from each other selected node to the first one at the current relative pose; no new structure.
- **No schema bump:** new enum variants and `#[serde(default)]` fields only.
- **Mouse (Fusion):** left-click select; left-drag on the *selected* body drags it (any tool except Measure/Joint/Constrain picking); left-drag on empty space or an unselected body does nothing; **middle-drag pans; Shift+middle-drag orbits** about the M8 pivot; wheel zooms toward the cursor (exists); right-click (no drag) opens the body context menu; right-drag does nothing. Top view: middle-drag pans, Shift+middle does nothing.
- **Keys:** J Joint, M Move, I Measure, H Hide/Show toggle, Shift+H Show all bodies, S command palette (Ctrl+K stays), Ctrl+Shift+Z redo (Ctrl+Y stays), F / Shift+F frame (exist). **Q/E rotate → R / Shift+R** (+15° / −15°); Q, E, L, D unbound.
- **Explorer drop uses Win32 directly** (`WM_DROPFILES` via a window subclass, functions declared by hand with `extern "system"`, linked from shell32/comctl32/ole32): no Cargo feature changes (the build rule stands). If winit registered an OLE drop target on the HWND, `RevokeDragDrop` it first.
- **STEP cache by input hash:** blake3 of the STEP bytes stored in the library entry; a repeat insert of the same bytes reuses the part and skips step2glb.
- **Plane-preserving LOD:** quadric-error edge collapse (Garland–Heckbert) on a welded mesh, boundary/crease edges penalised; coplanar regions collapse with zero error so they stay flat. Same `decimate` signature.
- **Floating part box = second Bevy camera rendering to an image** on its own render layer, shown in a Slint floating panel; inactive and not rendered while closed.

## Stages and owners

| Stage | Task | Engineer | Owns (new or changed) |
|---|---|---|---|
| 1 | A. Navigation, keymap, view cube corners, numeric entry, undo toast | Engineer Opus Low | `lab-viewport/src/camera.rs`, `lab-viewport/src/gizmo.rs`, `DragValue` in `lab-viewport/src/lib.rs`, `lab-app/src/actions.rs` (remaps), key handlers in `app.slint`, `lab-ui/ui/shortcuts.slint`, `lab-app/src/view_cube.rs` + `view_cube.slint`, `controller.rs` (`undo_redo` text, drag-number arms) |
| 1 | B. Direct STEP insert + plane-preserving LOD | Engineer Opus | `lab-import/src/lod.rs`, `lab-import/src/lib.rs` (hash cache), `PartDef.source_hash` in `lab-core/src/part.rs`, `lab-app/src/import.rs`, `lab-app/src/drop.rs` (new) + hook in `main.rs`, `PlaceAt` in `lab-viewport/src/lib.rs` + its handling in `place.rs` |
| 1 | C. Joint model, solver, snap points | Engineer Opus | `Attach::Joint` + joint types in `lab-core/src/model.rs`, `lab-mates/src/joints.rs` (new) + `mod`, `lab-import/src/snaps.rs` (new) + `mod`, minimal `Joint` arms where `match attach` stops compiling |
| 1 | E. Floating part box | Engineer Opus | `lab-viewport/src/part_box.rs` (new) + plugin registration, `PartBox*` variants in `lab-viewport/src/lib.rs`, `lab-ui/ui/part_box.slint` (new) + instance in `app.slint`, `lab-app/src/part_box.rs` (new) + controller/bridge arms |
| 2 | Merge stage 1 | Engineer Sonnet | merge commits, `Integration fix:` commits |
| 3 | D. Joint tool UI, DOF drag, DOF glyph, joint inspector | Engineer Opus | `lab-app/src/joint.rs` (new), `lab-viewport/src/feature_pick.rs` (snap/hover drawing, preview pose), jointed-body branch in `gizmo.rs`, Joint section in `inspector.slint` + `ui_models.rs`, joint panel `lab-ui/ui/joint_panel.slint` (new) |
| 3 | F. Context menu, Hide, Isolate selection, walls toggle, body hover | Engineer Opus Low | `Node.hidden` + `Command::SetHidden` in `lab-core`, `lab-viewport/src/isolate.rs`, right-click in `camera.rs`/`picking.rs`, `lab-ui/ui/context_menu.slint` (new), `lab-app/src/context.rs` (new) |
| 3 | G. Relationships bar | Engineer Sonnet | `lab-app/src/relationships.rs` (new), `lab-ui/ui/relationships_bar.slint` (new) + instance in `app.slint`, `ViewportRequest::Highlight` + its drawing (`lab-viewport/src/outline.rs` or wherever the selection outline lives) |
| 4 | Merge stage 3, walkthrough, release `m9-draft` | Engineer Sonnet | merge commits, evidence, release per FIRST-DRAFT.md |

Same-stage tasks own disjoint files. Shared rows (`ACTIONS`, the controller `match`, `ViewportRequest`/`ViewportEvent` enums, `app.slint` instances, bridge arms) are appended at the end of their block; Sonnet keeps both sides at merge. Each task adds its own `ACTIONS` rows and key branches; only A changes existing ones.

## A. Navigation, keymap, view cube corners, numeric entry, undo toast (Engineer Opus Low)

- **Mouse** (`camera.rs`, `gizmo.rs`): implement the mouse decision above. Middle = pan, Shift+middle = orbit (keep the M8 pivot maths in `drag()`), left-drag on empty space / unselected body = nothing (no orbit, no box select), right button = nothing in camera.rs (F adds the menu). Left-press on the selected node in Select tool starts the existing `Plane` drag (hole snap, Shift free, Ctrl 1 mm), exactly like Move mode does today. A node with `Attach::Joint` or `Attach::Constrained`: leave the M8 behaviour; D adds the joint branch.
- **Keys:** `tool.move` M, `tool.measure` I, `palette.open` also S, `tool.rotate_right` R, `tool.rotate_left` Shift+R, remove Q/E. Keep F, Shift+F, T, P/3, ?, J, Esc. Update the `shortcut` column so the sheet and palette show the new keys, and rewrite the sheet's Mouse block: Select = Left-click · Drag selected body = Left-drag (Shift free, Ctrl fine) · Pan = Middle-drag · Orbit = Shift+Middle-drag · Zoom = Wheel · Context menu = Right-click · Exact value = type a number while dragging, Enter.
- **Numeric entry:** while a gizmo/body drag (`Plane`, arrow or ring) is held, digit / `.` / `-` / Backspace keys build a number shown in a small overlay near the cursor ("50 mm" or "15°"); Enter sends `ViewportRequest::DragValue(f64)`; the viewport applies it exactly and ends the drag with one `TransformRequested`: Plane = that many mm along the current drag direction (start → current, in the platform plane; zero displacement → +X), no hole snap; arrow = along the arrow; ring = degrees. Esc clears the number (a second Esc cancels the drag as today).
- **View cube corners and edges** (`view_cube.rs`, `view_cube.slint`): split each face into a 3×3 grid; centre cell = face (today), edge cells = the 12 edge directions (yaw/pitch halfway between the two faces), corner cells = the 8 corners (pitch ±35.26°, yaw ±45°/±135°). Extend `CubeFace` (or add `CubeCell` rows) so Slint highlights the hovered cell; `hit` returns that cell's (yaw, pitch).
- **Undo toast:** `undo_redo` shows "Undid: <label>" / "Redid: <label>".
- Check: run once: middle-drag pans, Shift+middle orbits, left-drag on empty space does nothing, left-drag on a selected post holder hops hole to hole, typing 50 Enter moves it 50 mm, click a cube corner, Ctrl+Z shows "Undid: Move".

## B. Direct STEP insert + plane-preserving LOD (Engineer Opus)

- **Drop** (`lab-app/src/drop.rs`): after the window exists (`main.rs`, where the HWND is fetched at `:228-239`), subclass the HWND (`SetWindowSubclass`), `DragAcceptFiles(hwnd, TRUE)`; on `WM_DROPFILES` read every path (`DragQueryFileW`) and the client drop point (`DragQueryPoint`), `DragFinish`, then post to the Slint event loop (`slint::invoke_from_event_loop`) a call `drop::on_files(paths, x, y)` in **physical px → convert to the viewport's logical coords**. Only `.step`/`.stp` (case-insensitive) are handled; others toast "Only STEP files can be dropped". Drop outside the viewport = place at the viewport centre.
- **Insert STEP** (existing `import.step` action, renamed title "Insert STEP…"): same path as a drop at the viewport centre.
- **Pipeline** (`lab-app/src/import.rs`): hash the STEP bytes (blake3, hex). If a library part has `source_hash == hash` → place it now. Otherwise toast "Converting <file>…", run the existing background `start` path, and on `finish` write the library entry silently (no wizard prompt; detected holes/seat from M8 as today), toast "Inserted <name>" (wizard stays in the library right-click), then place. Several drops queue one after another.
- `PartDef` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub source_hash: Option<String>`.
- **Placement under the cursor:** `ViewportRequest::PlaceAt { part: PartId, x: f32, y: f32 }` (viewport logical px). The viewport runs the same `placement(...)` as a library drop at that point (hole snap on the platform under the point, seat connector) and emits the normal `PlaceRequested`, so undo/selection behave like a library drop. No platform under the point → floor, like today.
- **LOD** (`lod.rs`): replace vertex clustering with quadric edge collapse on a welded mesh (weld 0.001 mm): per-vertex quadrics from incident triangle planes (area-weighted), boundary edges get a perpendicular penalty plane, reject a collapse that flips a triangle normal (> 90°) or makes a sliver (min angle < 2°), collapse to the cheaper endpoint or the optimal point when the 3×3 system is well conditioned. Keep `pub fn decimate(mesh: &TriMesh, target: usize) -> TriMesh` and every caller. Recompute normals per output triangle group so flat regions shade flat (this also fixes the LOD1/2 normal smearing on the Defect log).
- Check: one unit test in `lod.rs`: a 50×50×10 mm plate with a Ø6 hole meshed finely (≥ 20k tris), decimated to 10 %: every output vertex lying in the top face's area stays at z = 10 within 1e-4 mm and the triangle count ≤ target·1.1. Then run once: drag the KM100 STEP from Explorer onto the table (seated on a hole), drag it again (instant, no "Converting" toast).

## C. Joint model, solver, snap points (Engineer Opus)

```rust
// lab-core/src/model.rs
pub enum Attach { …, Joint { joint: Joint } }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointFrame { pub origin: DVec3, pub z: DVec3, pub x: DVec3 } // in the body's own frame (part frame for a node, platform frame for a platform); z, x unit and ⟂

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointKind { Rigid, Revolute, Slider, Cylindrical, PinSlot, Planar, Ball }

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limit { pub min: Option<f64>, pub max: Option<f64>, pub rest: Option<f64> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub kind: JointKind,
    pub child: JointFrame,
    pub parent: JointFrame,
    #[serde(default)] pub offset: Mm,        // along the parent frame's z
    #[serde(default)] pub angle: f64,        // radians about the parent frame's z
    #[serde(default)] pub flip: bool,        // child z opposite parent z (rotate π about x)
    #[serde(default)] pub values: [f64; 3],  // current DOF values (mm / radians), order below
    #[serde(default)] pub limits: [Limit; 3],
    #[serde(default)] pub suppressed: bool,  // behaves as Free at the current pose
    #[serde(default)] pub seq: u64,          // creation order (relationships bar)
}
```
DOF order per kind (axes of the joint frame after offset/angle/flip): Rigid none · Revolute [rot z] · Slider [trans z] · Cylindrical [rot z, trans z] · PinSlot [rot z, trans x] · Planar [trans x, trans y, rot z] · Ball [rot z, rot x, rot y] (applied in that order).

```rust
// lab-mates/src/joints.rs
pub enum Dof { Rot(Axis), Trans(Axis) }  pub enum Axis { X, Y, Z }
pub fn dofs(kind: JointKind) -> &'static [Dof];
/// Child pose in the parent's frame: P · T(offset, angle, flip) · D(values) · C⁻¹.
pub fn local_pose(j: &Joint) -> (DVec3, DQuat);
pub fn clamp(j: &Joint, values: [f64; 3]) -> [f64; 3];
/// DOF values that bring the grabbed child point (child frame) closest to the ray (parent frame), clamped.
/// Rotation: angle of the ray's hit on the plane ⟂ the axis through the grab point; translation: closest point on the axis/plane.
pub fn drag_values(j: &Joint, grab: DVec3, ray: (DVec3, DVec3)) -> [f64; 3];
/// SetAttach to `parent` with `seq = max seq in doc + 1` and values = rest (or 0), pose = local_pose.
pub fn joint(doc: &Document, id: NodeId, parent: Parent, joint: Joint) -> Option<Command>;
/// Rigid joints from ids[1..] to ids[0] at their current relative poses (identity-like frames), one Batch.
pub fn rigid_group(doc: &Document, ids: &[NodeId]) -> Option<Command>;
```
```rust
// lab-import/src/snaps.rs  (part space, mm)
pub enum SnapKind { FaceCentre, CircleCentre, Vertex, MidEdge }
pub struct Snap { pub kind: SnapKind, pub frame: JointFrame } // z = face normal / circle axis, out of the material
/// Joint-origin candidates for a click, nearest first: the clicked planar region's centroid, the
/// centre of the nearest circular edge (end ring of the cylinder fit under/near the click, axis as z),
/// and sharp-edge (dihedral > 30°) vertices and mid-edges within 3 mm of the hit.
pub fn joint_snaps(mesh: &TriMesh, point: DVec3, normal: DVec3) -> Vec<Snap>;
```
- Reuse `features.rs`'s welding, region growing and cylinder fit; do not duplicate them.
- `match attach` sites that stop compiling get a `Joint { .. }` arm that behaves like `Constrained` today (keeps the attach; D adds drag).
- Check: unit tests in `joints.rs`: (1) Revolute with limits 0..90° — `drag_values` for a ray at 120° returns 90°, `local_pose` keeps the child origin on the parent axis; (2) Slider moves exactly along z; (3) serde round trip of `Attach::Joint` through the project file format; plus one `snaps.rs` test: a box with a Ø6 through hole → a click near the hole's top rim yields a `CircleCentre` on the axis at the top face within 0.1 mm.

## E. Floating part box (Engineer Opus)

```rust
// lab-viewport/src/lib.rs
ViewportRequest::PartBox(Option<NodeId>),   // open for this node (replaces any open one) / close
ViewportRequest::PartBoxPointer { kind: PointerKind, x: f32, y: f32, button: PointerButton, shift: bool, delta: f32 }, // box-local logical px; reuse the main viewport's pointer types
ViewportEvent::FeatureHover { target: SelectionTarget, hit: Option<(DVec3, DVec3)> },  // part frame, mm
// clicks in the box emit the existing ViewportEvent::FeaturePoint with the same frames as the main view
#[derive(GizmoConfigGroup, Default, Reflect)] pub struct BoxGizmos;  // drawn only by the box camera, in part space
```
- On open: spawn copies of the node's visible LOD0 meshes (identity transform, part space) on a dedicated `RenderLayers` layer, a key light, and a camera with `RenderTarget::Image` (320×320 logical × scale factor) on that layer, orbiting the part's bbox centre (start: iso view, frame the bbox). Left-drag orbits, middle-drag pans, wheel zooms; left-click without drag ray-casts only the box copies and emits `FeaturePoint { target: Node(id), .. }`; hover emits `FeatureHover`. Close despawns everything; while closed nothing renders and idle stays ~0 (camera gone, no `keep_alive`).
- Hand the image to Slint the same way the main viewport texture reaches Slint (follow `host.rs`/`main.rs:124`); if that is impossible for a second texture, copy the image to a `slint::Image` once per rendered frame (only while interacting).
- `part_box.slint`: floating panel over the viewport's top-left, title "Pick on <name>", ✕ close, draggable by the title bar, image + TouchArea forwarding pointer events → `lab-app/src/part_box.rs` → `PartBoxPointer`. Action `view.part_box` (palette only, no key) opens it for the selected node, for the check.
- Check: run once: select a part, palette "Part box": the box shows the part, orbits without moving the room, a click logs a `FeaturePoint`; close it and `scripts/dev/idle.ps1` reads ~0 %.

## Stage 2: Merge stage 1 (Engineer Sonnet)

Merge A, B, C, E branches into `draft` (`--no-ff`), keeping both sides of shared-row conflicts. `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import --offline` once. Report merge commits and results.

## D. Joint tool UI, DOF drag, DOF glyph, joint inspector (Engineer Opus)

- Action `tool.joint` "Joint", key J (take J from `tool.constrain`, which stays in the palette without a key). Step hints in the existing hint area: "Pick a joint origin on the part to move" → "Pick the joint origin to attach to" → "Choose the joint type · Enter to accept, Esc to cancel".
- **Step 1:** a click on an unlocked node selects it as body 1 and opens the part box for it (`PartBox(Some(id))`). In both views, `FeatureHover` / main-view hover → `lab_import::snaps::joint_snaps` on the CPU mesh (generated: merged `PartMesh`; imported: `wizard::part_mesh`) → draw all candidates as small dots, the nearest bigger, plus its plane outline or axis line (`BoxGizmos` in the box, `Gizmos<OnTop>` in the main view). A click takes the nearest candidate as the child `JointFrame`. Platform targets: top-plane centre under the click or the nearest hole's axis (reuse `constrain.rs` platform logic).
- **Step 2:** pick on another node or a platform in the main view (cycle → toast, as M8). Close the box. Body 1 jumps to the joint pose as a live preview (`ViewportRequest::PreviewPose { id, position, rotation }` / clear), and the joint panel (`joint_panel.slint`, bottom-left of the viewport) opens: type (7 kinds, default Rigid), offset mm, angle °, flip, and per DOF min/max/rest. Every edit re-previews. Enter → `lab_mates::joints::joint(..)` as one command labelled "Joint"; Esc → clear preview, pose unchanged. Context-menu "Joint" (F) starts at step 1 with the right-clicked body.
- **DOF drag** (`gizmo.rs` branch): left-drag on a selected `Attach::Joint` body → `drag_values` from the grab point and the cursor ray, preview the pose, one `SetAttach` on release ("Move joint"); limits stop it; numeric entry (A's `DragValue`) sets the first DOF exactly (mm or °). Locked or Rigid bodies don't move.
- **DOF glyph:** selected jointed body → at the joint origin, a double arrow per translation DOF and an arc with arrowheads per rotation DOF (`Gizmos<OnTop>`), greyed past a limit.
- **Inspector:** Joint section for `Attach::Joint` nodes: kind, offset, angle, flip, current values, limits, suppressed, Delete (→ `Free` at the current pose). One `SetAttach` per edit. The M8 Constraints section stays.
- Rigid Group: action `joint.rigid_group` (palette + context menu when 2+ nodes are selected) → `rigid_group`.
- Check: run the exit criterion's steps 3–4 once (KM100 on a post, Revolute 0–90°, drag stops at 90°).

## F. Context menu, Hide, Isolate selection, walls toggle, body hover (Engineer Opus Low)

- `Node` gains `#[serde(default, skip_serializing_if = "std::ops::Not::not")] pub hidden: bool`; `Command::SetHidden { ids: Vec<NodeId>, hidden: bool }` (label "Hide"/"Show", undoable). The viewport hides those entities and their descendants and excludes them from picking/snapping. Actions `edit.hide_toggle` H (selection), `view.show_all_bodies` Shift+H.
- **Isolate selection:** `view.isolate` with node(s) selected → `ViewportRequest::IsolateNodes(Vec<NodeId>)`: only those nodes and their descendants stay visible (room, walls, platforms hidden like the platform isolate); `view.show_all` / Esc restores. Platform isolate unchanged.
- **Walls toggle:** action `view.toggle_walls` "Hide room walls" (palette + an icon toggle next to the view switcher): `Hidden` ↔ the previous `WallDisplay`.
- **Context menu:** right-click without drag (< 4 px) on a body: select it, `ViewportEvent::ContextMenu { target: SelectionTarget, x: f32, y: f32 }` (add the right-click to `camera.rs`' click path and `picking.rs`), and `context_menu.slint` at the cursor: Joint, Move, Rotate 90°, Duplicate, Ground/Unground (`locked`), Hide, Isolate, Delete; Rigid Group when 2+ nodes are selected. Each item runs the existing action id (`tool.joint`, `tool.move`, new `edit.rotate_90` = +90° about Z through the existing rotate command, `edit.duplicate`, new `edit.ground_toggle`, `edit.hide_toggle`, `view.isolate`, `edit.delete`, `joint.rigid_group`). Ids from D that are not merged yet just do nothing until stage 4. Esc / click elsewhere closes it.
- **Body hover:** the node under the cursor gets a light outline/tint (lighter than selection) when no tool is picking; skip if it already does that.
- Check: run once: right-click a post > Hide (gone), Ctrl+Z (back, "Undid: Hide"), Isolate it, Esc, walls toggle.

## G. Relationships bar (Engineer Sonnet)

- `lab-app/src/relationships.rs`: `pub fn chips(doc: &Document) -> Vec<Chip { text, kind, child: NodeId, parent: Parent, joint: bool, index: usize, suppressed: bool }>`: M8 constraints first (one chip per constraint, node order), then joints by `seq`. Text: `"<child name> → <parent name> · <Kind>"` (kind name as in the joint panel: Rigid, Revolute, Slider, Cylindrical, Pin-slot, Planar, Ball; constraints: Mate, Flush, Concentric, Angle). Rebuilt whenever the document changes.
- `relationships_bar.slint`: a strip along the bottom of the viewport area (chip height ~24 px, horizontal scroll, Theme colours, a small type glyph per kind, suppressed chips dimmed); hidden when there are no chips.
- Hover a chip → `ViewportRequest::Highlight(Vec<NodeId>)` (child + parent node) drawn like hover; leaving clears it. Click → select the child. Double-click → select the child and scroll the inspector to its Joint/Constraints section. Right-click → menu: Suppress/Unsuppress (joints only; one `SetAttach` toggling `suppressed`), Delete (joint → child `Free` at its current pose; constraint → remove that row, last row → `Free`), Select bodies (child + parent if the selection supports several, else the child).
- Check: run once with one M8 constraint and one joint (via a quick unit test of `chips` if the joint tool is not merged yet): chips show in order, hover highlights both, Delete removes one.

## Stage 4: Merge stage 3, walkthrough, release (Engineer Sonnet)

Merge D, F, G branches into `draft`, `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import -p lab-app --offline` once. Walk the exit criterion once (steps 1–7) and report each step met / not met (not met → Defect log, no fix loop). Then the release per FIRST-DRAFT.md with tag `m9-draft`.
