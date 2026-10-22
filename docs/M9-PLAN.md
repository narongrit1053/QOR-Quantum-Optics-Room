# M9 plan: Fusion 360 UX (navigation, direct STEP, joints, floating part box, relationships bar)

Board request on POLA-121 (2026-10-22/23), parent POLA-124. Built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus the one check in its section), other flaws go to the Defect log (POLA-74, one line each), at most 2 attempts at any failure. Read this top part and your own section; open only the files it names. Worktree `D:/Try/LabSetup-wt/<KEY>` on branch `draft-<KEY>` from the current `draft` (M8 merged, `7c18fd3` or later). The cargo target was cleaned by POLA-126: your first build takes long; wait for the lock.

**Exit criterion (stage 5 walks it once):** (1) drag a KM100 STEP from Explorer onto the table: it appears seated on a hole within a few seconds; a second drag of the same file is instant. (2) The area round a hole on its top face picks as one flat plane. (3) Press J, click the KM100: the floating box opens; spin it, pick the mounting bore's circular edge, then the post's top edge in the main view; choose Revolute, limits 0–90°, Enter. (4) The bar shows "KM100 → Post · Revolute"; dragging the KM100 rotates it about the post only and stops at 90°. (5) Middle-drag pans, Shift+middle orbits, left-dragging the selected post holder moves it hole to hole; typing 50 + Enter while dragging moves it exactly 50 mm. (6) Right-click > Hide works on a body; Ctrl+Z shows "Undid: …". (7) Save, reopen: joints and bar unchanged; idle CPU/GPU back to ~0 % with the floating box closed.

## What already exists (reuse, do not rebuild)

- **Camera input** (`lab-viewport/src/camera.rs`): `handle_input` (`:314`) turns `ViewportInput` into orbit/pan/zoom; `drag()` (`:466`) pans when `button != Left || shift || Top`, else orbits about `orbit_pivot()` (`:442`, M8 selection pivot). Wheel already zooms toward the cursor (`zoom_toward`, `:488`). Left clicks without drag go to `Clicks` for picking. `GizmoCapture` makes a press belong to the gizmo.
- **Gizmo drag** (`lab-viewport/src/gizmo.rs`): `handles(doc, catalog, node, tool)` (`:93`); `Tool::Move` grabs the ring *or the selected part itself* as `GizmoHandle::Plane`; `Drag::update` (`:184`) snaps to platform holes (`lab_mates::snap`; Shift free, Ctrl 1 mm), emits `ViewportEvent::TransformRequested { .., done }`. Constrained nodes are re-solved by the app after a drag (POLA-120).
- **Actions/keys**: `lab-app/src/actions.rs` `ACTIONS` (palette + shortcut sheet); single keys in `lab-ui/ui/app.slint` `key-pressed` (`:585-640`), Ctrl keys in `capture-key-pressed` (`:443-530`, Ctrl+Shift+Z redo already works, Q/E rotate at `:519-526`); sheet `lab-ui/ui/shortcuts.slint` (Mouse block `:63`). Unknown action ids are ignored by `controller.rs` `on_action`.
- **View cube**: `lab-app/src/view_cube.rs` `faces()`/`hit()`, `view_cube.slint`, `ViewportRequest::ViewDirection { yaw, pitch }`.
- **Features**: `lab_import::features::{pick_feature, detect}` (`features.rs`), `fit::fit_hole`, `PartDef.features`, `lab_core::part::{Feature, Frame { origin, z, x }}`.
- **Constraints (M8)**: `Attach::Constrained { constraints }`, `lab_mates::constraints::{solve, default_kind, constrain, world_pose}`, tool UI `lab-app/src/constrain.rs`, `ViewportEvent::FeaturePoint`, `ViewportRequest::HighlightFeature`, `Tool::Constrain`.
- **Import**: `lab_import::import_step` (blocking, stores the GLB by *output* hash), `lab-app/src/import.rs` `start()` (thread, `detect`, `seat_frame`, `hole_connectors`, `write_part`); LODs via `lod::decimate` (vertex clustering, also used for LOD0 when over `LOD0_BUDGET`).
- **Placement**: `ViewportRequest::DragPart(PartDrag::Move/Drop { pos, mods })` ghosts a catalog part under the cursor and seats it on a hole → `ViewportEvent::PlaceRequested`. Direct STEP reuses this.
- **Undo**: `lab_core::History` keeps a label per entry (`execute_labeled`, `execute_coalescing` + `end_gesture`, `undo`); `controller::show_toast(ui, app, text, undo)`.
- **Walls**: `WallDisplay::{Auto, Always, Hidden}` + `ViewportRequest::SetWallDisplay`. **Isolate**: `ViewportRequest::Isolate(Option<PlatformId>)` (`isolate.rs`). **Lock**: `Node.locked`.

## Decisions

- **Mouse (Fusion):** middle-drag pans; Shift+middle-drag orbits about the M8 pivot; wheel zoom unchanged. Left press on the **selected** body drags it in *every* tool except Measure/Constrain/Joint (the Select tool gets the `Plane` handle; jointed bodies get their DOF drag, stage 4). Left-drag on empty space and right-drag do nothing. In Top view middle-drag pans and Shift+middle also pans. A right click without drag opens the context menu.
- **Keys:** J Joint, M Move tool, I Measure, H Hide/Show selection, S command palette (Ctrl+K stays), F / Shift+F frame (exist), Ctrl+Shift+Z redo (exists). **Rotate ±15° moves from Q/E to R (+15°) / Shift+R (−15°)** (single keys match the rest; Ctrl+Q reads as quit on many apps). E, L, Q, D are unmapped. M8 Constrain keeps its palette row without a key.
- **Joints are a new `Attach::Joint`, closed form, no solver.** Child pose = P · offset · motion(values) · C⁻¹ with P, C the two picked origin frames. M8 `Constrained` stays as is and still loads. **Ground = the existing `Node.locked`** (palette row "Ground", no new field). **Rigid Group** = Rigid joints from every other selected node to the first, one undo step.
- **Joint order** for the bar: `Joint.seq`, assigned `doc max seq + 1` at creation (no schema bump: `#[serde(default)]`). M8 constraint chips follow the joints, in node order.
- **Live preview without ghosts:** the Joint tool applies the joint for real as a coalescing gesture (`execute_coalescing`, key `"joint"`); every panel edit re-applies it; Enter = `end_gesture`; Esc = `end_gesture` + `undo`. One undo step either way.
- **Hide and isolate-selection are view state**, not document data: `AppState.hidden: BTreeSet<NodeId>`, not saved, not undoable (Fusion behaves the same).
- **Direct STEP cache key = blake3 of the STEP bytes**, stored as `PartDef.meta["step_hash"]`; a library part with the same hash is reused without converting. Old imports (no `step_hash`) are not matched, so they get the new decimator on their next drop.
- **Explorer file drop:** Slint 1.18 does not forward winit's `DroppedFile`. Enable `slint`'s `unstable-winit-030` feature in the workspace `Cargo.toml` (the *only* sanctioned feature change in M9; the winit crate is already in the offline registry) and use `slint::winit_030::WinitWindowAccessor::on_winit_window_event`. Cursor position at drop: Win32 `GetCursorPos` + `ScreenToClient` (winit's `DroppedFile` carries none). If that build fails twice offline, fall back to Insert STEP only and log it.
- **Floating part box = a second Bevy camera** on its own render layer drawing one node's mesh copy into a second texture; spawned on open, despawned on close (no idle cost). Picks in it send the same `ViewportEvent::FeaturePoint` as the main view, so the Joint tool does not care where a pick came from.

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 1 | POLA-125 | Plan | Lead | this file |
| 1 | POLA-126 | Cleanup (disk) | Engineer Sonnet | worktrees, builds, `target` |
| 2 | POLA-127 | A. Fusion navigation, keys, view cube edges/corners | Engineer Opus Low | `camera.rs` (`drag`, `handle_input` button rules), `gizmo.rs` `handles()` Select arm only, `actions.rs`, `app.slint` key handlers, `shortcuts.slint`, `view_cube.rs`, `view_cube.slint` |
| 2 | POLA-128 | B. Direct STEP insert + plane-preserving LOD | Engineer Opus | `lab-import/src/lod.rs`, `lab-import/src/lib.rs` (`import_step` hash), `lab-app/src/import.rs` (`quick_insert`), `lab-app/src/file_drop.rs` (new), workspace `Cargo.toml` slint feature, `main.rs`/`lib.rs` one wiring line each |
| 2 | POLA-129 | C. Joint model, pose, snaps | Engineer Opus | `lab-core/src/model.rs` (`Attach::Joint`, `Joint`, `JointKind`, `Limit`), `lab-mates/src/joints.rs` (new), `lab-import/src/snaps.rs` (new), their `mod` lines, minimal `match attach` arms elsewhere |
| 2 | POLA-130 | D. Context menu, hide/isolate/walls, undo toast, hover, tool hint | Engineer Opus Low | `lab-app/src/context_menu.rs` (new), `lab-ui/ui/context_menu.slint` (new), `lab-viewport/src/isolate.rs`, `lab-viewport/src/outline.rs` (hover), `camera.rs` right-click (3 lines) + `picking.rs` `ContextMenu` emission, `lab-core/src/history.rs` (`undo_label`/`redo_label`), undo/redo arms in `controller.rs`, `tool-hint` property + label in `app.slint` |
| 3 | POLA-131 | Merge stage 2 | Engineer Sonnet | merge commits, `Integration fix:` commits |
| 4 | POLA-132 | E. Joint tool + panel + inspector | Engineer Opus | `lab-app/src/joint.rs` (new), `lab-ui/ui/joint_panel.slint` (new), inspector Joint section (`inspector.slint`, `ui_models.rs`, `edit.rs`), `Tool::Joint` + hover feature events in `picking.rs`/`feature_pick.rs` |
| 4 | POLA-133 | E2. DOF drag, numeric entry, DOF glyph | Engineer Opus Low | `gizmo.rs` (`Drag` for `Attach::Joint`, typed value), DOF glyph in `feature_pick.rs` (append-only, E also edits it), numeric field in `overlays.slint`, its `controller.rs` arms |
| 4 | POLA-134 | F. Floating part box | Engineer Opus | `lab-viewport/src/part_box.rs` (new), `host.rs` (second texture), `lab-ui/ui/part_box.slint` (new) + instance in `app.slint`, `lab-app/src/part_box.rs` (new) |
| 4 | POLA-135 | G. Relationships bar | Engineer Opus Low | `lab-app/src/relations.rs` (new), `lab-ui/ui/relations_bar.slint` (new) + instance in `app.slint`, `ViewportRequest::HighlightNodes` handling in `outline.rs` (append) |
| 5 | POLA-136 | Merge stage 4, walkthrough, release `m9-draft` | Engineer Sonnet | merge commits, evidence, release per FIRST-DRAFT.md |

Tasks in one stage own disjoint files except the shared rows (`ACTIONS`, the `controller.rs` action `match`, `app.slint` instances, `lib.rs` enum variants, `mod` lines): append at the end of their block; Sonnet keeps both sides at merge.

**Shared interfaces fixed now** (add exactly these; the owner implements, others code against them):

```rust
// lab-viewport/src/lib.rs
pub enum Tool { …, Joint }                               // C adds the variant (stage 2); E gives it behaviour
pub enum ViewportRequest { …,
    SetHidden(Vec<lab_core::NodeId>),                    // D: these nodes and their descendants are not drawn or picked
    IsolateNodes(Option<Vec<lab_core::NodeId>>),         // D: show only these nodes (+descendants, + their root platform); hide room/walls/fixtures like Isolate
    HighlightNodes(Vec<lab_core::NodeId>),               // G: hover outline on these (empty clears); D draws it with the hover outline
    PartBox(Option<PartBoxSpec>),                        // F
    PartBoxInput(ViewportInput),                         // F: input in the box's own physical px
    DragValue(f64),                                      // E2: typed value (mm or degrees) for the drag in progress; applies exactly and ends it
}
pub struct PartBoxSpec { pub node: lab_core::NodeId, pub width: u32, pub height: u32 }
pub enum ViewportEvent { …,
    ContextMenu { target: Option<SelectionTarget>, pos: Vec2 },          // D: right click without drag (selects the target first)
    FeatureHover { target: SelectionTarget, point: DVec3, normal: DVec3 },// E: like FeaturePoint, on pointer move while Tool::Joint/Constrain is active, throttled to one per frame
    DragStarted { id: lab_core::NodeId, unit: DragUnit },                // E2: a gizmo/DOF drag began (the app shows the numeric field)
}
pub enum DragUnit { Mm, Degrees }
```

New action ids (A adds every row and key, handlers live with their owner; until merged an id is a no-op): `tool.joint` "Joint" J (A's stage-2 handler starts the M8 constrain tool; E replaces it), `tool.measure` I, `tool.move` M, `view.hide` "Hide/Show" H (D), `palette.open` also S, `tool.rotate_right` R, `tool.rotate_left` Shift+R, `view.walls` "Hide Room Walls" (D), `view.isolate_selection` "Isolate Selection" (D), `edit.rotate_90` "Rotate 90°" (D), `tool.ground` "Ground" (E), `tool.rigid_group` "Rigid Group" (E), `view.part_box` "Open Part Box" (F).

## A. Fusion navigation, keys, view cube edges/corners (Engineer Opus Low)

- `camera.rs`: `drag()` pans for `Middle` without Shift (and always in Top), orbits for `Middle`+Shift; a `Left` or `Right` drag that the gizmo did not capture changes nothing (no orbit, no pan, no click). Keep the pivot logic. A right release without drag is D's (leave a clear spot; D adds 3 lines).
- `gizmo.rs` `handles()`: `Tool::Select` returns the same as `Tool::Move` for the `Plane` handle only (no ring, no slide arrow drawn: the part itself is the handle). Nothing else in gizmo.rs.
- Keys and rows as listed above; remove Q/E. Shortcut sheet: new rows come from `ACTIONS`; rewrite the Mouse block: "Middle-drag: pan · Shift+Middle-drag: orbit · Wheel: zoom to cursor · Left-click: select · Left-drag selected part: move (Shift free, Ctrl 1 mm) · Right-click: menu".
- View cube: `faces()` gains the 12 edges and 8 corners as clickable regions (chamfer bands ~18 % of the face size along each edge, corner squares where they meet); `hit()` returns the yaw/pitch of the edge (45° between its two faces) or corner (iso: yaw ±45°/±135°, pitch ±35.26°). Hover accent like the faces.
- Check: run the app once: middle pans, Shift+middle orbits round the selected node, left-drag on empty does nothing, left-drag on a selected post holder moves it hole to hole, R turns it, a view-cube corner gives an iso view.

## B. Direct STEP insert + plane-preserving LOD (Engineer Opus)

```rust
// lab-app/src/import.rs
/// Converts `step` in the background (or reuses the library part with the same STEP hash),
/// writes the library entry silently, reloads the catalog, then places it at `pos`
/// (viewport px) with DragPart Move+Drop so it seats on a hole. Toast on error.
pub fn quick_insert(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, step: PathBuf, pos: Option<Vec2>);
```
- Hash: `blake3` of the STEP file bytes. Catalog lookup: any part with `meta["step_hash"] == hex` → place it immediately. Else run the same body as `start()` (factor it out, keep its error handling), with defaults: vendor from the file name (Thorlabs naming as in `choose_file`, else "Imported"), part number/name = cleaned stem, category `imported`, finish anodised black, source up Y; add `meta["step_hash"]`. Progress toast "Converting <name>…". The wizard (Import Vendor Part…) stays as it is.
- Entry points: `import.step_insert` "Insert STEP…" (file dialog → `quick_insert(.., None)`; `None` = centre of the viewport), and Explorer drop (`file_drop.rs`: `.step`/`.stp` only; others → toast "Only STEP files can be dropped"). See Decisions for the winit hook and cursor position.
- `lod.rs`: replace vertex clustering with **quadric-error edge collapse** (Garland–Heckbert, on welded vertices 0.001 mm; boundary/crease edges > 30° get penalty planes so silhouettes and hole rims stay; reject collapses that flip a normal). Coplanar regions then stay exactly planar. Same `decimate(mesh, target)` signature; LOD0 over budget uses it too. Keep normals per face group (no smearing across creases > 30°).
- Check: one unit test in `lod.rs`: a 100×100×10 mm plate with a Ø6 mm hole, decimated to 25 %: every output vertex that came from the top face is within 0.01 mm of z = 10, and `features::pick_feature` 4 mm from the hole edge on the top returns `Plane` with normal +Z. Then drop a STEP file once and drop it again (second is instant).

## C. Joint model, pose, snaps (Engineer Opus)

```rust
// lab-core/src/model.rs
pub enum Attach { …, Joint(Joint) }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub kind: JointKind,
    pub child: Frame,                 // joint origin in the child's part frame (z = joint axis)
    pub parent: Frame,                // in the parent's frame (part frame / platform frame)
    #[serde(default)] pub offset: DVec3,   // mm, in the parent origin frame
    #[serde(default)] pub angle: f64,      // rad, about the parent origin z
    #[serde(default)] pub flip: bool,      // child z runs against parent z
    #[serde(default)] pub values: [f64; 3],        // current DOF values, order = dofs(kind)
    #[serde(default)] pub limits: [Option<Limit>; 3],
    #[serde(default)] pub suppressed: bool,        // pose kept, motion and re-solve off
    #[serde(default)] pub seq: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit { pub min: f64, pub max: f64, pub rest: f64 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JointKind { Rigid, Revolute, Slider, Cylindrical, PinSlot, Planar, Ball }

// lab-mates/src/joints.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dof { RotZ, SlideZ, SlideX, SlideY, RotX, RotY }   // in the parent origin frame
pub fn dofs(kind: JointKind) -> &'static [Dof];   // Revolute [RotZ], Slider [SlideZ], Cylindrical [RotZ, SlideZ], PinSlot [RotZ, SlideX], Planar [SlideX, SlideY, RotZ], Ball [RotX, RotY, RotZ], Rigid []
pub fn joint_pose(j: &Joint) -> (DVec3, DQuat);   // child pose relative to its parent; values clamped to limits
pub fn clamp(j: &mut Joint);
/// SetAttach for `id` jointed to `parent` (seq assigned), pose from `joint_pose`.
pub fn joint(doc: &Document, id: NodeId, parent: Parent, j: Joint) -> Option<Command>;
/// Rigid joints from ids[1..] to ids[0] at their current relative poses; one Batch.
pub fn rigid_group(doc: &Document, ids: &[NodeId]) -> Option<Command>;
pub fn next_seq(doc: &Document) -> u64;

// lab-import/src/snaps.rs  (part frame, mm)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind { FaceCentre, EdgeCentre, Vertex, MidEdge }
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap { pub kind: SnapKind, pub frame: Frame }   // z out of the material (face normal / circle axis)
/// Best snap near a hit, priority: circular edge centre (rim of a fitted cylinder within `radius`),
/// vertex, mid-edge (sharp edges > 30°) within `radius`, else the centroid of the coplanar region.
pub fn snap_at(mesh: &TriMesh, point: DVec3, normal: DVec3, radius: f64) -> Option<Snap>;
```
- `Command::label` for a `SetAttach` with `Attach::Joint` reads "Joint <kind>". `match attach` sites that stop compiling get a `Joint(_)` arm behaving like `Free` (E2 adds the real drag). M8 files load unchanged.
- Add `Tool::Joint` to `lab-viewport/src/lib.rs` and the `handles()` arm (no handles), nothing more.
- Check: unit tests in `joints.rs`: (1) Revolute, value 90° about a vertical parent axis, turns the child 90° and keeps the axis points coincident; values beyond a 0–90° limit clamp to 90°; (2) Slider value 10 moves 10 mm along the axis; (3) serde round trip of `Attach::Joint` in the project format; plus one `snaps.rs` test: a generated post's top rim returns `EdgeCentre` on the axis at the top.

## D. Context menu, hide/isolate/walls, undo toast, hover, tool hint (Engineer Opus Low)

- `ContextMenu` event (see interfaces): `camera.rs` pushes right releases without drag to a new `RightClicks` resource; `picking.rs` resolves the target like a left click, selects it, sends the event. Also Shift+F10 / the Menu key on the selection opens it at the node's projected origin.
- Menu (`context_menu.slint`, a Slint `PopupWindow` at `pos`): Joint, Move, Rotate 90°, Duplicate, Hide, Isolate, Delete for a node; Isolate Plate, Hide Room Walls for a platform; Hide Room Walls / Show All for empty space. Rows dispatch action ids.
- Hide: `view.hide` toggles the selection in `AppState.hidden` → `SetHidden`; with nothing selected it shows all. `view.isolate_selection` → `IsolateNodes(Some(selection))`; Esc / Show All releases both. `view.walls` toggles `WallDisplay::Hidden` ↔ the previous setting. `edit.rotate_90`: reuse the rotate-nudge path with 90°.
- Undo toast: `History::undo_label(&self) -> Option<&str>` / `redo_label` (peek before undo); toast "Undid: <label>" / "Redid: <label>".
- Hover: outline the node under the cursor (thinner/fainter than the selection outline, same `outline.rs` machinery), plus `HighlightNodes`. Tool hint: a `tool-hint` string property shown bottom-centre of the viewport while non-empty; `controller::set_hint(ui, &str)`; the constrain and measure tools set their step text there instead of toasts.
- Check: run the app once: right-click a post > Hide hides it, H shows it again, Ctrl+Z after a move shows "Undid: Move …".

## Stage 3: Merge stage 2 (Engineer Sonnet)

Merge `draft-POLA-127` … `draft-POLA-130` into `draft` (`--no-ff`), keeping both sides of shared-row conflicts (both edits in `camera.rs` survive). `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import -p lab-app --offline` once. Report merge commits and results.

## E. Joint tool + panel + inspector (Engineer Opus)

- `tool.joint` (J) sets `Tool::Joint`; hint "Pick a joint origin on the part to move". While active, pointer moves send `FeatureHover`; the app (`joint.rs`) runs `snaps::snap_at` (radius 3 mm, CPU mesh as `constrain.rs` gets it; platform hit → nearest hole centre with z = +Z, else hit point) and shows it with `HighlightFeature` (extend `feature_pick.rs` with a snap glyph: small circle for edge centre, dot for vertex/mid-edge, square for face centre).
- First pick (body 1, child) → open the part box (`part_box::open(ui, app, id)`, F) → hint "Pick the origin to join to". Picks from the box or the main view both arrive as `FeaturePoint`; a pick on body 1 again replaces origin 1. Second pick on another body or platform (reject body 1's descendants, toast) → close the box, apply the joint as a coalescing gesture (Decisions), open `joint_panel.slint` next to the inspector: type (7 kinds), offset x/y/z mm, angle °, flip, per-DOF min/max/rest, Enter/OK, Esc/Cancel. Each edit re-applies.
- Default kind: axis–axis (edge centres) → Revolute; face–face → Rigid; else Rigid. Body 1's start values = its current pose projected onto the DOFs (no jump beyond the joint itself).
- Inspector: a Joint section for `Attach::Joint` nodes (same fields as the panel, each edit one `SetAttach`, delete → `Free` at the current pose). `tool.ground` toggles `locked` on the selection; `tool.rigid_group` runs `joints::rigid_group` on a multi-selection (≥ 2, else toast).
- Check: run the app once: J on a post holder, pick its bore edge, pick a post's top edge, Revolute, Enter; the panel closes and the inspector shows the joint; Esc in a second try leaves nothing changed.

## E2. DOF drag, numeric entry, DOF glyph (Engineer Opus Low)

- `gizmo.rs`: for a selected `Attach::Joint` node (not suppressed, not locked), a left press on the body starts a joint drag: one DOF → `RotZ` = angle of the cursor ray's hit on the plane ⟂ the axis through the joint origin, `Slide*` = `line_param` along that axis; two/three DOFs → the DOF whose screen direction best matches the first 6 px of movement. Values go through `joints::clamp` and `joint_pose`; emit `TransformRequested` with the updated `Attach::Joint`. Shift free, else 1 mm / 5° steps. Rigid: no drag. Send `DragStarted` on every gizmo/joint drag start.
- Numeric entry: on `DragStarted` the app shows a small field at the cursor (`overlays.slint`); digits, `.`, `-` and Backspace go into it while the pointer is down; Enter sends `DragValue(v)`: a plane drag moves exactly `v` mm along the current drag direction from the start, a ring/RotZ drag turns `v`°, a slide sets `v` mm; it ends the drag (`done: true`). Esc clears the field.
- DOF glyph (`feature_pick.rs`, append): on the selected jointed node draw `Gizmos<OnTop>` arrows for each `Slide*` and an arc for each `Rot*` at the joint origin, only while selected (no continuous redraw when idle).
- Check: one unit test for the plane drag with `DragValue(50.0)` (exactly 50 mm), then run the app once: a revolute body drags round its axis and stops at its limit.

## F. Floating part box (Engineer Opus)

```rust
// lab-app/src/part_box.rs
pub fn open(ui: &AppWindow, app: &Rc<RefCell<AppState>>, node: NodeId);   // also `view.part_box` on the selected node
pub fn close(ui: &AppWindow, app: &Rc<RefCell<AppState>>);
```
- `lab-viewport/src/part_box.rs`: on `PartBox(Some(spec))` spawn a camera (own `RenderLayers`, own `OrbitCamera` framed on the part, iso start) rendering a copy of that node's meshes (identity pose, part frame) into a `spec.width × spec.height` texture; on `None` despawn everything. `PartBoxInput` drives its orbit (left-drag orbits here: it is a part viewer; wheel zooms) and its left clicks raycast the copy and send `ViewportEvent::FeaturePoint { target: Node(id), .. }` (and `FeatureHover` while `Tool::Joint`). The main camera ignores the copy's layer.
- `host.rs`: deliver the box texture to the UI next to the main one (same recycle path); render frames on demand only while the box is open.
- `part_box.slint`: a floating, draggable panel (~320×320 px, title = part name, close button) over the viewport, an `Image` of the texture, pointer events forwarded as `PartBoxInput`.
- Check: run the app once: select a part, run "Open Part Box", orbit inside it (the room does not turn), click a face (the app logs/toasts the `FeaturePoint`), close it; `scripts/dev/idle.ps1` shows ~0 % afterwards.

## G. Relationships bar (Engineer Opus Low)

- `relations.rs`: `pub fn chips(doc: &Document) -> Vec<Chip { node: NodeId, parent: Parent, label: String, icon: String, suppressed: bool }>`: every `Attach::Joint` by `seq`, then every M8 `Constrained` constraint in node order; label "<child name> → <parent name> · <kind>".
- `relations_bar.slint`: a strip along the viewport bottom (above the status line, hidden when there are no chips), horizontally scrollable chips. Hover → `HighlightNodes([child, parent node])`; click → select the child; double-click → select it and scroll the inspector to its Joint/Constraints section; right-click menu: Suppress/Unsuppress (toggle `suppressed`, one `SetAttach`; M8 constraints: Suppress disabled), Delete (attach → `Free` at the current pose), Select bodies (select child; parent node if any).
- Refresh on every document change (same place the scene list refreshes).
- Check: one unit test for `chips` order on a document with two joints and one constraint, then run the app once and hover/click a chip.

## Stage 5: Merge stage 4, walkthrough, release (Engineer Sonnet)

Merge `draft-POLA-132` … `draft-POLA-135` into `draft`, `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-mates -p lab-import -p lab-app --offline` once. Walk the exit criterion once and report each step met / not met (not met → Defect log, no fix loop). Then the release per FIRST-DRAFT.md with tag `m9-draft`.
