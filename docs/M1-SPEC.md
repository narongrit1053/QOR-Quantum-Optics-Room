# LabSetup M1 Specification: Core, Platforms, Room, Persistence

Status: final synthesis, ready to implement. Base: `main` at `86c212d` (M0 done, DESIGN.md §13).
Base design: **B-robust**. It is augmented with judge-recommended ideas from A-lean and C-experience, and every fatal flaw the judges listed is fixed (§0.2).

Nobody implementing this saw the design discussion. Every decision below is final. A task that needs to change another task's interface stops and asks; it does not edit files it does not own.

Editorial pass (final editor): this revision applies the verifier corrections. They cover the HighlightLook struct variants, the WallState fields, the wall-fade eye source, Slint Path stroke and label maths, Tooltip styled-text, the Slint struct leaf file (`types.slint`), the controlled SegmentedControl, the M0 members that are removed, Shift+F, and sheet focus handling.

---

## 0. Decisions

### 0.1 Decisions at a glance

| # | Decision |
|---|---|
| D1 | New pure crate **`lab-geom`** (deps: `glam`, `thiserror`). It holds polygon validation, fillets, triangulation, distances, extrusion and mitred walls. `lab-core` uses it for validation and holes, `lab-viewport` for meshes. |
| D2 | New crate **`lab-file`**. It holds the zip container, atomic writes, project load/save, autosave and recovery, user settings, and the I/O worker thread. `lab-core` stays free of disk I/O and threads. |
| D3 | **No new external runtime crates.** Deflate and CRC come from `flate2 = "1.1"` with default features. `flate2 1.1.10` is already in the runtime tree with exactly these features (default = `rust_backend` + `runtime_detection`, via png and usvg), so nothing new compiles. Win32 dialogs come from new features on `windows-sys 0.60.2`, which only `lab-app` uses. `tempfile 3.27.0` (already in Cargo.lock) is a dev-dependency of `lab-file` only. **No Bevy feature changes.** |
| D4 | **Which holes exist** is computed once, in `lab_core::holes::compute_holes` (pure, f64, deterministic, brute-force-oracle tested). The viewport draws exactly that set: an R8 presence mask (power-of-two size, one-texel zero border, box-filtered mips) read by a procedural SDF fragment shader in an `ExtendedMaterial<StandardMaterial, HoleGridExt>` written in WESL. |
| D5 | The hole shader is inserted **synchronously** into `Assets<Shader>` under a `uuid_handle!` constant in `Plugin::build`. Nothing loads asynchronously, so on-demand rendering never waits on an asset. A `SettleFrames(n)` resource bounds the extra frames after spawns. |
| D6 | Platform and room edits are **whole-value commands** (`UpdatePlatform`, `SetRoom`). The inverse is the old value. `PlatformAspects` (a diff) lets placement-only edits just re-transform. |
| D7 | Outlines are stored in **the order the user typed the corners**. All geometry code accepts either orientation, so corner and wall numbers never renumber themselves. |
| D8 | The room outline is the **inner face** of the walls, and walls extrude outward. Openings (doors and windows) are `(wall, offset, width, height, sill)` with a stable `OpeningId`. **Shape edits refit openings**: a door that no longer fits is moved, or removed if it cannot fit, inside the same undoable command, and a toast offers Undo. Typing an invalid value directly into a door field is rejected with a message. |
| D9 | Selection (`SelectionTarget { Node, Platform, Room }`) is UI state. It is not saved and not undoable. |
| D10 | **Display units (mm/in) and wall display (Auto/Always/Hidden) are per-user preferences** in `settings.json`. Changing them never marks the document edited and never adds an undo step. This deviates from DESIGN §4.2 (`meta.display_units`); T7 updates the doc. |
| D11 | Dirty means `Session::saved_state != Some(History::state_id())`. State ids come from per-entry `state_before`/`state_after` (C-style labels, B-style ids). Undoing back to the save point makes the document clean, and trimming the 500-entry limit stays correct. |
| D12 | **Ids only grow.** Commands that add ids raise `next_id` to `max(next_id, id + 1)`. Undo never lowers it. App code takes new ids with the non-mutating `Document::next_free_id()`, never `allocate_id()` on the live document. Tests that compare after undo use `Document::content_eq` (everything except `next_id`). |
| D13 | **Hole identity.** `LatticeIndex {i, j}` (relative to the grid origin) is the stable identity that M2 attaches parts to. `HoleRef {col, row}` ("C14") is a display label relative to the **candidate lattice range** (outline bounding box shrunk by the margin). Labels do not depend on exclusions or interior cut-outs, so M0 labels and positions stay the same. **Pinning:** when a platform has nodes on it, any outline or grid edit converts `GridOrigin::Auto` to `GridOrigin::At(current origin)` in the same command, so holes never shift under placed parts. |
| D14 | All disk I/O runs on one `IoWorker` thread, and every job runs inside `catch_unwind`. Each save is **verified by decoding its own bytes** before an atomic rename. Modal Win32 file dialogs run on a short-lived STA thread of their own and return through `slint::invoke_from_event_loop`. A borrow is never held across a call that can re-enter. |
| D15 | Autosave: a single-shot 60 s timer, armed only while the document is dirty. One directory per session holds a lock file held with `File::try_lock`. The session ends **only through an explicit `end()`**, never `Drop`, so a panic or crash leaves the autosave recoverable. Recovery works per candidate (Restore / Discard / Later), and an open candidate keeps holding its lock. |
| D16 | Slint lists: **one persistent `Rc<VecModel<T>>` per list property**, created at startup and updated in place with `set_row_data`, push and remove. A model is never replaced, and no struct contains a nested array, so a focused cell survives refreshes. |
| D17 | The edit pipeline rule: **if the committed text equals the field's currently formatted value, it is a no-op** (decided in Slint and again in Rust before parsing). A successful Enter moves focus to the viewport, so the next Ctrl+Z undoes the document. The root `FocusScope.capture-key-pressed` performs document undo/redo whenever `EditState.field-dirty` is false, and otherwise lets the `TextInput` undo its own text. |
| D18 | Walls are **opaque at rest**, use `AlphaMode::Blend` only during a ~250 ms fade, and become `Visibility::Hidden` when faded. The fade target is a pure, tested predicate: the eye is past the wall's outer face, **or** a segment from the eye to the orbit focus or to any platform's top centre crosses the wall below its height. It uses 50 mm hysteresis, and there is no fading in Top view. |
| D19 | In the viewport, each platform is a **root entity**. Its slab, top and legs, and all node entities on it, are `ChildOf(root)`. Moving or re-heighting a platform is one transform write. |
| D20 | No Slint `MenuBar` (on Windows it is a light native menu) and no std-widgets inside the inspector (their style clashes with the dark theme). The inspector scrolls in a plain `Flickable`. |
| D21 | `[profile.dev.package.lab-core]` and `[profile.dev.package.lab-geom]` get `opt-level = 2`, so hole and mesh computation stay fast in debug builds. |
| D22 | **Slint choice controls are controlled components.** `SegmentedControl.current` is an `in` property, and a click only emits `selected(i)`. Every choice property on AppWindow is driven by Rust, so undo, a refused choice or a selection change always moves the thumb (§6.1, §6.3). |

### 0.2 Judge fatal flaws and how this spec fixes them

| Flaw | Fix in this spec |
|---|---|
| B: shader loaded through `embedded_asset!`/`ShaderRef::Path` (an async load) | D5: a synchronous `Assets<Shader>::insert(HOLE_GRID_SHADER.id(), Shader::from_wesl(..))` (§5.5) |
| B: tests demand exact `Document` equality after undo, while ids are allocated outside commands | D12: `next_free_id()`, commands raise `next_id`, `content_eq` in tests (§3.2, §3.14) |
| B: hole labels relative to the tight bounds of existing holes; `LatticeIndex` claimed stable under `Centered` | D13: labels relative to the candidate range; the pinning rule; `Auto` documented as unstable unless pinned (§3.4, §7.3) |
| B: Enter does not blur, and Ctrl+Z is rejected while a field has focus | D17: blur on a successful Enter, and capture gated on `field-dirty`, not on focus (§6.4) |
| A: task gates require `cargo test --workspace` while files are owned by others | Per-crate gates; broken crates are listed explicitly per phase (§8.2) |
| A/C: wall fade is a half-plane test only (fails in L rooms) | D18 segment-occlusion predicate (§5.6) |
| A/C: Enter on unchanged inch text changes geometry | D17 no-op rule, enforced in Slint and in Rust (§6.4, §7.3) |
| A: table models rebuilt, so focus is lost | D16 persistent models (§6.2) |
| A: recovery discards every session; Discard deletes all | Per-candidate recovery (§4.5, §7.5) |
| A: walls correct only at 90° corners | Mitred walls in `lab-geom` (§2.6) |
| A: display units undoable and stored in the document | D10 |
| C: `Drop` deletes the autosave on panic | D15: explicit `end()`, no `Drop` impl |
| C: walls permanently `AlphaMode::Blend` | D18 |
| C: `state_id` = top entry id (wrong after trimming or coalescing) | D11 per-entry before/after ids |

---

## 1. Ground rules for every task

1. **Units and frames.** The document uses mm, radians and +Z up. A node's placement is relative to its platform frame, whose origin is platform-local `(0,0)` on the top surface. Presets and parametric outlines centre the outline's bounding box on that origin. Custom polygons are typed in that frame, and **editing an outline never moves the frame**. Switching a custom polygon back to a parametric kind recentres the shape on the origin, and that is documented in the UI hint. Render space is metres, +Y up. Convert only through `scene::to_render`, `scene::rotation_to_render`, and the new `normal_to_render`/`from_render` (§5.2).
2. **Errors.** Every fallible public function returns `Result<_, TypedError>`. An error's `Display` text is a complete user-facing sentence with **1-based** corner, edge, wall and zone numbers. No `unwrap`/`expect` on user data or file contents. Internal invariants may use `expect("INVARIANT: …")`.
3. **Atomicity.** `Command::apply` validates first, then mutates. On `Err` the document is bit-identical to before. A save either fully replaces its target or leaves it untouched.
4. **Determinism.** The same input gives the same hole sets, meshes, JSON and zip bytes. Never iterate a `HashMap` into output; use `BTreeMap` or a sorted Vec.
5. **Bevy.** Do not touch the Bevy feature list. Verify every Bevy, Slint, wgpu and windows-sys API against `D:\dev\cargo\registry\src\index.crates.io-1949cf8c6b5b557f\` before use. The APIs this spec names were checked there, except where a passage is marked **verify during implementation**; each such passage also gives a fallback.
6. **Idle.** No polling loops and no repeating timers. Single-shot timers only (autosave, toast, status).
7. **Build env** (Git Bash): `export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH`. Run long builds in the background. Use `cargo … --offline`.

### 1.1 Limits (`lab-core/src/limits.rs`, `pub mod limits`)

| Constant | Value |
|---|---|
| `MAX_COORD` | 100_000.0 mm (absolute value of any coordinate or position) |
| `PLATFORM_MIN_EXTENT` / `PLATFORM_MAX_EXTENT` | 50.0 / 20_000.0 mm (outline bounding box, per axis) |
| `PLATFORM_MIN_AREA` | 2_500.0 mm² |
| `OUTLINE_MAX_CORNERS` | 128 |
| `OUTLINE_MIN_EDGE` | 1.0 mm |
| `FLATTEN_TOLERANCE` | 0.02 mm (maximum arc-chord sagitta) |
| `ARC_MAX_SEGMENTS` / `FLATTEN_MAX_POINTS` | 64 / 2048 |
| `THICKNESS` | 3.0..=1000.0 mm |
| `TOP_HEIGHT_MAX` | 5000.0 mm, and `top_height >= thickness` |
| `PITCH` | 3.0..=500.0 mm per axis, and `pitch >= thread.diameter() + 1.0` |
| `MARGIN_MAX` | 1000.0 mm, and `margin >= thread.diameter() / 2` |
| `MAX_LATTICE_AXIS` | 2046 candidate columns or rows (2048 texture limit including the border) |
| `MAX_LATTICE_POINTS` | 250_000 |
| `MAX_ZONES` | 16 per platform (shader uniform array size) |
| `LEG_SIZE` | 10.0..=500.0 mm, and `inset >= size / 2` |
| `LEG_MAX_SPAN` / `MAX_LEGS` | 2200.0 mm / 12 |
| `ROOM_CORNERS` | 3..=64 |
| `ROOM_MIN_WALL` | 100.0 mm (inner face) |
| `ROOM_ANGLE_DEG` | 20.0..=340.0 (interior angle) |
| `WALL_HEIGHT` / `WALL_THICKNESS` | 500.0..=10_000.0 / 20.0..=1000.0 mm |
| `OPENING_MIN_WIDTH` / `OPENING_MIN_HEIGHT` / `OPENING_MIN_JAMB` / `MAX_OPENINGS` | 300.0 / 300.0 / 50.0 mm / 32 |
| `NAME_MAX_CHARS` | 128 (trimmed, non-empty) |
| `MAX_ID` | `i32::MAX as u64` (ids cross into Slint `int`) |
| `HISTORY_LIMIT` | 500 |
| `EPS_KEEP` | 1e-6 mm (comparisons favour keeping a hole that sits exactly at a limit) |

---

## 2. `lab-geom` (new crate, pure, f64)

`crates/lab-geom/Cargo.toml` (T0) depends only on `glam` and `thiserror`. Modules: `polygon.rs`, `fillet.rs`, `distance.rs`, `triangulate.rs`, `mesh.rs`, `walls.rs`, plus `lib.rs`, which re-exports everything public. Every function is orientation-agnostic unless stated.

### 2.1 `polygon.rs`
```rust
use glam::{DVec2, DVec3};
#[derive(Clone, Copy, Debug)]
pub struct PolygonLimits { pub min_corners: usize, pub max_corners: usize, pub min_edge: f64, pub max_abs_coord: f64, pub min_area: f64 }
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PolygonError {
    #[error("needs at least {min} corners, has {count}")] TooFewCorners { count: usize, min: usize },
    #[error("has {count} corners; the maximum is {max}")] TooManyCorners { count: usize, max: usize },
    #[error("corner {} has an invalid coordinate", .corner + 1)] NonFinite { corner: usize },
    #[error("corner {} is too far from the origin", .corner + 1)] OutOfRange { corner: usize },
    #[error("edge {} is only {length:.2} mm long (minimum {min} mm)", .edge + 1)] EdgeTooShort { edge: usize, length: f64, min: f64 },
    #[error("corner {} folds back on itself", .corner + 1)] FoldBack { corner: usize },
    #[error("edges {} and {} cross", .a + 1, .b + 1)] SelfIntersecting { a: usize, b: usize },
    #[error("encloses no area")] ZeroArea,
    #[error("area {area:.0} mm² is below the minimum {min:.0} mm²")] TooSmallArea { area: f64, min: f64 },
}
pub fn signed_area(points: &[DVec2]) -> f64;                 // shoelace; > 0 for CCW seen from +Z
pub fn bounds(points: &[DVec2]) -> (DVec2, DVec2);           // (min, max); (INF, -INF) for empty
pub fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64;          // (b-a).perp_dot(c-a)
pub fn segments_intersect(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool; // closed segments, incl. collinear overlap
pub fn validate_simple_polygon(points: &[DVec2], limits: &PolygonLimits) -> Result<(), PolygonError>;
pub fn interior_angle(points: &[DVec2], corner: usize) -> f64; // radians in (0, 2π), measured inside, orientation-aware
pub fn outward_normal(points: &[DVec2], edge: usize) -> DVec2; // unit; edge i = p[i] -> p[(i+1)%n]
```
`validate_simple_polygon` checks, in this order:
1. corner count
2. every coordinate finite
3. `|x|,|y| <= max_abs_coord`
4. every edge `>= min_edge`
5. fold-back: consecutive edges anti-parallel, i.e. `|cross| <= 1e-12·|a||b|` and `dot < 0`
6. self-intersection: brute force over every non-adjacent edge pair with `segments_intersect` (n is at most 2048, so this is fine)
7. `|signed_area| == 0` gives `ZeroArea`; `< min_area` gives `TooSmallArea`

Collinear same-direction corners (interior angle exactly π) are **allowed**.

### 2.2 `fillet.rs`
```rust
#[derive(Clone, Copy, Debug, PartialEq)] pub struct FilletCorner { pub point: DVec2, pub radius: f64 }
#[derive(Clone, Debug, PartialEq)]
pub struct FlatPath {
    pub points: Vec<DVec2>,      // flattened closed loop, same orientation as the input
    pub smooth: Vec<bool>,       // true = arc-interior vertex (side walls share its normal)
    pub source_corner: Vec<u32>, // input corner that produced each point
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FilletError {
    #[error("corner {} radius cannot be negative", .corner + 1)] NegativeRadius { corner: usize },
    #[error("corner {} radius is too large; the maximum here is {max_radius:.1} mm", .corner + 1)] TooLarge { corner: usize, max_radius: f64 },
    #[error("{0}")] Polygon(PolygonError),
}
pub fn flatten_fillets(corners: &[FilletCorner], tolerance: f64, max_points: usize, limits: &PolygonLimits) -> Result<FlatPath, FilletError>;
```
Algorithm:
1. Validate the corner points (radii ignored) with `validate_simple_polygon`.
2. For corner `P` with previous point `A` and next point `B`, let `u = (A-P)/|A-P|`, `v = (B-P)/|B-P|` and `θ = acos(clamp(u·v,-1,1))`. If `radius == 0` or `θ >= π - 1e-9`, the corner stays sharp. Otherwise the tangent length is `t = r / tan(θ/2)`, the arc centre is `P + normalize(u+v)·r/sin(θ/2)`, and the arc runs from `P+u·t` to `P+v·t`. The same formula works at reflex corners, where it gives a concave fillet.
3. Fit check per edge: `t_i + t_{i+1} <= |edge| + 1e-9`. If it fails, return `TooLarge` for the corner with the larger `t`, with `max_radius` = the largest radius for that corner that fits given the neighbour's current `t`.
4. Segments per arc: `n = clamp(ceil(arc_angle / (2·acos(1 - tol/r))), 2, ARC_MAX_SEGMENTS)`. If the total exceeds `max_points`, scale every arc's `n` down proportionally (minimum 2). Emit the arc start (`smooth=false`), the interior points (`smooth=true`) and the arc end (`smooth=false`).
5. Re-run the self-intersection check on the flattened loop. A concave fillet can touch another edge; that gives `Polygon(SelfIntersecting)`, with the indices mapped back through `source_corner`.

### 2.3 `distance.rs`
```rust
pub fn point_in_polygon(points: &[DVec2], p: DVec2) -> bool;          // non-zero winding; boundary unspecified
pub fn distance_to_segment(a: DVec2, b: DVec2, p: DVec2) -> f64;
pub fn distance_to_boundary(points: &[DVec2], p: DVec2) -> f64;
pub fn distance_to_region(points: &[DVec2], p: DVec2) -> f64;        // 0 inside, else distance_to_boundary
pub fn horizontal_crossings(points: &[DVec2], y: f64, out: &mut Vec<f64>); // half-open rule (y0 <= y < y1 or y1 <= y < y0), sorted ascending
```

### 2.4 `triangulate.rs`
```rust
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TriangulateError { #[error("degenerate polygon")] Degenerate, #[error("no ear found ({remaining} corners left)")] NoEarFound { remaining: usize } }
pub fn triangulate(points: &[DVec2]) -> Result<Vec<[u32; 3]>, TriangulateError>; // triangles CCW seen from +Z, indices into `points`
```
- Preprocess: drop vertices with `|orient(prev,p,next)| <= 1e-12·scale²`, where `scale` is the bbox diagonal.
- O(n²) ear clipping on a CCW index list. An ear is a convex vertex whose triangle contains no remaining reflex vertex (inclusive test).
- If a full pass finds no ear, clip the convex vertex with the largest triangle area and log nothing (a pure crate). With no convex vertex left, return `NoEarFound`.
- Postconditions (tested): triangle count = preprocessed vertex count − 2, and the area sum equals `|signed_area|` within 1e-9 relative.

### 2.5 `mesh.rs`
```rust
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh { pub positions: Vec<DVec3>, pub normals: Vec<DVec3>, pub uvs: Vec<DVec2>, pub indices: Vec<u32> }
impl TriMesh {
    pub fn append(&mut self, other: &TriMesh);
    pub fn triangle_count(&self) -> usize;
    /// Test helper: every undirected edge (positions quantised to 1e-6 mm) is used by exactly two triangles in opposite directions.
    pub fn is_closed(&self) -> bool;
}
pub struct PlatformMeshes { pub top: TriMesh, pub body: TriMesh }
/// top: face at z_top, normal +Z, uv = local (x, y) in mm. body: side walls + bottom face at z_bottom (normal −Z).
/// Side normals: per-edge outward; averaged at `smooth` vertices; sharp vertices are duplicated per edge.
pub fn extrude(path: &FlatPath, z_bottom: f64, z_top: f64) -> Result<PlatformMeshes, TriangulateError>;
/// Flat polygon at z; up = true gives normal +Z with CCW winding from above. uv = (x, y) * uv_scale.
pub fn flat_polygon(points: &[DVec2], z: f64, up: bool, uv_scale: f64) -> Result<TriMesh, TriangulateError>;
pub fn circle_points(center: DVec2, radius: f64, tolerance: f64, max_segments: usize) -> Vec<DVec2>; // CCW
```
Winding rule: every triangle is CCW seen from outside the solid, so the geometric normal agrees with the vertex normals. `top ∪ body` is closed (`is_closed`).

### 2.6 `walls.rs` (mitred walls with real openings)
```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpeningSpec { pub wall: usize, pub offset: f64, pub width: f64, pub sill: f64, pub height: f64 }
#[derive(Clone, Debug)]
pub struct WallGeometry {
    pub wall: usize,
    pub inner_start: DVec2, pub inner_end: DVec2,   // outline corners wall, wall+1
    pub outer_start: DVec2, pub outer_end: DVec2,   // mitred outer corners
    pub outward_normal: DVec2,
    pub body: TriMesh,                               // no bottom face
    pub footprint: TriMesh,                          // plan quad inner_start, inner_end, outer_end, outer_start at footprint_z, normal +Z
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WallError {
    #[error("{0}")] BadOutline(PolygonError),
    #[error("corner {} is {degrees:.0}°; allowed 20°–340°", .corner + 1)] AngleOutOfRange { corner: usize, degrees: f64 },
    #[error("wall {} is too short for its thickness", .wall + 1)] OuterFaceTooShort { wall: usize },
    #[error("opening {} is on wall {}, which does not exist", .opening + 1, .wall + 1)] UnknownWall { opening: usize, wall: usize },
    #[error("opening {} does not fit on wall {}: it must lie between {min_offset:.0} and {max_end:.0} mm from the wall start", .opening + 1, .wall + 1)] OpeningDoesNotFit { opening: usize, wall: usize, min_offset: f64, max_end: f64 },
    #[error("openings {} and {} overlap", .a + 1, .b + 1)] OpeningsOverlap { a: usize, b: usize },
    #[error("opening {} is taller than the wall", .opening + 1)] OpeningTooTall { opening: usize },
}
/// Usable inner-face interval [min_offset, max_end] on `wall`, already including min_jamb.
pub fn opening_limits(inner: &[DVec2], thickness: f64, wall: usize, min_jamb: f64) -> (f64, f64);
pub fn build_walls(inner: &[DVec2], thickness: f64, height: f64, openings: &[OpeningSpec], min_jamb: f64, footprint_z: f64)
    -> Result<Vec<WallGeometry>, WallError>;
```
Geometry, with `s = sign(signed_area)`:
- For edge `i` from `A=p[i]` to `B=p[i+1]`: `d=(B-A)/|B-A|` and `n_out = s·(d.y,-d.x)`.
- The outer corner `A'` is the intersection of the outward offset lines (offset `thickness`) of edges `i-1` and `i`. The angle limits guarantee a finite intersection.
- In edge coordinates `σ = (X-A)·d`, the outer face spans `[σ(A'), σ(B')]` and must be longer than 10 mm, otherwise `OuterFaceTooShort`.
- `opening_limits`: `min_offset = max(0, σ(A')) + jamb` and `max_end = min(L, σ(B')) - jamb`.
- An opening `[o, o+w]` fits if `o >= min_offset-1e-9`, `o+w <= max_end+1e-9`, `sill >= 0` and `sill+height <= H`. Openings on one wall must not overlap and must be at least `jamb` apart.

Faces per wall:
- **Inner face** (normal `-n_out`): `[0,L]×[0,H]` minus the openings, as strips (full height between openings, above each opening `[sill+h, H]` if > 0, below `[0, sill]` if > 0).
- **Outer face** (`+n_out`): the same over `[σ(A'), σ(B')]`.
- **Top quad** `A,B,B',A'` at `z=H`, normal +Z.
- **Mitre end faces** at both ends.
- **Reveals** per opening: two jambs, the lintel underside (−Z), and the sill top (+Z, only when `sill > 0`).
- No bottom face.

`uv = (σ, z)/1000`, which gives the same vertex layout (position, normal, uv) as Bevy primitives. `body` plus a virtual bottom is closed.

### 2.7 lab-geom tests (`cargo test -p lab-geom`)
- **Area and validation**: `signed_area` for CW and CCW squares. One minimal failing input per `PolygonError` (bow-tie, zero edge, spike, NaN, 1e6 coordinate, three collinear points). A collinear corner is allowed.
- **Fillets**: rounded 1200×900 at r=50 gives four arcs with interior points at r ± tol. `TooLarge` reports the right corner and `max_radius`. A concave fillet at the reflex corner of an L. A fillet touching an edge gives `SelfIntersecting`. The point budget is respected (128 corners, r=1000).
- **Triangulation**: square, L, U, comb (40 teeth), spiral, flattened rounded rect, plus 100 random star-shaped and 100 random orthogonal polygons (in-test xorshift64, seeds 1..=100). Each checks the area sum, the triangle count and that all triangles are CCW. CW input also works.
- **Extrusion**: `extrude` is closed, and its normals agree with the winding.
- **Distances**: `distance_*` and `point_in_polygon` against hand values. `horizontal_crossings` through a vertex and along a horizontal edge.
- **Walls**:
  - A rectangle gives 4 closed bodies.
  - At an L-room reflex corner, `σ(A') > 0`.
  - `opening_limits` gives `jamb` at a 90° corner and `t + jamb` at a 270° corner.
  - A fitting opening keeps the body closed.
  - Overlapping openings give `OpeningsOverlap`.
  - `sill + height == H` emits no lintel strip.
  - A 120° corner produces mitred bodies that do not overlap (the outer corners match).

---

## 3. `lab-core`

### 3.1 Module layout
`lib.rs`, `limits.rs` (new), `units.rs` (extended), `prefs.rs` (new), `model.rs` (changed), `outline.rs` (new), `holes.rs` (new), `room.rs` (new), `validate.rs` (new), `command.rs` (changed), `history.rs` (new; `History` moves here), `selection.rs` (new), `presets.rs` (new), `migrate.rs` (new), `sample.rs` (updated).

`lib.rs` re-exports: `pub use command::{ChangeSet, Command, CommandError, PlatformAspects}; pub use history::History; pub use glam; pub use model::*; pub use outline::*; pub use holes::*; pub use room::*; pub use selection::SelectionTarget; pub use units::{Mm, LengthUnit}; pub use prefs::WallDisplay; pub use validate::{PlatformError, GridError, RoomError}; pub use migrate::{LoadError, LoadReport};`. Update the crate doc comment's frame sentence to the one in §1.1.

The dependencies are the M0 set plus `lab-geom` (added by T0).

### 3.2 Data model (`model.rs`), schema 2
```rust
pub const SCHEMA_VERSION: u32 = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] pub struct NodeId(pub u64);      // unchanged
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] pub struct PlatformId(pub u64);  // unchanged
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] pub struct OpeningId(pub u64);   // new

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document { pub schema_version: u32, pub name: String, pub room: Room, pub platforms: Vec<Platform>, pub nodes: Vec<Node>, next_id: u64 }
impl Document {
    pub fn new(name: impl Into<String>) -> Self;            // Room::default(), no platforms, next_id 1
    /// Mutating allocation. Only for building documents outside History (samples, default_document, tests).
    pub fn allocate_id(&mut self) -> u64;
    /// Non-mutating: the id the next new object should use. App code uses this (plus k for several).
    pub fn next_free_id(&self) -> u64;
    pub(crate) fn bump_next_id(&mut self, used: u64);        // next_id = max(next_id, used + 1)
    pub fn node(&self, id: NodeId) -> Option<&Node>; pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node>; pub fn node_index(&self, id: NodeId) -> Option<usize>;
    pub fn platform(&self, id: PlatformId) -> Option<&Platform>;
    pub fn platform_index(&self, id: PlatformId) -> Option<usize>;
    pub fn nodes_on(&self, id: PlatformId) -> impl Iterator<Item = &Node>;
    pub fn contains_id(&self, raw: u64) -> bool;            // any node, platform or opening
    pub fn max_id(&self) -> u64;                            // 0 if empty
    /// Equality ignoring next_id (which only grows). Use in every undo/redo test.
    pub fn content_eq(&self, other: &Document) -> bool;
    pub fn to_json(&self) -> serde_json::Result<String>;    // pretty, unchanged
    pub fn from_json(json: &str) -> Result<(Document, LoadReport), LoadError>; // §3.11
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] pub enum PlatformKind { OpticalTable, Breadboard, Custom }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum Surface { Stainless, BlackAnodized }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub id: PlatformId,
    pub name: String,
    pub kind: PlatformKind,
    pub surface: Surface,
    pub outline: Outline,
    pub thickness: Mm,
    pub top_height: Mm,          // top surface above the floor
    pub position: DVec2,         // frame origin in room coordinates
    pub rotation_z: f64,
    pub hole_grid: Option<HoleGrid>,
    pub support: Support,        // unchanged enum: Legs { size, inset } | Fixed
}
impl Platform {
    pub fn local_to_room(&self, p: DVec2) -> DVec2;
    pub fn room_to_local(&self, p: DVec2) -> DVec2;
    pub fn outline_path(&self) -> Result<lab_geom::FlatPath, OutlineError>;   // FLATTEN_TOLERANCE, FLATTEN_MAX_POINTS
    pub fn leg_positions(&self) -> Vec<DVec2>;                                // §3.3
    pub fn room_aabb(&self) -> (DVec3, DVec3);                                // room-space box incl. legs down to z = 0 (Legs) or z = top - thickness
    pub fn validate(&self) -> Result<(), PlatformError>;
}
```
`Node`, `Shape` and `Finish` are unchanged. **Removed**: `Platform::size`, `Room::size`, `Room::height`, `HoleGrid::{counts, hole_position, nearest_hole}`. `Thread` moves to `holes.rs` (still re-exported).

### 3.3 Outlines (`outline.rs`)
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outline {
    Rectangle { size: DVec2 },
    RoundedRectangle { size: DVec2, radius: Mm },
    LShape { size: DVec2, notch: DVec2, notch_corner: RectCorner },
    Polygon { corners: Vec<OutlineCorner> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RectCorner { XNegYNeg, XPosYNeg, XPosYPos, XNegYPos }   // UI 0..3 = ↙ ↘ ↗ ↖
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineCorner { pub point: DVec2, #[serde(default)] pub radius: Mm }
impl Outline {
    pub fn corners(&self) -> Vec<OutlineCorner>;   // canonical list below
    pub fn to_polygon(&self) -> Outline;           // Polygon { corners: self.corners() }; identical geometry
    pub fn bounds(&self) -> (DVec2, DVec2);        // of the corner points (fillets never extend it)
    pub fn kind_index(&self) -> u8;                // 0 rect, 1 rounded, 2 L, 3 polygon
    pub fn with_corner_inserted(&self, after: usize) -> Result<Outline, OutlineError>; // Polygon only: midpoint of edge after→after+1 at index after+1, radius 0
    pub fn with_corner_removed(&self, index: usize) -> Result<Outline, OutlineError>;  // Polygon only; refuses at 3 corners
    pub fn validate(&self) -> Result<(), OutlineError>;
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OutlineError {
    #[error("Size must be between {min} and {max} mm")] SizeOutOfRange { min: f64, max: f64 },
    #[error("Corner radius is too large; the maximum here is {max:.1} mm")] RadiusTooLarge { max: f64 },
    #[error("The cut-out must leave at least 50 mm on each side")] BadNotch,
    #[error("Corner {} radius is too large; the maximum here is {max:.1} mm", .corner + 1)] FilletTooLarge { corner: usize, max: f64 },
    #[error("Corner {} radius cannot be negative", .corner + 1)] NegativeRadius { corner: usize },
    #[error("The outline {0}")] Polygon(lab_geom::PolygonError),
    #[error("Only custom outlines have editable corners")] NotAPolygon,
    #[error("An outline needs at least 3 corners")] TooFewCorners,
}
```
Canonical corners. All are CCW for the parametric kinds, rectangles are centred, `hx=size.x/2`, `hy=size.y/2`:
- **Rectangle**: `(-hx,-hy),(hx,-hy),(hx,hy),(-hx,hy)`, radius 0.
- **RoundedRectangle**: the same four corners, each with `radius`. Valid if `0 <= radius <= min(hx,hy)`.
- **LShape**, notch `(a,b)` at `XPosYPos`: `(-hx,-hy),(hx,-hy),(hx,hy-b),(hx-a,hy-b),(hx-a,hy),(-hx,hy)`. The other corners mirror x (`XNegYPos`), y (`XPosYNeg`) or both. If exactly one axis was mirrored, reverse the list. Then rotate the list so the corner with the lexicographically smallest `(y, x)` comes first. Valid if `50 <= a <= size.x-50` and `50 <= b <= size.y-50`.
- **Polygon**: as stored.

Validation for every kind:
1. Each bbox extent lies in `PLATFORM_MIN_EXTENT..=PLATFORM_MAX_EXTENT`.
2. `flatten_fillets(corners, FLATTEN_TOLERANCE, FLATTEN_MAX_POINTS, {3, 128, 1.0, MAX_COORD, 2500})` succeeds.
3. A collinear corner with radius > 0 is treated as sharp.

`leg_positions()`. Pure and tested; it returns `[]` for `Fixed`.
1. **Corner candidates.** For each corner of `corners()` with interior angle ≤ 150°: `P + normalize(u+v)·inset/sin(θ/2)`, which is at distance `inset` from both edges. A rectangle gives `(±(hx-inset), ±(hy-inset))`, as in M0.
2. **Span fill.** For consecutive corner candidates along the CCW outline whose straight distance exceeds `LEG_MAX_SPAN`, insert `ceil(d/LEG_MAX_SPAN) - 1` evenly spaced points between them. Example: a 3000×1500 table with inset 150 gets 6 legs, and a 2400×1200 table gets 4.
3. **Keep test.** Keep a point only if `point_in_polygon(flat, c)` and `distance_to_boundary(flat, c) >= size·0.7072`.
4. **Spacing.** Drop a point closer than `size` to an earlier kept one, and stop at `MAX_LEGS`.
5. **Fallback.** If nothing survives, try the four bbox-corner inset points, then the bbox centre if it is inside, else return no legs.

### 3.4 Holes (`holes.rs`)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Thread { M4, M6, Unc8_32, Unc1_4_20 }       // M0; label(self), diameter(self) unchanged
impl Thread { pub const ALL: [Thread; 4]; pub fn radius(self) -> Mm; pub fn index(self) -> usize; pub fn from_index(i: usize) -> Option<Thread>; }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleGrid {
    pub pitch: DVec2,                 // x, y spacing
    pub thread: Thread,
    pub margin: Mm,                   // minimum distance from a hole centre to the outline boundary
    pub origin: GridOrigin,
    pub exclusions: Vec<ExclusionZone>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GridOrigin { Auto, At(DVec2) }   // Auto: pattern centred on the bbox. At(p): LatticeIndex(0,0) at p (platform-local)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExclusionZone { pub name: String, pub shape: ZoneShape }
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ZoneShape { Rect { center: DVec2, size: DVec2 }, Circle { center: DVec2, radius: Mm } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum GridStandard { Metric, Imperial, Custom }
impl HoleGrid {
    pub fn metric() -> Self;     // pitch (25,25), M6, margin 12.5, Auto, []   ← DEFAULT for new tables
    pub fn imperial() -> Self;   // pitch (25.4,25.4), Unc1_4_20, margin 12.7, Auto, []
    pub fn standard(&self) -> GridStandard;   // Metric/Imperial if pitch/thread/margin equal within 1e-9 and origin Auto (zones ignored)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LatticeIndex { pub i: i32, pub j: i32 }
// HoleRef { col, row } and its Display ("C14", "Z1", "AA1") are unchanged (display label).

#[derive(Clone, Debug, PartialEq)]
pub struct HoleSet {
    pub origin: DVec2,        // position of LatticeIndex(0,0)
    pub pitch: DVec2,
    pub thread_radius: Mm,
    pub first: LatticeIndex,  // index of label (col 0, row 0) = candidate range minimum
    pub cols: u32, pub rows: u32,   // candidate range size (0 if empty)
    bits: Vec<u64>,           // row-major over the candidate range
    count: usize,
}
impl HoleSet {
    pub fn empty() -> Self;
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    pub fn contains(&self, idx: LatticeIndex) -> bool;
    pub fn contains_label(&self, h: HoleRef) -> bool;
    pub fn position(&self, idx: LatticeIndex) -> DVec2;                // lattice position, whether or not the hole exists
    pub fn label(&self, idx: LatticeIndex) -> Option<HoleRef>;         // Some if idx is inside the candidate range
    pub fn index_of(&self, h: HoleRef) -> LatticeIndex;
    pub fn hole_at(&self, h: HoleRef) -> Option<DVec2>;                // position only if the hole exists
    pub fn iter(&self) -> impl Iterator<Item = LatticeIndex> + '_;     // j ascending, then i
    pub fn nearest(&self, p: DVec2) -> Option<LatticeIndex>;           // exact Euclidean; ties: smallest (j, i)
    pub fn summary(&self) -> String;                                   // "1,716 holes · 48 × 36 grid"; "No holes"
}
pub fn lattice_origin(grid: &HoleGrid, bounds: (DVec2, DVec2)) -> DVec2;
pub fn candidate_range(grid: &HoleGrid, bounds: (DVec2, DVec2)) -> (RangeInclusive<i32>, RangeInclusive<i32>);
pub fn compute_holes(platform: &Platform) -> Result<HoleSet, PlatformError>;   // Ok(empty) when hole_grid is None
```
**Lattice origin.**
- `At(p)` gives `p`.
- `Auto`, per axis, with bbox `[lo,hi]`, pitch `p` and margin `c`: `usable = (hi-lo) - 2c`; `n = if usable < -EPS_KEEP {0} else {floor(usable/p + 1e-9) + 1}`; `origin = lo + ((hi-lo) - (n-1)·p)/2` (or `(lo+hi)/2` when `n == 0`).

So under `Auto`, `LatticeIndex(0,0)` is the first candidate. `Auto` is **not** stable under outline edits; the D13 pinning rule (applied in `lab-app` edit.rs) protects placed parts.

**Candidate range.** `i ∈ [ceil((lo + c - o)/p - 1e-9), floor((hi - c - o)/p + 1e-9)]`, and the same for `j`. Validation rejects more than 2046 per axis or more than 250 000 in total. Labels: `HoleRef { col: i - first.i, row: j - first.j }`.

**Existence rule.** A hole at lattice point `q` exists if and only if all of these hold:
1. `q` is inside the flattened outline.
2. `distance_to_boundary(flat, q) >= margin - EPS_KEEP`.
3. For every zone, `dist(q, zone) >= thread_radius - EPS_KEEP`, where `dist` is 0 inside the zone. Rect: `distance_to_region` of its 4-corner polygon. Circle: `max(0, |q-c|-r)`. In words, the whole thread circle lies outside every zone.

**Algorithm** (exact; the oracle must agree): for each candidate row `j`, take the edges whose y-range expanded by `c` contains `y_j`. Compute `horizontal_crossings` from those edges. For candidates inside a crossing interval, compute the minimum distance to those edges, then check the zones. Target: under 20 ms in release and under 200 ms in a debug test for 3600×1500 rounded r=50 at 25 mm.

**Nearest** is brute force over set bits, comparing squared distances.

### 3.5 Room (`room.rs`)
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room { pub outline: Vec<DVec2>, pub wall_height: Mm, pub wall_thickness: Mm, pub openings: Vec<Opening> }
impl Default for Room { /* (-3000,-2500),(3000,-2500),(3000,2500),(-3000,2500); height 3000; thickness 150; no openings */ }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opening { pub id: OpeningId, pub name: String, pub wall: u32, pub offset: Mm, pub width: Mm, pub height: Mm, pub sill: Mm }
#[derive(Clone, Debug, PartialEq)]
pub enum OpeningFix { Moved { id: OpeningId, name: String, wall: u32 }, Removed { id: OpeningId, name: String } }
impl Room {
    pub fn validate(&self) -> Result<(), RoomError>;
    pub fn walls(&self) -> Result<Vec<lab_geom::WallGeometry>, RoomError>;   // build_walls(.., OPENING_MIN_JAMB, footprint_z = 1.0)
    pub fn wall_length(&self, wall: usize) -> Mm;
    pub fn opening_limits(&self, wall: usize) -> (Mm, Mm);                   // lab_geom::opening_limits (single source of truth)
    pub fn wall_compass(&self, wall: usize) -> &'static str;                 // 8-way from the outward normal, +Y = "North": "North","North-east",…
    pub fn bounds_outer(&self) -> (DVec2, DVec2);                            // outline bbox expanded by thickness
    pub fn is_centered_rectangle(&self) -> Option<DVec2>;                    // axis-aligned 4-corner rectangle centred at 0
    // Shape helpers: build the new room, remap openings by wall, then refit_openings; never fail because of an opening.
    pub fn with_corner_inserted(&self, after: usize) -> Result<(Room, Vec<OpeningFix>), RoomError>;
    pub fn with_corner_removed(&self, index: usize) -> Result<(Room, Vec<OpeningFix>), RoomError>;   // refuses below 3
    pub fn with_corner_moved(&self, index: usize, to: DVec2) -> Result<(Room, Vec<OpeningFix>), RoomError>;
    pub fn with_rectangle(&self, size: DVec2) -> Result<(Room, Vec<OpeningFix>), RoomError>;         // keeps wall indices and orientation
    pub fn with_l_shape(&self) -> Result<(Room, Vec<OpeningFix>), RoomError>;                        // from the bbox: notch 1/3 × 1/3 at +X+Y
    pub fn with_dimensions(&self, height: Mm, thickness: Mm) -> Result<(Room, Vec<OpeningFix>), RoomError>;
    /// Clamps each opening into its wall's limits (offset into [min_offset, max_end - width]), pushes overlapping
    /// openings right by order of offset (offset = previous end + jamb), clamps height to wall_height - sill.
    /// An opening that still does not fit, or whose height would fall below 300 mm, is removed. Deterministic.
    pub fn refit_openings(&mut self) -> Vec<OpeningFix>;
}
```
**Wall remapping before refit:**
- **Insert after corner `k`.** An old edge `e != k` maps to the new index of `p[e]`. Edge `k` splits into `k` and `k+1`. An opening on `k` stays on `k` if its centre lies in the first half; otherwise it moves to `k+1` with `offset -= |mid - p_k|`.
- **Remove corner `k`.** An old edge `e ∉ {k-1, k}` maps to the new index of `p[e]`. Openings on either merged edge are re-projected onto the merged edge: `offset_new = (old start point on its inner line - new_start)·new_dir`, and the width is unchanged.
- **Move.** Wall and offset are kept.

After any of these, run `refit_openings()`, then `validate()`.

### 3.6 Validation (`validate.rs`)
```rust
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlatformError {
    #[error("Name cannot be empty")] EmptyName, #[error("Name is longer than 128 characters")] NameTooLong,
    #[error("{0}")] Outline(OutlineError),
    #[error("Thickness must be between 3 and 1000 mm")] Thickness,
    #[error("Top height must be at least the thickness and at most 5000 mm")] TopHeight,
    #[error("Position or rotation is not a valid number or is out of range")] Placement,
    #[error("{0}")] Grid(GridError),
    #[error("Legs: size must be 10–500 mm and the inset at least half the size")] Legs,
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GridError {
    #[error("Pitch must be between 3 and 500 mm and at least {min:.2} mm for this thread")] Pitch { min: f64 },
    #[error("Edge margin must be at least {min:.2} mm (the hole radius) and at most 1000 mm")] Margin { min: f64 },
    #[error("Grid origin is out of range")] Origin,
    #[error("The grid is too dense: {cols} × {rows} positions (limit 2046 per side and 250,000 in total)")] TooDense { cols: u64, rows: u64 },
    #[error("At most 16 exclusion zones")] TooManyZones,
    #[error("Exclusion zone {} is invalid: {reason}", .zone + 1)] Zone { zone: usize, reason: String },
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RoomError {
    #[error("Room outline {0}")] Outline(lab_geom::PolygonError),
    #[error("Room corner {} is too sharp or too reflex ({degrees:.0}°; allowed 20°–340°)", .corner + 1)] Angle { corner: usize, degrees: f64 },
    #[error("Wall height must be 500–10000 mm")] Height, #[error("Wall thickness must be 20–1000 mm")] Thickness,
    #[error("Wall {} is too short for its thickness", .wall + 1)] WallTooShort { wall: usize },
    #[error("{name} is on wall {}, which does not exist", .wall + 1)] UnknownWall { name: String, wall: u32 },
    #[error("{name} does not fit on wall {}: it must lie between {min:.0} and {max:.0} mm from the wall start", .wall + 1)] DoesNotFit { name: String, wall: u32, min: f64, max: f64 },
    #[error("{a} and {b} overlap")] Overlap { a: String, b: String },
    #[error("{name}: width and height must be at least 300 mm, the sill at least 0, and the top at most the wall height")] OpeningSize { name: String },
    #[error("At most 32 openings")] TooManyOpenings,
    #[error("Opening ids must be unique")] DuplicateOpeningId,
    #[error("Corners need at least 3 entries")] TooFewCorners,
}
impl Document {
    pub fn validate(&self) -> Result<(), Vec<String>>;   // fatal problems, human-readable, naming the item ("Platform “Table B”: edges 2 and 5 cross")
    pub(crate) fn repair(&mut self) -> Vec<String>;      // non-fatal fixes, returned as notes
}
```
**Platform rules:**
- The name is trimmed and 1..=128 chars.
- The outline is valid.
- Thickness and top height are in range.
- Position is finite with `|x|,|y| <= MAX_COORD`, and rotation is finite.
- Grid: pitch, margin, origin (finite, `<= MAX_COORD`) and the candidate count are within §1.1 limits. There are at most 16 zones. Each zone has a finite centre, `size > 0` or `radius > 0`, and a name of 1..=128 chars.
- Legs as in §1.1.

**Room rules:**
- `validate_simple_polygon` with `{3, 64, 100.0, MAX_COORD, 1e6}`.
- Angles 20°–340°.
- Height and thickness in range.
- `build_walls` succeeds.
- Openings: at most 32, unique ids, and each fits per `opening_limits`.

**Document fatal issues:** duplicate ids across nodes, platforms and openings; a node referencing a missing platform; any platform or room error; an id above `MAX_ID`.

**Repairs** (non-fatal, reported):
- `next_id <= max_id` → `max_id + 1`
- a node quaternion with `|len-1| > 1e-6` → normalised
- `margin` below the thread radius → raised

### 3.7 Commands and ChangeSet (`command.rs`)
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    AddNode { node: Node, index: Option<usize> },                 // M0 (+ bump_next_id)
    RemoveNode(NodeId),                                           // M0
    SetPlacement { id: NodeId, position: DVec3, rotation: DQuat },// M0
    Rename { id: NodeId, name: String },                          // M0
    AddPlatform { platform: Box<Platform>, index: Option<usize> },
    RemovePlatform(PlatformId),                                   // cascades to its nodes
    RestorePlatform { platform: Box<Platform>, index: usize, nodes: Vec<(usize, Node)> },
    UpdatePlatform { platform: Box<Platform> },
    SetRoom(Box<Room>),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformAspects { pub name: bool, pub kind: bool, pub surface: bool, pub placement: bool, pub geometry: bool, pub holes: bool, pub support: bool }
impl PlatformAspects {
    /// placement = position|rotation_z|top_height; geometry = outline|thickness; holes = hole_grid OR outline changed;
    /// support = support OR geometry changed; surface; name; kind.
    pub fn between(old: &Platform, new: &Platform) -> Self;
    pub fn any(&self) -> bool;
    pub fn needs_rebuild(&self) -> bool;   // geometry | holes | support | surface
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    pub nodes_added: Vec<NodeId>, pub nodes_removed: Vec<NodeId>, pub nodes_updated: Vec<NodeId>,
    pub platforms_added: Vec<PlatformId>, pub platforms_removed: Vec<PlatformId>,
    pub platforms_updated: Vec<(PlatformId, PlatformAspects)>,
    pub room_changed: bool,
}
impl ChangeSet { pub fn is_empty(&self) -> bool; pub fn merge(&mut self, other: ChangeSet); /* union, dedupe by id, aspects OR-ed */ pub fn touches(&self, t: SelectionTarget) -> bool; pub fn single_target(&self) -> Option<SelectionTarget>; }
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error("no node with id {0:?}")] UnknownNode(NodeId),
    #[error("a node with id {0:?} already exists")] DuplicateNode(NodeId),
    #[error("node {0:?} refers to a platform that does not exist")] UnknownPlatform(NodeId),
    #[error("no platform with id {0:?}")] NoSuchPlatform(PlatformId),
    #[error("id {0} is already in use")] DuplicateId(u64),
    #[error("{0}")] InvalidPlatform(PlatformError),
    #[error("{0}")] InvalidRoom(RoomError),
    #[error("internal: bad restore index")] BadIndex,
}
impl Command {
    pub fn apply(self, doc: &mut Document) -> Result<(Command, ChangeSet), CommandError>;  // atomic
    pub fn label(&self, doc: &Document) -> String;   // computed before apply
}
```
`single_target()` returns `Some` when the change set touches exactly one platform (added or updated) or only the room.

| Command | Preconditions (checked before mutation) | Effect | Inverse | ChangeSet | Default label |
|---|---|---|---|---|---|
| AddPlatform | id unused (`contains_id`); id ≤ MAX_ID; `validate()` | insert at `min(index,len)`; `bump_next_id(id)` | `RemovePlatform(id)` | `platforms_added=[id]` | "Add {name}" |
| RemovePlatform | exists | remove it and every node on it; record `(index,node)` ascending | `RestorePlatform{..}` | `platforms_removed=[id]`, `nodes_removed=[..]` | "Delete {name}" |
| RestorePlatform | all ids unused; indices valid when inserted ascending; `validate()` | insert the platform at `index`, then the nodes ascending; bump | `RemovePlatform(id)` | `platforms_added`, `nodes_added` | "Restore {name}" |
| UpdatePlatform | id exists; `validate()` | replace | `UpdatePlatform{old}` | `platforms_updated=[(id, aspects)]`, **empty if new == old** | "Edit {old name}" |
| SetRoom | `validate()`; opening ids unused by nodes and platforms; ids ≤ MAX_ID | replace; bump for max opening id | `SetRoom(old)` | `room_changed = new != old` | "Edit Room" |
| AddNode / RemoveNode / SetPlacement / Rename | as in M0 (AddNode also bumps) | M0 | M0 | the M0 lists renamed `nodes_*` | "Add/Delete/Move/Rename {name}" |

### 3.8 History (`history.rs`)
```rust
pub struct History { undo: Vec<Entry>, redo: Vec<Entry>, limit: usize, current: u64, next_state: u64, open_gesture: Option<String> }
struct Entry { command: Command, label: String, coalesce: Option<String>, state_before: u64, state_after: u64 }
impl Default for History { fn default() -> Self { Self::new(500) } }
impl History {
    pub fn new(limit: usize) -> Self;   // current 0, next_state 1
    pub fn execute(&mut self, doc: &mut Document, command: Command) -> Result<ChangeSet, CommandError>;            // label = command.label(doc)
    pub fn execute_labeled(&mut self, doc: &mut Document, command: Command, label: impl Into<String>) -> Result<ChangeSet, CommandError>;
    /// Reserved for M2 drags (implemented and tested now, unused by the M1 UI).
    pub fn execute_coalescing(&mut self, doc: &mut Document, command: Command, label: impl Into<String>, key: &str) -> Result<ChangeSet, CommandError>;
    pub fn end_gesture(&mut self);
    pub fn undo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>>;
    pub fn redo(&mut self, doc: &mut Document) -> Option<Result<ChangeSet, CommandError>>;
    pub fn can_undo(&self) -> bool; pub fn can_redo(&self) -> bool;
    pub fn undo_label(&self) -> Option<&str>; pub fn redo_label(&self) -> Option<&str>;
    pub fn state_id(&self) -> u64;
    pub fn clear(&mut self);   // drops both stacks, keeps current
}
```
**Rules:**
- **No-op commands.** A successful command with an empty ChangeSet records nothing and leaves `state_id` unchanged.
- **execute.** `after = next_state++`. Push `{inverse, label, None, before: current, after}`, then set `current = after`, clear redo, close any gesture, and drop the oldest entry beyond `limit`.
- **execute_coalescing.** If `open_gesture == Some(key)` and the top entry's `coalesce == Some(key)`, apply the command, **discard its inverse**, and set `top.state_after = next_state++` and `current = top.state_after`. Otherwise push a new entry with `coalesce = Some(key)` and set `open_gesture = Some(key)`. Redo is cleared in both cases.
- **undo.** Pop the entry, apply its command, push the inverse to redo with the same label and ids, set `current = state_before`, and close the gesture. **redo** mirrors this.
- **Failure policy.** If an inverse fails to apply (this should never happen), the document is unchanged because commands are atomic. Clear both stacks, set `current = next_state++`, and return the error. The app shows the sheet "Undo history was reset because of an internal error."

### 3.9 Selection, preferences and units
```rust
// selection.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum SelectionTarget { Node(NodeId), Platform(PlatformId), Room }
impl SelectionTarget { pub fn exists_in(self, doc: &Document) -> bool; }
// prefs.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] pub enum WallDisplay { #[default] Auto, Always, Hidden }
// units.rs (Mm, INCH, inches, format_mm kept)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] pub enum LengthUnit { #[default] Millimetre, Inch }
impl LengthUnit { pub fn suffix(self) -> &'static str; /* "mm" | "in" */ }
pub fn format_length(value: Mm, unit: LengthUnit) -> String;           // number only; mm ≤ 2 decimals, in ≤ 3; trailing zeros trimmed; "-0" → "0"
pub fn format_length_with_unit(value: Mm, unit: LengthUnit) -> String; // "1200 mm", "47.244 in"
pub fn format_count(n: usize) -> String;                               // "1,728"
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("Enter a number")] Empty,
    #[error("Use a dot as the decimal separator")] Comma,
    #[error("“{0}” is not a number")] NotANumber(String),
    #[error("Unknown unit “{0}” (use mm, cm, m, in, ft)")] UnknownUnit(String),
}
/// Accepts an optional sign, digits with an optional '.', optional spaces, and an optional case-insensitive unit:
/// mm | cm | m | in | inch | inches | " | ft | feet | '. A feet value may be followed by an inch value ("1' 6\"", "1 ft 6 in").
/// No suffix means default_unit. No exponents, no thousands separators, no fractions. A ',' with no '.' gives Comma.
pub fn parse_length(text: &str, default_unit: LengthUnit) -> Result<Mm, ParseError>;
pub fn parse_angle_deg(text: &str) -> Result<f64, ParseError>;   // "45", "45°", "-12.5 deg" → radians
pub fn format_angle_deg(radians: f64) -> String;                 // normalised to (-180, 180], ≤ 2 decimals trimmed
pub fn normalize_angle(radians: f64) -> f64;                     // into (-π, π]
```

### 3.10 Presets and naming (`presets.rs`)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum PresetGroup { TableMetric, TableImperial, BreadboardMetric, BreadboardImperial, SteelBreadboardMetric, SteelBreadboardImperial, Custom }
#[derive(Clone, Copy, Debug)]
pub struct Preset { pub id: &'static str, pub group: PresetGroup, pub label: &'static str, pub hint: &'static str,
                    pub kind: PlatformKind, pub surface: Surface, pub size: (f64, f64), pub thickness: Mm,
                    pub grid: GridStandard /* Metric|Imperial */, pub legs: bool }
pub const PRESETS: &[Preset];
pub fn preset(id: &str) -> Option<&'static Preset>;
/// Pure (no id allocation). Name: next_platform_name. Outline::Rectangle centred (custom: Polygon of a 900×600 rectangle).
/// Support: Legs { size: 120, inset: 150 } when preset.legs, else Fixed.
/// Placement: if `on` is a platform (the current selection) and the preset is not a table → same position and rotation,
///   top_height = on.top_height + thickness. Otherwise tables: top_height 800; other presets: top_height = thickness (on the floor).
///   Position: the room bbox centre if no other platform's room-space bbox comes within 100 mm of the new one; otherwise
///   100 mm to the right (+X) of the right-most platform bbox, y = 0.
pub fn platform_from_preset(doc: &Document, preset: &Preset, id: PlatformId, on: Option<PlatformId>) -> Platform;
/// Replaces kind, surface, outline, thickness, grid (keeping existing exclusions) and support; keeps id, name, position, rotation, top_height.
pub fn apply_preset(platform: &Platform, preset: &Preset) -> Platform;
pub fn next_platform_name(doc: &Document, kind: PlatformKind) -> String; // "Table A".."Table Z","Table AA"; "Breadboard 1"…; "Platform 1"…
pub fn default_document() -> Document;  // "Untitled": Room::default() + opening "Door 1" (wall 0, offset 600, 1000 × 2100, sill 0) + "Table A" from table-m-1200x900 at the origin
```
**Preset table.** Labels are `W × D` with W ≥ D and W along +X. The `hint` holds a Thorlabs part number **only where it is listed here**. T7 checks each hint against thorlabs.com before release and blanks any it cannot confirm (the size row stays).

| id | group | size (mm) | thick | grid | surface | legs | hint |
|---|---|---|---|---|---|---|---|
| table-m-1200x900, -1500x1200, -1800x1200 | TableMetric | as named | 210 | Metric | Stainless | yes | "Nexus-class optical table" |
| table-m-2400x1200, -2400x1500, -3000x1500, -3600x1500 | TableMetric | as named | 310 | Metric | Stainless | yes | same |
| table-i-48x36, -60x48, -72x48 | TableImperial | in × 25.4 | 210 | Imperial | Stainless | yes | same |
| table-i-96x48, -96x60, -120x60, -144x60 | TableImperial | in × 25.4 | 310 | Imperial | Stainless | yes | same |
| bb-m-150x150 / 300x300 / 450x300 / 450x450 / 600x450 / 600x600 / 900x600 | BreadboardMetric | as named | 12.7 | Metric | BlackAnodized | no | "Thorlabs MB1515/M" / "MB3030/M" / "MB3045/M" / "MB4545/M" / "MB4560/M" / "MB6060/M" / "MB6090/M" |
| bb-i-6x6 / 12x6 / 12x12 / 18x12 / 24x18 / 24x24 / 36x24 | BreadboardImperial | in × 25.4 | 12.7 | Imperial | BlackAnodized | no | "Thorlabs MB6" / "MB612" / "MB12" / "MB1218" / "MB1824" / "MB2424" / "MB2436" |
| bbs-m-600x450 / 900x600 / 1200x900 | SteelBreadboardMetric | as named | 60 | Metric | Stainless | no | "Steel honeycomb breadboard" |
| bbs-i-24x18 / 36x24 / 48x36 | SteelBreadboardImperial | in × 25.4 | 61 | Imperial | Stainless | no | "Steel honeycomb breadboard" |
| custom | Custom | Polygon 900×600 rect | 60 | Metric | Stainless | no (Fixed) | "Edit the corners in the inspector" |

### 3.11 Migration and loading (`migrate.rs`)
```rust
#[derive(Debug, Clone, Default, PartialEq)] pub struct LoadReport { pub migrated_from: Option<u32>, pub repairs: Vec<String> }
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a LabSetup project ({0})")] NotAProject(String),
    #[error("saved by a newer LabSetup (format {found}); this version reads up to format {supported}")] NewerSchema { found: u32, supported: u32 },
    #[error("the project file is damaged: {0}")] Json(#[from] serde_json::Error),
    #[error("could not convert from format {from}: {detail}")] Migration { from: u32, detail: String },
    #[error("the project contains invalid data: {}", .0.join("; "))] Invalid(Vec<String>),
}
type Step = fn(&mut serde_json::Value, &mut Vec<String>) -> Result<(), String>;
const MIGRATIONS: &[(u32, Step)] = &[(1, v1_to_v2)];
pub fn migrate(value: &mut serde_json::Value, notes: &mut Vec<String>) -> Result<Option<u32>, LoadError>;
```
`Document::from_json` runs these steps in order:
1. Parse a `Value`.
2. Check `schema_version`: missing, not an integer, or 0 gives `NotAProject`; greater than 2 gives `NewerSchema`.
3. Apply the migration steps; each sets `schema_version = from + 1`.
4. `from_value::<Document>`.
5. `repair()`; its notes go into the report.
6. `validate()`; failures give `Invalid`.

**`v1_to_v2`** (exact):
- **room.** `{size:[w,h], height}` becomes `{outline:[[-w/2,-h/2],[w/2,-h/2],[w/2,h/2],[-w/2,h/2]], wall_height: clamp(height,500,10000), wall_thickness: 150.0, openings: []}`. Add a note if the height was clamped.
- **each platform:**
  - `kind`: `"OpticalTable"` if `support` is `{"Legs":…}`, else `"Breadboard"`. `surface` is `"Stainless"` for tables and `"BlackAnodized"` otherwise.
  - `outline = {"Rectangle":{"size":size}}`; remove `size`.
  - `hole_grid {pitch, margin, thread}` becomes `{pitch:[p,p], thread, margin, origin, exclusions:[]}`. `origin` is `"Auto"` if the Auto origin computed by §3.4 equals `(-w/2+margin, -h/2+margin)` within 1e-9 on both axes, else `{"At":[-w/2+margin, -h/2+margin]}`. Either way the M0 hole set and labels are reproduced.
- `nodes`, `name` and `next_id` are untouched.

### 3.12 Sample (`sample.rs`)
- `demo_document()` uses the v2 model: `Room::default()` with no openings (as M0), and "Table A" with `Outline::Rectangle{(1200,900)}`, OpticalTable, Stainless, metric grid, `Legs{120,120}`.
- `Builder::hole(col,row)` = `compute_holes(platform).unwrap().hole_at(HoleRef{col,row}).expect(..)`.
- All node positions are identical to M0 (tested against the fixture).

### 3.13 Fixture
`crates/lab-core/tests/fixtures/demo_v1.json` is produced by **T0 from the untouched M0 code** (§8).

### 3.14 lab-core tests (`cargo test -p lab-core`, deterministic)
**Ported M0 tests** keep identical expected values:
- `metric_grid_counts_and_positions`: 1728 holes; label A1 at (-587.5,-437.5); last at (587.5,437.5); `contains_label(HoleRef{col:48,row:0}) == false`.
- `imperial_grid_fits_whole_inches` (48×24).
- `nearest_hole_snaps_and_clamps`: the same three asserts via `nearest` + `label`.
- `hole_labels`.
- The five command tests, with field renames; they compare with `content_eq`.
- The six sample tests.

**New tests.** Numbers are hand-verified; H4 uses an oracle:
- **H1.** Rounded 1200×900, r=50, margin 12.5, M6: **1716** holes (3 lost per corner).
- **H2.** L 1200×900 with notch (400,300) at XPosYPos: **1536** holes. The hole at x=187.5, exactly 12.5 from the notch edge, is kept.
- **H3.** Circle zone (0,0) r=50 on 1200×900 metric: **1716** holes. (37.5,37.5), at 53.033 from the centre, is kept.
- **H4.** Diamond, U, triangle, a 30°-rotated rectangle polygon, and 64 seeded random simple polygons with random fillets and zones: the set equals a brute-force oracle (point-in-polygon + all-edge distance + all zones). The diamond is symmetric.
- **H5.** `nearest((400,300))` on the H2 L gives the hole at (387.5,137.5) (a tie with (412.5,137.5); the smaller i wins). `nearest((0,0))` with the H3 zone gives (-37.5,-37.5). An empty set gives None.
- **H6.** Pitch below diameter+1 gives `GridError::Pitch`. Margin below the radius gives `Margin`. 20 m × 20 m at 3 mm gives `TooDense`. A 60×60 platform with margin 30 gives an empty set and summary "No holes".
- **H7.** Performance: 3600×1500 rounded r=50 at 25 mm computes in under 200 ms in a debug test.
- **H8.** Labels are independent of exclusions: adding a zone that empties column A leaves the label of every other hole unchanged.
- **O1.** Outlines: one test per `OutlineError` variant. `to_polygon()` gives the same flattened path. Insert and remove at k=0 and k=n-1. L lists for all 4 notch corners have positive area and the correct notch bbox.
- **L1.** Legs: a rectangle gives the M0 4 points (as a set). 3000×1500 with inset 150 gives 6 legs; 2400×1200 gives 4. An L gives 5. A 40-gon gives at most 12, all inside.
- **R1.** Room helpers: insert, remove and move with a door on the split or merged wall, k=0 and k=n-1. A door that no longer fits is Moved, or Removed when impossible, and reported. `refit_openings` is idempotent.
- **C1.** Every command: apply, then apply its inverse, gives `content_eq(before)`. Error cases leave the doc bit-identical. A no-op `UpdatePlatform` gives an empty ChangeSet and no history entry. RemovePlatform followed by undo restores node order exactly.
- **C2.** `PlatformAspects::between` for each field.
- **P1.** Property test: 64 seeds × 60 random steps (AddPlatform from a random preset with `next_free_id`, RemovePlatform, UpdatePlatform with valid and invalid mutations, SetRoom via the room helpers, AddNode/RemoveNode, undo, redo). After every step, `doc.validate().is_ok()`. Equal state ids imply `content_eq` docs (record a `BTreeMap<u64, Document>`). Undoing everything returns to the start state (`content_eq`).
- **S1.** History: labels; the limit (undo all after trimming leaves the doc dirty relative to state 0); a failed inverse clears the stacks and changes state_id; an empty ChangeSet does not change state_id; coalescing: 3 coalesced commands give 1 undo, and each changes state_id; `end_gesture` splits entries.
- **U1.** Units: `parse_length("47.244 in")=1199.9976`, `format_length(1199.9976, Inch)="47.244"`, `"12,5"` → Comma, `"2'"` → 609.6, `"1' 6\""` → 457.2, `"1e3"` → NotANumber, `-0.0` → "0". Round trip `parse(format(x,u),u)` within the display precision for 10k seeded x.
- **M1.** The fixture loads with `migrated_from == Some(1)` and validates.
- **M2.** Every v1 node position is unchanged.
- **M3.** The migrated table's hole positions equal the M0 grid (1728 positions, |Δ| ≤ 1e-9) and its labels are identical.
- **M4.** A newer schema gives `NewerSchema{3,2}`. A missing `schema_version` gives `NotAProject`. Truncated JSON gives `Json` (with a line number). An invalid outline gives `Invalid`.
- **M5.** `to_json(from_json(x))` is a fixed point after one round.
- **PR1.** Every preset builds a valid platform. MB6060/M gives 24×24 holes and MB12 gives 12×12. `platform_from_preset` puts a breadboard on the floor when `on` is None, and on the table when `on` is a table.

---

## 4. `lab-file` (new crate): container, atomic writes, autosave, settings, I/O worker

Dependencies (T0): `lab-core`, `serde`, `serde_json`, `thiserror`, `flate2` (workspace, `"1.1"`, default features). Dev-dependency: `tempfile = "3"` (resolves to the locked 3.27.0).

Verified APIs:
- `flate2::write::DeflateEncoder::new(Vec<u8>, flate2::Compression::default())`
- `flate2::read::DeflateDecoder::new(&[u8])`
- `flate2::Crc` (`new`, `update`, `sum`)
- `std::fs::File::try_lock` (stable; `WouldBlock` for a second handle)
- `std::fs::rename` (MoveFileExW with replace-existing on Windows)

Modules: `zip.rs` (private), `atomic.rs`, `project.rs`, `autosave.rs`, `settings.rs`, `worker.rs`, `lib.rs` (re-exports).

### 4.1 Errors
```rust
#[derive(Debug, thiserror::Error)] #[error("{}: {kind}", .path.display())]
pub struct FileError { pub path: PathBuf, pub kind: FileErrorKind }
#[derive(Debug, thiserror::Error)]
pub enum FileErrorKind {
    #[error("the file or folder does not exist")] NotFound,
    #[error("access was denied (the file may be read-only or open in another program)")] PermissionDenied,
    #[error("the file is locked by another program; try again or use Save As")] Locked,
    #[error("this is not a LabSetup project ({0})")] NotAProject(String),
    #[error("the project file is damaged ({0})")] Corrupt(String),
    #[error("the file is too large ({size} bytes; the limit is {limit})")] TooLarge { size: u64, limit: u64 },
    #[error("it was saved by a newer LabSetup (format {found}); update LabSetup to open it (this version reads format {supported})")] NewerSchema { found: u32, supported: u32 },
    #[error("it contains invalid data: {0}")] Invalid(String),
    #[error("the folder is not empty and is not a LabSetup project folder")] FolderNotEmpty,
    #[error("{0}")] Io(std::io::Error),
    #[error("internal error: {0}")] Internal(String),
}
```
`io::Error` mapping: `NotFound` → NotFound; `PermissionDenied` → PermissionDenied; raw OS errors 32/33 → Locked; anything else → Io. `LoadError` mapping: NotAProject → NotAProject; NewerSchema → NewerSchema; Json and Migration → Corrupt; Invalid → Invalid (joined with "; ").

Limits: `ARCHIVE_MAX_BYTES = 512 MiB` and `PROJECT_JSON_MAX_BYTES = 256 MiB`.

### 4.2 Zip subset (`zip.rs`, `pub(crate)`)
```rust
pub(crate) struct ZipEntryOut<'a> { pub name: &'a str, pub data: &'a [u8] }
pub(crate) fn write_zip(entries: &[ZipEntryOut]) -> Vec<u8>;
pub(crate) fn read_entry(archive: &[u8], name: &str, max_size: usize) -> Result<Vec<u8>, ZipError>;
#[derive(Debug, PartialEq)] pub(crate) enum ZipError { NotZip, Truncated, Unsupported(&'static str), BadCrc, BadDeflate, TooLarge, Missing, Duplicate }
```
**Writer** (deterministic). For each entry, a local header:
- signature `0x04034b50`, version 20, flags `0x0800`
- method 8 if the deflated output is smaller than the input, else 0
- time `0x0000`, date `0x0021`
- CRC (`flate2::Crc`), sizes, name, extra length 0

Then the data. After all entries come the central directory (`0x02014b50`, made-by 20, needed 20, the same fields, comment/disk/attributes 0, local offset) and the EOCD (`0x06054b50`, counts, CD size and offset, comment length 0). No zip64.

**Reader** (strict and bounded):
- Scan backwards for the EOCD from `len-22` to `max(0,len-22-65535)`, requiring `eocd_pos + 22 + comment_len == len`.
- Reject multi-disk archives and any `0xFFFF`/`0xFFFFFFFF` field (zip64) as `Unsupported`.
- Walk the central directory with `checked_add` bounds. Exactly one entry may match `name` (more gives `Duplicate`; none gives `Missing`).
- At the local header, verify the signature. Data starts at `offset + 30 + local name_len + local extra_len`.
- Flag bit 0 (encrypted) gives Unsupported. Methods other than 0 and 8 give Unsupported.
- Sizes come from the **central directory**, so archives with data descriptors (Explorer and 7-Zip output) work. `uncompressed > max_size` gives TooLarge before allocating.
- Method 8: `DeflateDecoder` read through `.take(uncompressed + 1)`; the length must match exactly.
- Verify the CRC. Other entries are ignored.

### 4.3 Atomic write (`atomic.rs`)
```rust
pub fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), FileError>;
```
1. The parent must exist and be a directory, else NotFound. A target that is a directory gives `Invalid("is a folder")`.
2. Temp file `parent/.{file_name}.{pid}.{counter}.tmp`, where `counter` is a process-wide `AtomicU64`. Open it with `create_new(true)`; on AlreadyExists, increment the counter and retry, at most 16 times.
3. `write_all`, `sync_all`, drop.
4. `rename(tmp, target)`. On PermissionDenied or OS error 32/33, retry up to 10 times, sleeping `25 ms × attempt`, then return Locked or PermissionDenied.
5. On any failure after step 2, remove the temp file (best effort). **The target is untouched unless the rename succeeds.**
6. Documented limitations: ACLs and attributes are not carried over; there is no directory fsync on Windows.

### 4.4 Projects (`project.rs`)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum ProjectFormat { Archive, Folder }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct ProjectLocation { pub path: PathBuf, pub format: ProjectFormat } // Archive: the .labsetup file; Folder: the folder
impl ProjectLocation { pub fn display_name(&self) -> String; /* file stem, or folder name */ }
pub struct LoadedProject { pub document: Document, pub location: ProjectLocation, pub report: LoadReport }
#[derive(Clone, Debug, Default)] pub struct SaveOptions { pub backup_schema: Option<u32> }
pub const PROJECT_JSON: &str = "project.json";
pub const PROJECT_EXTENSION: &str = "labsetup";
pub fn encode_archive(doc: &Document) -> Result<Vec<u8>, FileErrorKind>;          // write_zip([project.json = to_json])
pub fn decode_bytes(bytes: &[u8]) -> Result<(Document, LoadReport), FileErrorKind>; // "PK\x03\x04" → zip; '{' after an optional BOM → plain JSON; else NotAProject
pub fn load_project(path: &Path) -> Result<LoadedProject, FileError>;
pub fn save_project(doc: &Document, location: &ProjectLocation, options: &SaveOptions) -> Result<(), FileError>;
```
- **`load_project`.** A directory is a Folder and reads `dir/project.json`. A file named `project.json` is a Folder at its parent. Any other file is an Archive (plain JSON inside a `.labsetup` is accepted). Check sizes **before** reading. A Folder without `project.json` gives NotAProject.
- **`save_project`:**
  1. Serialize in memory.
  2. **Verify by decoding** and check `content_eq` with `doc`, plus the same `next_id`. A mismatch gives `Internal`.
  3. If `backup_schema` is `Some(v)` and `<target>.v{v}.bak` does not exist, copy the existing target there.
  4. Archive: `atomic_write(path)`.
  5. Folder: if the folder does not exist, `create_dir`. If it exists, it must contain `project.json` or be empty, else FolderNotEmpty. Then `atomic_write(folder/project.json)` with the pretty JSON.
- `encode_archive` is byte-identical across runs.

### 4.5 Autosave and crash recovery (`autosave.rs`)
```rust
pub fn default_root() -> PathBuf;   // $LABSETUP_AUTOSAVE_DIR, else %LOCALAPPDATA%\LabSetup\autosave, else temp_dir()/LabSetup/autosave
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo { pub format_version: u32 /* 1 */, pub pid: u32, pub started_at_ms: u64, pub autosaved_at_ms: Option<u64>,
                         pub title: String, pub location: Option<ProjectLocation>, pub schema_version: u32 }
pub struct AutosaveSession { dir: PathBuf, lock: std::fs::File, info: SessionInfo }   // NO Drop impl
impl AutosaveSession {
    pub fn start(root: &Path, title: &str) -> Result<Self, FileError>;   // root/s-{unix_ms}-{pid}/ with "lock" (try_lock held) + session.json
    pub fn dir(&self) -> &Path;
    pub fn write(&mut self, doc: &Document, title: &str, location: Option<&ProjectLocation>) -> Result<(), FileError>; // atomic: autosave.labsetup, then session.json
    pub fn clear(&mut self) -> Result<(), FileError>;                    // delete autosave.labsetup, update session.json
    pub fn end(self) -> Result<(), FileError>;                           // explicit clean exit: unlock, remove the dir
}
pub struct RecoveryCandidate { pub dir: PathBuf, pub info: Option<SessionInfo>, pub autosaved_at: std::time::SystemTime, lock: std::fs::File }
impl RecoveryCandidate {
    pub fn load(&self) -> Result<LoadedProject, FileError>;   // location = info.location (may be None)
    pub fn discard(self) -> Result<(), FileError>;            // delete the dir
    pub fn release(self);                                     // "Later": drop the lock, keep the files
    pub fn newer_format(&self) -> bool;                       // info.schema_version > SCHEMA_VERSION
}
pub fn find_recoverable(root: &Path, own: Option<&Path>) -> Vec<RecoveryCandidate>;   // newest first
```
**Rules:**
- A session dir is **dead** if its `lock` can be `try_lock`ed; a missing lock file also counts as dead. `WouldBlock` means the session is alive, so skip it.
- A candidate holds the lock until it is discarded or released.
- A dead session without `autosave.labsetup` is deleted. Stray `*.tmp` files in dead sessions are deleted.
- An unreadable `session.json` gives `info: None`.
- A newer-format autosave is never deleted automatically.

### 4.6 Settings (`settings.rs`)
```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings { #[serde(default)] pub display_unit: LengthUnit, #[serde(default)] pub wall_display: WallDisplay, #[serde(default)] pub last_dir: Option<PathBuf> }
pub fn settings_path() -> PathBuf;               // $LABSETUP_SETTINGS, else %APPDATA%\LabSetup\settings.json
pub fn load_settings() -> Settings;              // missing → default; corrupt → renamed to settings.json.corrupt, default
pub fn save_settings(s: &Settings) -> Result<(), FileError>;   // atomic_write
```

### 4.7 I/O worker (`worker.rs`)
```rust
pub enum IoJob {
    Save { ticket: u64, doc: Box<Document>, location: ProjectLocation, options: SaveOptions },
    Load { ticket: u64, path: PathBuf },
    Autosave { ticket: u64, doc: Box<Document>, title: String, location: Option<ProjectLocation> },
    ClearAutosave,
    EndSession,
}
pub enum IoResult {
    Saved { ticket: u64, location: ProjectLocation, result: Result<(), FileError> },
    Loaded { ticket: u64, result: Result<LoadedProject, FileError> },
    Autosaved { ticket: u64, result: Result<(), FileError> },
}
pub struct IoWorker { tx: std::sync::mpsc::Sender<IoJob>, handle: Option<std::thread::JoinHandle<()>> }
impl IoWorker {
    pub fn start(autosave: Option<AutosaveSession>, on_result: impl Fn(IoResult) + Send + 'static) -> Self;  // thread "lab-io"
    pub fn submit(&self, job: IoJob);                       // never blocks
    pub fn shutdown(self, timeout: std::time::Duration) -> bool;  // drop tx; poll is_finished every 20 ms (shutdown only)
}
```
- Jobs run FIFO on one thread, inside `std::panic::catch_unwind(AssertUnwindSafe(..))`. A panic becomes `FileErrorKind::Internal` and the worker continues.
- With no session, Autosave returns `Err(Internal("autosave unavailable"))`, and ClearAutosave and EndSession are no-ops.
- `on_result` runs on the worker thread. The app passes a closure that calls `slint::invoke_from_event_loop`.

### 4.8 lab-file tests (`cargo test -p lab-file`)
- **Z1.** `write_zip`/`read_entry` round trip with empty, 1-byte, compressible and incompressible data (store and deflate).
- **Z2.** Identical bytes on repeat.
- **Z3.** Truncation at every 97th byte gives Truncated or NotZip, never a panic.
- **Z4.** A flipped data byte gives BadCrc or BadDeflate.
- **Z5.** A declared 300 MiB entry gives TooLarge without allocating.
- **Z6.** The encrypted flag, an unknown method and a zip64 marker each give Unsupported.
- **Z7.** A hand-built archive with a data descriptor (flag bit 3) reads correctly.
- **Z8** (`#[ignore]`, run once in T7). Our archive extracts with PowerShell `Expand-Archive`, and an archive made with `Compress-Archive` loads.
- **A1.** `atomic_write` creates a file and replaces one.
- **A2.** It replaces a target that is held open by `File::open`.
- **A3.** A read-only target gives PermissionDenied with the original unchanged and no `.tmp` left.
- **A4.** A missing parent gives NotFound.
- **P1.** Save, load, save gives identical archive bytes, and the loaded document is `content_eq`. Run on `demo_document()` and on a document with every outline kind, both origins, both zone kinds and openings.
- **P2.** A Folder save loads from both the folder path and the `project.json` path.
- **P3.** FolderNotEmpty.
- **P4.** A plain-JSON `.labsetup` loads.
- **P5.** A newer schema inside a zip gives NewerSchema.
- **P6.** `.v1.bak` is created once and not overwritten.
- **P7.** The v1 fixture loads through `load_project` with `migrated_from == Some(1)`.
- **S1.** Crash simulation: a child process (`std::process::Command` running this test binary with `--ignored --exact helper_crash_session`) starts a session, writes, and exits with `std::process::exit(3)`. `find_recoverable` finds it and `load` works. A second `find_recoverable` in the same process does not return it while the candidate is held. `discard` removes the dir.
- **S2.** A live session (lock held) is skipped.
- **S3.** A dead session without an autosave is cleaned.
- **S4.** A panic inside a job unwinds without deleting the session dir.
- **W1.** Save then Load run in order. A job that panics gives Internal, and the worker continues.
- **T1.** A corrupt settings file is renamed and defaults are returned.

---

## 5. `lab-viewport`

### 5.1 Public API (`lib.rs`)
```rust
pub use lab_core::{SelectionTarget, WallDisplay};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameTarget { All, Selection, Platform(PlatformId), Room }
pub enum ViewportRequest {
    Input(ViewportInput), SetViewMode(ViewMode),
    Frame(FrameTarget),                                         // replaces FrameAll
    Select(Option<SelectionTarget>),                            // was Option<NodeId>
    LoadDocument(Box<Document>),                                // full rebuild; clears selection and hover
    ApplyChanges { snapshot: Box<Document>, changes: ChangeSet },   // incremental
    SetWallDisplay(WallDisplay),
    Resize { width: u32, height: u32 }, ReleaseTexture(wgpu::Texture),
}
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportEvent {
    Ready { adapter: String, backend: String, quality: Quality, msaa_samples: u32 },   // unchanged
    FrameReady { frames_rendered: u64 },                                              // unchanged
    SelectionChanged(Option<SelectionTarget>),
    HoverChanged(Option<SelectionTarget>),                                            // nodes only in M1
    SceneError(String),                                                               // an item could not be built (skipped)
}
pub(crate) enum FrameRequest { Input(ViewportInput), SetViewMode(ViewMode), Frame(FrameTarget), Select(Option<SelectionTarget>),
                               LoadDocument(Box<Document>), ApplyChanges(Box<Document>, ChangeSet), SetWallDisplay(WallDisplay) }
```
The `start(...)` signature is unchanged. `host.rs` maps the new requests one-to-one into `FrameRequest`.

### 5.2 Files and conversions
```
src/lib.rs            API (T4b)
src/host.rs           routing, SettleFrames, plugin registration (T4b)
src/scene.rs          conversions, lighting, ScenePlugin, SceneIndex, apply_scene_requests, nodes (T4b)
src/sync.rs           pure SyncOp planning (T4b)
src/mesh.rs           TriMesh → Bevy Mesh (T4b)
src/platform.rs       platform root + parts (T4b)
src/hole_material.rs  HoleGridExt, plugin, mask, params, shade_cpu (T4a, wired by T4b)
src/hole_grid.wesl    shader (T4a)
src/room.rs           floor, walls, footprints, fade (T4b stub → T4c)
src/picking.rs        PickId picking + highlight (T4b)
src/camera.rs         FrameTarget framing, getters (T4b)
```
`scene.rs` keeps `MM`, `to_render` and `rotation_to_render`, makes them `pub(crate)`, and adds:
```rust
pub(crate) fn normal_to_render(n: DVec3) -> Vec3 { Vec3::new(n.x as f32, n.z as f32, -n.y as f32) }
pub(crate) fn from_render(v: Vec3) -> DVec3 { DVec3::new(v.x as f64, -v.z as f64, v.y as f64) * 1000.0 }
```
`mesh.rs`:
```rust
/// Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default()) — MAIN_WORLD|RENDER_WORLD, required for CPU ray casting.
/// POSITION = to_render(p), NORMAL = normal_to_render(n), UV_0 = uv as f32 (unconverted mm for platform tops), Indices::U32.
pub(crate) fn trimesh_to_bevy(m: &lab_geom::TriMesh) -> Mesh;
```
The axis change has det = +1, so winding is preserved (tested).

### 5.3 Incremental sync (`sync.rs`, pure)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SyncOp {
    DespawnNode(NodeId), DespawnPlatform(PlatformId) /* root + parts + node children */, DespawnRoom,
    SpawnRoom, BuildPlatform(PlatformId) /* create root if missing, (re)build parts, set root transform */,
    PlacePlatform(PlatformId) /* root transform only */, SpawnNode(NodeId),
}
pub(crate) fn plan_full(doc: &Document) -> Vec<SyncOp>;   // SpawnRoom, BuildPlatform(each), SpawnNode(each whose platform exists)
pub(crate) fn plan_changes(old: &Document, new: &Document, changes: &ChangeSet) -> Vec<SyncOp>;
```
`plan_changes` rules. Output order is all despawns, then SpawnRoom/BuildPlatform, then PlacePlatform, then SpawnNode, deduplicated:
- Every node id in `nodes_added`, `nodes_removed` or `nodes_updated`: `DespawnNode`, plus `SpawnNode` if it exists in `new`.
- `platforms_removed`: `DespawnPlatform` (removes node children). Its nodes appear in `nodes_removed`, and their DespawnNode is harmless.
- `platforms_added`: `BuildPlatform`.
- `platforms_updated` with `needs_rebuild()`: `BuildPlatform`. With only `placement`: `PlacePlatform`. With only name or kind: nothing.
- `room_changed`: `DespawnRoom`, `SpawnRoom`.

The Bevy system `apply_scene_requests` (PreUpdate, replacing M0's `load_requested_document`) walks `PendingRequests` **in order**:
- `LoadDocument`: despawn everything tagged `SceneEntity`, clear `SceneIndex`, run `plan_full`.
- `ApplyChanges`: `plan_changes(index.snapshot, new, changes)`.

Each then stores the snapshot. After a plan with any spawn, set `SettleFrames(2)`. If the selection or hover target no longer exists in the snapshot, set it to None and send `SelectionChanged(None)`.
```rust
#[derive(Resource, Default)]
pub(crate) struct SceneIndex {
    pub snapshot: Option<Document>,
    pub room: Vec<Entity>,
    pub platforms: BTreeMap<PlatformId, PlatformEntities>,
    pub nodes: BTreeMap<NodeId, Entity>,
}
pub(crate) struct PlatformEntities { pub root: Entity, pub parts: Vec<Entity> }   // parts = top, body, legs (children of root)
```
`BuildPlatform` on an existing root despawns only `parts`. Node children survive because they are children of the root, not parts.

### 5.4 Platforms (`platform.rs`)
```rust
#[derive(Component)] pub(crate) struct PlatformRoot(pub PlatformId);
pub(crate) fn platform_root_transform(p: &Platform) -> Transform;   // translation = to_render(position.extend(top_height)), rotation = rotation_to_render(DQuat::from_rotation_z(rotation_z))
pub(crate) fn spawn_platform_parts(commands: &mut Commands, root: Entity, p: &Platform, assets: &mut PlatformAssets) -> Result<Vec<Entity>, String>;
pub(crate) struct PlatformAssets<'a> { pub meshes: &'a mut Assets<Mesh>, pub materials: &'a mut Assets<StandardMaterial>,
                                       pub hole_materials: &'a mut Assets<HoleTopMaterial>, pub images: &'a mut Assets<Image> }
```
Parts, all `ChildOf(root)`, `SceneEntity`, with platform-local transforms:
1. `path = p.outline_path()`, then `lab_geom::extrude(&path, -thickness, 0.0)`. On error, return `Err` and the caller sends `SceneError`.
2. **top**: mesh `top` with the `HoleTopMaterial` (§5.5). Base StandardMaterial:
   - Stainless: srgb(118,122,128), metallic 0.6, roughness 0.45
   - BlackAnodized: srgb(40,42,46), metallic 0.3, roughness 0.5

   Components: `PickId(SelectionTarget::Platform(id))`, `HighlightLook::HoleTop { material: handle.clone(), owner: SelectionTarget::Platform(id) }` (a struct variant, §5.7). The top sits at exactly z = 0 (the body has no top face, so the M0 0.2 mm lift is gone).
3. **body**: mesh `body`.
   - Stainless: srgb(34,36,40), metallic 0.4, roughness 0.6
   - BlackAnodized: srgb(22,23,26), metallic 0.3, roughness 0.55

   Components: `PickId(SelectionTarget::Platform(id))`, `HighlightLook::Standard { material: handle.clone(), base_color, owner: SelectionTarget::Platform(id) }`, where `base_color` is the body colour above.
4. **legs**: `Support::Legs` with `leg_height = top_height - thickness > 0`. A `Cuboid::new(size, leg_height, size)` (render units) at each `leg_positions()` entry, z centre `-thickness - leg_height/2`. Material srgb(28,29,33), roughness 0.7. Not pickable.

**Holes** are computed on the viewport thread: `compute_holes(p)` → `build_mask(cols, rows, |c,r| set.contains_label(HoleRef{col:c,row:r}))` → `hole_params(...)`.

**Nodes** (`scene.rs::spawn_node`): spawned with `ChildOf(root)`, local `translation = to_render(node.position)` and `rotation = rotation_to_render(node.rotation)`, plus `SceneEntity`, `SceneNode(id)`, `HighlightLook::Standard { material, base_color, owner: SelectionTarget::Node(id) }`, and `PickId(SelectionTarget::Node(id))` when `node.pickable`. Materials are the M0 `finish_material`.

### 5.5 Hole rendering (`hole_material.rs` + `hole_grid.wesl`; T4a)
Verified in Bevy 0.20: `bevy::pbr::{ExtendedMaterial{base,extension}, MaterialExtension, MaterialPlugin}`, `MaterialExtension::fragment_shader() -> ShaderRef`, `impl From<Handle<Shader>> for ShaderRef`, `bevy::asset::uuid_handle!`, `Assets::insert(id, asset) -> Result<(), InvalidGenerationError>` (never errors for UUID ids), `Shader::from_wesl(source, "embedded://lab_viewport/hole_grid.wesl")` (module path `lab_viewport::hole_grid`), the derives `Asset, AsBindGroup, TypePath`, and `bevy::render::render_resource::ShaderType`. The extension is not `#[bindless]`, so the combined material is non-bindless and `pbr_input_from_standard_material` handles that with `@if(BINDLESS)`.
```rust
pub(crate) const MAX_ZONES: usize = 16;
pub(crate) const HOLE_GRID_SHADER: Handle<Shader> = bevy::asset::uuid_handle!("6c1f0a4e-3b9d-4f7e-9a51-2d8c7e4b1f03");
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub(crate) struct HoleGridParams {
    pub origin: Vec2,          // platform-local mm of the label (0,0) hole
    pub pitch: Vec2,
    pub mask_size: Vec2,       // padded texture size in texels
    pub hole_radius: f32,      // 0 = no holes
    pub chamfer_radius: f32,   // min(1.35 * hole_radius, 0.5 * min(pitch))
    pub hole_shade: f32,       // 0.92
    pub chamfer_shade: f32,    // 0.30
    pub highlight: f32,        // 0 or 1 (selected)
    pub show_zones: f32,       // 0 or 1 (selected)
    pub accent: Vec4,          // linear #0A84FF
    pub zone_count: u32,
    pub zones: [Vec4; MAX_ZONES],   // rect: (cx, cy, half_w, half_h); circle: (cx, cy, r, -1)
}
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub(crate) struct HoleGridExt {
    #[uniform(100)] pub params: HoleGridParams,
    #[texture(101)] #[sampler(102)] pub mask: Handle<Image>,
}
impl MaterialExtension for HoleGridExt { fn fragment_shader() -> ShaderRef { HOLE_GRID_SHADER.into() } }
pub(crate) type HoleTopMaterial = ExtendedMaterial<StandardMaterial, HoleGridExt>;
pub(crate) struct HoleGridPlugin;
impl Plugin for HoleGridPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut().resource_mut::<Assets<Shader>>()
            .insert(HOLE_GRID_SHADER.id(), Shader::from_wesl(include_str!("hole_grid.wesl"), "embedded://lab_viewport/hole_grid.wesl"))
            .expect("INVARIANT: uuid asset ids never fail to insert");
        app.add_plugins(MaterialPlugin::<HoleTopMaterial>::default());
    }
}
pub(crate) struct HoleMask { pub size: (u32, u32), pub levels: Vec<Vec<u8>> }   // levels[0] = full size
/// Texel (k,l) = label cell (k-1, l-1); 255 present, 0 absent; one-texel zero border; padded to next_power_of_two(cols+2) × next_power_of_two(rows+2);
/// mips: 2×2 box average down to 1×1 (an axis already at 1 stays 1). cols == 0 || rows == 0 → 1×1 [0].
pub(crate) fn build_mask(cols: u32, rows: u32, present: impl Fn(u32, u32) -> bool) -> HoleMask;
/// R8Unorm, all mips concatenated in `data` (M0 hole_tile_image pattern), RenderAssetUsages::RENDER_WORLD,
/// sampler linear min/mag/mip, ClampToEdge.
pub(crate) fn mask_image(mask: &HoleMask) -> Image;
pub(crate) struct GridGeometry { pub origin: Vec2, pub pitch: Vec2, pub hole_radius: f32 }
#[derive(Clone, Copy, Debug)] pub(crate) enum ZoneParam { Rect { center: Vec2, half: Vec2 }, Circle { center: Vec2, radius: f32 } }
pub(crate) fn hole_params(grid: Option<&GridGeometry>, mask: &HoleMask, zones: &[ZoneParam]) -> HoleGridParams;
/// Rust mirror of the shader's `shade` output (mips sampled bilinearly with linear interpolation between levels).
pub(crate) fn shade_cpu(params: &HoleGridParams, mask: &HoleMask, p: Vec2, dpdx: Vec2, dpdy: Vec2) -> f32;
```
`hole_grid.wesl` (exact starting point):
```wesl
import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
};

const PI: f32 = 3.14159265;

struct HoleGridParams {
    origin: vec2<f32>,
    pitch: vec2<f32>,
    mask_size: vec2<f32>,
    hole_radius: f32,
    chamfer_radius: f32,
    hole_shade: f32,
    chamfer_shade: f32,
    highlight: f32,
    show_zones: f32,
    accent: vec4<f32>,
    zone_count: u32,
    zones: array<vec4<f32>, 16>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> grid: HoleGridParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var mask_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var mask_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Derivatives first, in uniform control flow.
    let p = in.uv;                                     // platform-local mm
    let dx = dpdx(p);
    let dy = dpdy(p);
    let px = max(max(length(dx), length(dy)), 1e-4);   // mm per pixel, worst axis
    let g = (p - grid.origin) / grid.pitch;            // label lattice coordinates
    let g_fw = max(length(dx / grid.pitch), length(dy / grid.pitch));

    // Near field: analytic, box-filtered coverage of the nearest lattice hole.
    let cell = floor(g + vec2<f32>(0.5));
    let d = length((g - cell) * grid.pitch);
    let size = vec2<i32>(grid.mask_size);
    let texel = vec2<i32>(cell) + vec2<i32>(1);
    let inside = all(texel >= vec2<i32>(0)) && all(texel < size);
    let present = select(0.0, textureLoad(mask_texture, clamp(texel, vec2<i32>(0), size - vec2<i32>(1)), 0).r, inside);
    let hole_cov = clamp((grid.hole_radius - d) / px + 0.5, 0.0, 1.0);
    let ring_cov = max(clamp((grid.chamfer_radius - d) / px + 0.5, 0.0, 1.0) - hole_cov, 0.0);
    let near = present * (hole_cov * grid.hole_shade + ring_cov * grid.chamfer_shade);
    let near_hole = present * hole_cov;

    // Far field: area-weighted mean × presence averaged over the pixel footprint (mip chain).
    let cell_area = grid.pitch.x * grid.pitch.y;
    let a_hole = PI * grid.hole_radius * grid.hole_radius / cell_area;
    let a_ring = PI * (grid.chamfer_radius * grid.chamfer_radius - grid.hole_radius * grid.hole_radius) / cell_area;
    let uv = (g + vec2<f32>(1.5)) / grid.mask_size;
    let lod = max(log2(max(g_fw, 1e-6)), 0.0);
    let mean = textureSampleLevel(mask_texture, mask_sampler, uv, lod).r;
    let far = mean * (a_hole * grid.hole_shade + a_ring * grid.chamfer_shade);
    let far_hole = mean * a_hole;

    // Near field once a hole radius spans >= 1.6 px, far field below 0.6 px.
    let t = smoothstep(0.6, 1.6, grid.hole_radius / px);
    let shade = mix(far, near, t);
    let hole_amount = mix(far_hole, near_hole, t);

    // Exclusion zones: tint + 1 px outline, only while the platform is selected.
    var tint = 0.0;
    var line = 0.0;
    if (grid.show_zones > 0.0) {
        for (var k = 0u; k < min(grid.zone_count, 16u); k++) {
            let z = grid.zones[k];
            var sd: f32;
            if (z.w < 0.0) {
                sd = length(p - z.xy) - z.z;
            } else {
                let q = abs(p - z.xy) - z.zw;
                sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
            }
            tint = max(tint, 1.0 - smoothstep(-0.5 * px, 0.5 * px, sd));
            line = max(line, 1.0 - smoothstep(0.5 * px, 1.5 * px, abs(sd)));
        }
    }

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var base = pbr_input.material.base_color.rgb * (1.0 - shade);
    base = mix(base, grid.accent.rgb, 0.15 * tint);
    base = mix(base, grid.accent.rgb, 0.85 * line);
    base = mix(base, grid.accent.rgb, 0.18 * grid.highlight);
    pbr_input.material.base_color = vec4<f32>(base, pbr_input.material.base_color.a);
    pbr_input.material.metallic = mix(pbr_input.material.metallic, 0.0, hole_amount);
    pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 1.0, hole_amount);
    pbr_input.material.base_color = alpha_discard(pbr_input.material.flags, pbr_input.material.alpha_cutoff, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
```
How it meets the requirements:

| Requirement | How |
|---|---|
| Respects the outline, margin and zones | The top mesh *is* the outline, so no fragment exists outside it. Holes come only from the mask, which is the lab-core `HoleSet` (outline, margin and zones applied). |
| Crisp at any zoom | The analytic SDF edge uses a box filter one pixel wide (`px` from the derivatives of mm-space UVs). There is no texture magnification. |
| No shimmer or moiré | Below about one pixel per hole radius, the shader blends to an energy-consistent mean. The mip chain averages presence over the pixel footprint (`lod = log2(lattice cells per pixel)`), and the worst-axis footprint is used at grazing angles. Margins and zones remain visible as lighter areas. |
| No brightness pop | The near field integrated over a cell equals the far value (tested with `shade_cpu`). |
| Top view | The same material under the orthographic camera. At whole-room zoom (~6.7 mm/px) the top is an even, slightly darker tone. From ~1 m framing (~0.7 mm/px) the holes are crisp dark discs with a lighter ring. |
| Cost on Intel UHD | About 45 ALU ops, one `textureLoad` and one `textureSampleLevel` per top pixel, on top of PBR shading the top already paid in M0. It replaces M0's 16× anisotropic tiled fetch. The zone loop runs only on the selected platform. There is one extra pipeline (the HoleTop material). |

### 5.6 Room (`room.rs`; stub by T4b, full by T4c)
```rust
pub(crate) struct RoomPlugin;   // init WallFade; Update: update_wall_fade.after(crate::camera::animate_camera)  (valid once animate_camera is pub(crate), §5.8)
#[derive(Resource, Default)] pub(crate) struct WallFade { pub display: WallDisplay, initialized: bool }
#[derive(Component, Clone, Debug)]
pub(crate) struct WallInfo { pub inner_start: DVec2, pub inner_end: DVec2, pub outer_start: DVec2, pub outer_end: DVec2,
                             pub outward: DVec2, pub thickness: f64, pub height: f64 }
/// AssetId<StandardMaterial> is Copy, so Clone + Copy derive is valid.
#[derive(Component, Clone, Copy, Debug)] pub(crate) struct WallState { pub alpha: f32, pub target: f32, pub material: AssetId<StandardMaterial> }
/// Spawns floor, wall bodies and footprints; returns the entities (stored in SceneIndex.room).
pub(crate) fn spawn_room(commands: &mut Commands, room: &Room, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Vec<Entity>;
/// Pure, unit-tested.
pub(crate) fn wall_occludes(wall: &WallInfo, eye: DVec3, targets: &[DVec3], currently_faded: bool) -> bool;
```
- **Floor.** `flat_polygon(outline, 0.0, up, 0.001)` with the M0 floor material, srgb(22,23,26), roughness 0.95, reflectance 0.2. Components: `SceneEntity`, `Backdrop`. Not pickable (a click on it clears the selection).
- **Walls** come from `room.walls()`. Each wall is two entities:
  - **body**: its own StandardMaterial `handle`, srgb(58,60,66), roughness 0.9, metallic 0, `AlphaMode::Opaque`, `NotShadowCaster`. Components: `SceneEntity`, `Backdrop`, `PickId(SelectionTarget::Room)`, `HighlightLook::Standard { material: handle.clone(), base_color, owner: SelectionTarget::Room }`, `WallInfo`, and `WallState { alpha: 1.0, target: 1.0, material: handle.id() }`.
  - **footprint**: srgb(70,73,80) at z = 1 mm, `SceneEntity` and `Backdrop`. Never faded, never pickable; it keeps the plan readable when walls are hidden.
- **Openings** are real holes in the bodies (from lab-geom). There are no door leaves in M1.
- **Occlusion predicate:**
  - `d = (eye.xy - inner_start)·outward`, `threshold = thickness + (if currently_faded {-50} else {+50})`.
  - The wall occludes if `d > threshold`, **or** if any segment eye→target intersects the wall's plan quad (Cyrus–Beck clip against the 4 half-planes) at a height `<= height` within the clipped parameter interval.
  - Targets: the orbit focus (`from_render(OrbitCamera::focus())`) and each platform's `position.extend(top_height)` from `SceneIndex.snapshot`.
- **update_wall_fade:**
  1. `eye = from_render(OrbitCamera::eye())`, or equivalently `from_render(camera Transform.translation)`. The camera is a root entity, so its local `Transform` is its world pose. **Do not use `GlobalTransform`.** Transform propagation runs in `PostUpdate`, so in `Update` after `animate_camera` the `GlobalTransform` still holds the previous frame's pose. After a direct-manipulation drag (no easing, so no further keep_alive frame), the final wall targets would then come from a stale eye and never be corrected.
  2. Target per `WallDisplay`:
     - Always: 1
     - Hidden: 0
     - Auto: 1 in `ViewMode::Top`, else `if wall_occludes(..) {0} else {1}`
  3. On the first run after a spawn, `alpha = target` with no animation. Otherwise alpha += (target−alpha)·(1−exp(−dt·14)), snapping within 0.03. `dt` is the real time since the previous `update_wall_fade` run (measured with `Instant`, not `Time::delta`), capped at 0.25 s; the first step of a fade is capped at 1/60 s. (Board decision (a) on POLA-2, 2026-10-19.)
  4. Write materials (looked up by `WallState.material`) only on change:
     - `alpha == 1`: Opaque, colour alpha 1, `Visibility::Inherited`
     - `0 < alpha < 1`: `AlphaMode::Blend`, colour alpha = alpha, visible
     - `alpha == 0`: `Visibility::Hidden`
  5. While any wall is unsettled, set `RedrawState.keep_alive = true`.
  6. `SetWallDisplay` requests update `WallFade.display`.
- **T4b stub:** `spawn_room` spawns only the floor; `wall_occludes` returns false; `RoomPlugin` does nothing.

### 5.7 Picking and highlight (`picking.rs`)
- `#[derive(Component, Clone, Copy)] pub(crate) struct PickId(pub SelectionTarget);` replaces `PickTarget`.
- `SceneNode(NodeId)` is kept for nodes.
- `Selection { selected: Option<SelectionTarget>, hovered: Option<SelectionTarget> }`.
- Ray-cast filter: an entity with `PickId`, excluding walls with `WallState.alpha < 0.5`. The default `RayCastVisibility::VisibleInView` skips hidden walls.
- A click selects the nearest hit's target, or None. Hover is set only for `Node` targets.
- `HighlightLook` has **struct variants only** (every construction site in §5.4 and §5.6 uses these field names):
  ```rust
  #[derive(Component, Clone)]
  pub(crate) enum HighlightLook {
      Standard { material: Handle<StandardMaterial>, base_color: Color, owner: SelectionTarget },
      HoleTop { material: Handle<HoleTopMaterial>, owner: SelectionTarget },
  }
  ```

Highlight colours:

| Target | Look |
|---|---|
| Node selected | `mix(base, ACCENT, 0.65)` (M0) |
| Node hovered | `mix(base, WHITE, 0.22)` (M0) |
| Platform selected, body | `mix 0.25` toward ACCENT |
| Platform selected, top | `params.highlight = 1`, `params.show_zones = 1` (both 0 otherwise) |
| Room selected (wall bodies) | `mix 0.25` toward ACCENT, preserving the material's current alpha (the fade owns alpha) |

The highlight system runs when `Selection` changed **or** any `Added<HighlightLook>` exists, so respawned entities pick up the current highlight. Material writes happen only when a value changes.

### 5.8 Camera, host, settle frames
- `camera.rs`: add `pub(crate) fn focus(&self) -> Vec3`, `pub(crate) fn eye(&self) -> Vec3` (the current view's eye position in render space, consistent with the `Transform` that `animate_camera` writes this frame) and `pub(crate) fn mode(&self) -> ViewMode` to `OrbitCamera`, and make `animate_camera` `pub(crate)`. Refactor `frame_all` into `fn frame_box(orbit, camera, min: Vec3, max: Vec3)`.
- `FrameRequest::Frame(target)` bounds are data-driven, so they work in the same frame the entities spawn:
  - `Platform(id)`: `Platform::room_aabb()` converted to render space.
  - `Room`: `bounds_outer()` × `[0, wall_height]`.
  - `All`: the union of all platform boxes plus the `Aabb`s of spawned node entities; with no platforms, Room.
  - `Selection`: the matching target; a node uses its `Aabb`.
  - Unknown ids are ignored.
- `host.rs`:
  - `init_resource::<SettleFrames>()` with `#[derive(Resource, Default)] pub(crate) struct SettleFrames(pub u8);`, and a `Last` system: `if settle.0 > 0 { settle.0 -= 1; redraw.keep_alive = true; }`.
  - Add `HoleGridPlugin` and `RoomPlugin` after `DefaultPlugins`, before `ScenePlugin`.
- **Zero idle cost is preserved.** Frames come only from requests, camera easing, wall fades and settle frames, and each is bounded.

### 5.9 lab-viewport tests (no GPU, `cargo test -p lab-viewport`)
- M0 tests kept: `axes_convert_*`, `rotations_convert_*`, the scaler tests and the camera tests.
- `hole_tile_has_full_mip_chain` is **retired together with `hole_tile_image`** and replaced by the mask tests.
- **Mask:** border texels are zero; the size is a power of two; `levels.len() == log2(max)+1`; the last level equals `count/padded_area` within 1/255 per level.
- **shade_cpu energy** (full 20×20 grid): the near field supersampled 64×64 per cell (px = 0.01 mm) averages to the far value within 1%. Over px from 0.05 to 20 mm, the cell-averaged shade changes by less than 3% (no pop).
- **Mesh conversion:** `trimesh_to_bevy` vertex count; a +Z normal maps to +Y; a top triangle stays front-facing.
- **Sync:** a seeded generator (64 seeds × 40 steps, reusing lab-core's command set) applies commands. A fake world of `key → (item clone at build time, placement)` is updated with `plan_changes`. After every step it must equal the fake world built by `plan_full(new)`. A placement-only update emits exactly one `PlacePlatform`.
- **wall_occludes** (T4c): eye outside, eye inside, inside the hysteresis band, the L-room reflex wall (the counterexample from the judges: a 6000×5000 room with a 2000×1667 notch, eye (8000,-2000,3000), target (500,2000,800) → the reflex wall occludes), and a segment passing over the wall top.
- `hole_params`: zone packing (circle `w = -1`), chamfer clamp, and empty grid → `hole_radius == 0`.

---

## 6. `lab-ui` (Slint; T5 owns `crates/lab-ui/**`)

Files:
- `ui/theme.slint`: add `danger: #FF453A`, `danger-soft: #FF453A26`, `warning: #FFD60A`, `field-bg: #FFFFFF0D`, `field-border: #FFFFFF1F`, `inspector-width: 300px`, `radius-field: 6px`.
- `ui/types.slint` (new, a **leaf file** that imports nothing but `theme.slint`, if needed): every exported struct of §6.2.
- `ui/components.slint`: M0 components plus `PillButton` and `IconButton`. `SegmentedControl` becomes a **controlled component** (D22): `in property <int> current`, and the click handler only calls `root.selected(i)`, never assigning `current`. It also gets a compact variant, and `current < 0` hides the thumb. The M0 view-mode use becomes `SegmentedControl { current: root.view-mode; selected(i) => { root.view-mode = i; root.view-mode-requested(i); } }` (view-mode stays `in-out`, and the M0 T/P keys keep assigning it).
- `ui/fields.slint` (new): `EditState`, `TableFocus`, `NumberField`, `TextField`, `LabeledField`, `CornerTable`, `ZoneList`, `DoorList`.
- `ui/inspector.slint` (new): `NodeInspector`, `PlatformInspector`, `RoomInspector`, `PreviewCard`.
- `ui/overlays.slint` (new): `Sheet`, `Toast`, `ProjectMenu`, `PresetMenu`, `PasteSheet`.
- `ui/app.slint`: AppWindow.
- `examples/gallery.rs` (new): AppWindow with mock data. Keys 1–6 switch through the node, platform rect, platform L, platform custom with an invalid row, room with doors, and sheet+toast states.

`src/lib.rs` and `build.rs` are unchanged.

**Import graph rule.** `fields.slint`, `inspector.slint`, `overlays.slint` and `app.slint` import the structs from `types.slint`. `app.slint` re-exports them with `export { SceneRow, LibraryRow, InspectorField, EditField, CornerRow, ZoneRow, DoorRow, WallChoice, OutlinePreview, PreviewLabel, PresetRow } from "types.slint";`. Never define the structs in `app.slint`: `app.slint` imports `fields.slint` and `inspector.slint`, so importing back from `app.slint` fails with "Recursive import" (i-slint-compiler-1.18.1 typeloader.rs). The Rust generator emits every used struct (`doc.used_types.structs_and_enums`), so the re-export is for clarity and stability. **Do not `export` non-Window components** (NumberField and the like) from `app.slint`. That triggers the warning "Exported component … doesn't inherit Window", which breaks T5's no-warnings criterion.

### 6.1 Components (`fields.slint`)
```slint
export global EditState {
    in-out property <bool> field-dirty;    // a focused field holds text != its value
    in-out property <bool> text-focused;   // any text field has focus
    in-out property <int> focus-viewport;  // incremented to ask AppWindow to move focus to the viewport scope
}
// Never binds TextInput.text to `value`; syncs explicitly.
export component NumberField inherits Rectangle {
    in property <string> id;               // field id (§6.3)
    in property <string> value;            // formatted document value
    in property <string> unit;             // "mm", "in", "°", ""
    in property <bool> enabled: true;
    in property <bool> invalid;
    in property <length> field-width: 96px;
    callback committed(string /*id*/, string /*text*/) -> bool;   // true = accepted or no-op
    callback edited(string /*id*/, string /*text*/);              // live (corner/zone cells only use it)
    callback focused(string /*id*/);
    // Contract:
    //  init => { input.text = value; }   changed value => { if (!input.has-focus || input.text == last-synced) { input.text = value; } last-synced = value; }
    //  Enter (accepted): if (input.text == value) { EditState.focus-viewport += 1; return; }    // no-op rule
    //                    if (committed(id, input.text)) { input.text = value; EditState.focus-viewport += 1; }
    //                    else { local-invalid = true; input.select-all(); }   // keep text and focus
    //  focus lost: if (input.text != value && !committed(id, input.text)) { input.text = value; }  local-invalid = false
    //  Escape: input.text = value; local-invalid = false; EditState.focus-viewport += 1
    //  edited: local-invalid = false; root.edited(id, input.text)
    //  changed has-focus: EditState.text-focused = input.has-focus; if focus gained { select-all(); root.focused(id); }
    //  EditState.field-dirty = input.has-focus && input.text != value  (kept current on every edit / focus change / value change)
    //  Right-aligned text, unit label in Theme.text-tertiary after the input; border Theme.danger when invalid || local-invalid.
}
export component TextField   // same contract, left-aligned, no unit (names)
export component LabeledField inherits HorizontalLayout { in property <string> label; /* + NumberField props */ }
```
`CornerTable`:
- Inputs: `in property <[CornerRow]> rows`, `in property <bool> show-radius`, `in property <string> prefix /* "platform" | "room" */`, `in-out property <int> focused-corner: -1`, `in property <int> invalid-row: -1`.
- Header: `#  X  Y  [R]`. Rows are 28 px: an index chip (an 18 px circle; accent when focused, danger when invalid) and NumberFields of 88/88/52 px.
- Hover shows `+` (action `{prefix}.corner.{k}.insert`) and `−` (`{prefix}.corner.{k}.delete`, disabled at 3 rows).
- Footer buttons "+ Add corner" (`{prefix}.corner.append`) and "Paste…" (`{prefix}.corners.paste`).
- Ids: `"{prefix}.corner." + row.index + ".x|y|r"`.
- **Enter in a cell** commits and moves focus to the same column of the next row. Implement it with an exported `global TableFocus { in-out property <string> request; }`: each cell has `property <bool> wanted: TableFocus.request == my-id; changed wanted => { if (wanted) { input.focus(); } }`.
- Tab moves across cells with the default focus navigation.

`ZoneList` and `DoorList` are similar row editors (fields listed in §6.3). Callbacks are bubbled to the root as `action(id)` and `field-committed`.

`PreviewCard`:
- 268×150, radius 10, background `#FFFFFF08`.
- Every `Path` fills the card (268×150) and uses one shared viewbox from `OutlinePreview` (`viewbox-x: vb-x; viewbox-y: vb-y; viewbox-width: vb-size; viewbox-height: vb-size;`) with the default `fit: contain`:
  - outline: fill `#FFFFFF0A`, stroke text-secondary
  - zones: accent stroke
  - error edges: danger stroke
- **Every Path in the card uses the same `stroke-width` (1.5 px).** Error edges are told apart by colour only. Reason, verified in i-slint-core-1.18.1 `items/path.rs` `fitted_path_events` and the skia renderer's `draw_path`: `stroke-width` is a logical length applied after fitting, so it does **not** scale with the viewbox, and no px-per-unit compensation is needed. However, fitting shrinks the fit box by `stroke-width` and offsets the path by `stroke-width/2`, so Paths with different stroke widths would land at slightly different scales and offsets.
- **Label maths.** Take `s = 1.5px` (the shared stroke width). The card is not square, so `contain` maps the square viewbox onto a square of side `S = 150px − s`, centred horizontally in the inset box. Its left edge is `ox = s/2 + ((268px − s) − S)/2` (= `s/2 + 59px`) and its top is `oy = s/2`. A `PreviewLabel` at viewbox coordinates `(x, y)` (y already negated, like the path) is centred at `(ox + (x − vb-x)/vb-size × S, oy + (y − vb-y)/vb-size × S)`. Never use the card width as the scale.
- The label whose `index == focused-corner` is accent-filled and scaled to 120% over 140 ms.

`overlays.slint`:
- **`Sheet`**: modal. It dims the window, swallows all pointer input, and shows a centred 420 px card with a title, a message and 1–3 buttons. There is a `destructive` index. **Keyboard:** the Sheet contains its own `sheet-keys := FocusScope`, which takes focus when the sheet opens (`changed sheet-open => { if (sheet-open) { sheet-keys.focus(); } }` in AppWindow or the Sheet). Its `key-pressed` handles Return (the default button) and Escape (the last button). Focus has to move: the root `shortcuts` rejecting keys in capture does **not** stop delivery to a `TextInput` that still has focus behind the sheet (i-slint-core-1.18.1 window.rs). When the sheet closes, AppWindow does `EditState.focus-viewport += 1` to return focus to the viewport.
- **`Toast`**: a bottom-centre chip above the toolbar with text and an optional action button. Opacity and y animate over 220 ms.
- **`ProjectMenu`**, **`PresetMenu`**: `PopupWindow` menus.
- **`PasteSheet`**: a multi-line `TextInput { single-line: false; }`, a hint "One corner per line: x, y[, r] — or tab-separated, in the current units", and buttons Apply / Cancel.
  - On open, the text area takes focus.
  - In a multi-line TextInput, Enter inserts a newline and never fires `accepted` (items/text.rs: Return → accepted only when single-line). So **Enter does not apply**.
  - The PasteSheet wraps its content in a `paste-keys := FocusScope` whose `capture-key-pressed` handles Ctrl+Return (Apply) and Escape (Cancel), returning `accept`. Everything else is rejected, so typing reaches the text area.
  - The Apply button shows the hint "Ctrl+Enter".
  - **Verify during implementation** (gallery shortcut page, R5) that the capture handler sees Ctrl+Return before the TextInput. Fallback: Apply by button only, and Escape handled by the Cancel path through `sheet-keys`.

### 6.2 Data structures (defined in the leaf file `ui/types.slint`, re-exported from `app.slint`)
```slint
export struct SceneRow { kind: int /*0 node, 1 platform, 2 room*/, id: int, label: string, detail: string, depth: int, selected: bool, is-beam: bool }
export struct LibraryRow { label: string, letter: string, tint: color }                 // M0
export struct InspectorField { label: string, value: string }                           // M0 node fields
export struct EditField { id: string, label: string, value: string, unit: string, enabled: bool, invalid: bool }
export struct CornerRow { index: int, x: string, y: string, r: string, invalid: bool }
export struct ZoneRow { index: int, name: string, kind: int /*0 rect, 1 circle*/, x: string, y: string, w: string, h: string, r: string }
export struct DoorRow { id: int, name: string, wall: int /*0-based*/, wall-label: string, offset: string, width: string, height: string, sill: string }
export struct WallChoice { index: int, label: string }                                   // "Wall 2 · East · 5.00 m"
export struct OutlinePreview { outline: string, zones: string, error-edges: string, vb-x: float, vb-y: float, vb-size: float }
export struct PreviewLabel { index: int, kind: int /*0 corner, 1 wall*/, x: float, y: float, text: string }
export struct PresetRow { id: string, group: string, label: string, hint: string }
```
`LibraryRow` and `InspectorField` move from M0's `app.slint` into `types.slint` unchanged. `OutlinePreview` has no `px-per-unit` field (§6.1: stroke width does not scale with the viewbox).

**Rule (D16):** Rust creates one `Rc<VecModel<_>>` per list property at startup and never replaces it. No struct contains an array.

### 6.3 AppWindow API (`app.slint`): the contract between T5 and T6
```slint
export component AppWindow inherits Window {
    // M0, kept: viewport-texture, viewport-width, viewport-height, library, rendering, status, gpu, in-out view-mode,
    //           selection-title, selection-detail, inspector
    // M0, REMOVED: `in property <bool> has-selection` (replaced by selection-kind != 0) and
    //              `callback frame-all()` (replaced by action("view.frame_all")). M0 main.rs calls ui.set_has_selection and
    //              ui.on_frame_all; T6c must drop both.
    // M0 callbacks kept: viewport-pointer, viewport-scroll, viewport-left, view-mode-requested, clear-selection
    // M0 callback CHANGED: scene-row-clicked(int) -> scene-row-clicked(int /*kind*/, int /*id*/)
    in property <string> window-title;              // title: root.window-title
    in property <string> project-name;              // sidebar header
    in property <string> project-detail;            // "1 table · 37 parts · offline"
    in property <bool> dirty;                       // "Edited" pill in the sidebar header
    in property <string> document-note;             // "", "Recovered — not saved yet", "Converted from an older format", "Saving…", "Opening…"
    in property <[SceneRow]> scene;
    in property <bool> can-undo; in property <bool> can-redo;
    in property <string> undo-label; in property <string> redo-label;   // "Undo Change Width"
    in property <int> display-unit;                 // 0 mm, 1 in (choice "units"; driven by Rust)
    in property <int> wall-display;                 // 0 auto, 1 always, 2 hidden
    in property <int> selection-kind;               // 0 none, 1 node, 2 platform, 3 room
    in property <string> inspector-error;           // banner; "" hides it
    // node
    in property <string> selection-title; in property <string> selection-detail; in property <[InspectorField]> inspector;
    // platform
    in property <string> platform-name; in property <string> platform-subtitle;   // "Optical table · 1200 × 900 mm"
    in property <int> platform-kind;                // 0 table, 1 breadboard, 2 custom
    in property <int> platform-surface;             // 0 stainless, 1 black anodized
    in property <int> platform-outline-kind;        // 0 rect, 1 rounded, 2 L, 3 custom
    in property <[EditField]> platform-outline-fields;   // ids platform.outline.width|depth|radius|notch_width|notch_depth (only those that apply)
    in property <int> platform-notch-corner;        // 0..3
    in property <[CornerRow]> platform-corners; in property <int> platform-corner-invalid;
    in property <OutlinePreview> platform-preview; in property <[PreviewLabel]> platform-preview-labels;
    in property <[EditField]> platform-placement;   // platform.x, platform.y, platform.rotation, platform.top_height, platform.thickness
    in property <int> platform-grid-standard;       // 0 none, 1 metric, 2 imperial, 3 custom
    in property <int> platform-thread;              // 0 M4, 1 M6, 2 8-32, 3 1/4-20
    in property <int> platform-origin-mode;         // 0 auto, 1 fixed
    in property <[EditField]> platform-grid-fields; // platform.grid.pitch_x|pitch_y|margin|origin_x|origin_y
    in property <string> platform-hole-summary;     // "1,716 holes · 48 × 36 grid"
    in property <string> platform-grid-note;        // "Grid fixed because parts are placed on this platform" or ""
    in property <[ZoneRow]> platform-zones;
    in property <int> platform-support;             // 0 legs, 1 none
    in property <[EditField]> platform-support-fields;   // platform.support.leg_size|leg_inset
    in property <int> platform-node-count;
    in property <string> platform-warning;          // "3 parts lie outside the outline" or ""
    in property <[PresetRow]> presets;              // current-unit groups first
    // room
    in property <string> room-subtitle;             // "6.00 × 5.00 m · 30.0 m² · 1 door"
    in property <[EditField]> room-fields;          // room.height, room.thickness (+ room.width, room.depth when rectangular)
    in property <bool> room-is-rectangle;
    in property <[CornerRow]> room-corners; in property <int> room-corner-invalid;
    in property <OutlinePreview> room-preview; in property <[PreviewLabel]> room-preview-labels;   // corner numbers + wall numbers
    in property <[DoorRow]> room-doors; in property <[WallChoice]> room-walls;
    // sheet, toast, busy
    in property <bool> sheet-open; in property <string> sheet-title; in property <string> sheet-message;
    in property <[string]> sheet-buttons; in property <int> sheet-default; in property <int> sheet-destructive;   // -1 none
    in property <string> paste-target;              // "" closed, "platform" | "room" = PasteSheet open
    in property <string> toast; in property <string> toast-action;   // toast-action "" = no button
    in property <bool> busy;                        // file I/O in flight: spinner chip, file actions disabled (editing still works)

    callback field-committed(string /*id*/, string /*text*/) -> bool;
    callback field-edited(string /*id*/, string /*text*/);        // live corner/zone preview, no command
    callback choice-selected(string /*id*/, int /*index*/);
    callback action(string /*id*/);
    callback paste-corners(string /*target*/, string /*text*/) -> string;   // "" ok, else error text (sheet stays open)
    callback sheet-button(int);
    callback toast-action-clicked();
    callback scene-row-clicked(int /*kind*/, int /*id*/);          // signature change from M0
}
```
**Choice properties are controlled (D22).** Every choice property above (`display-unit`, `wall-display`, `platform-kind`, `platform-surface`, `platform-outline-kind`, `platform-notch-corner`, `platform-grid-standard`, `platform-thread`, `platform-origin-mode`, `platform-support`) is an `in` property bound one-way to a controlled `SegmentedControl`. Example: `SegmentedControl { current: root.platform-kind; selected(i) => { root.choice-selected("platform.kind", i); } }`.
- Never `current <=> root.x` with an `in` property: that is a compile error ("Cannot link to an 'in' property").
- Never assign `current` inside the control. A one-way binding plus an assignment compiles silently, but the assignment permanently breaks the binding, so later Rust updates (undo, a refused choice, a selection change) would no longer move the thumb.
- Rust refreshes the property synchronously in the `choice-selected` handler, so a refused choice leaves the thumb where it was.
- The door wall dropdown follows the same rule.

**Field ids:**
- Platform: `platform.name`, `platform.x`, `platform.y`, `platform.rotation` (degrees), `platform.top_height`, `platform.thickness`, `platform.outline.width|depth|radius|notch_width|notch_depth`, `platform.corner.{k}.x|y|r`, `platform.grid.pitch_x|pitch_y|margin|origin_x|origin_y`, `platform.zone.{k}.name|x|y|w|h|r`, `platform.support.leg_size|leg_inset`
- Room: `room.width|depth|height|thickness`, `room.corner.{k}.x|y`, `room.door.{id}.name|offset|width|height|sill`

**Choice ids:** `platform.kind`, `platform.surface`, `platform.outline.kind`, `platform.outline.notch_corner`, `platform.grid.standard`, `platform.grid.thread`, `platform.grid.origin`, `platform.support`, `room.door.{id}.wall`, `units`, `walls`.

**Action ids:**
- File: `file.new`, `file.open`, `file.open_demo`, `file.save`, `file.save_as`, `file.save_folder`
- Edit and view: `edit.undo`, `edit.redo`, `selection.delete`, `view.frame_all`, `view.frame_selection`, `view.top`, `view.3d`
- Platforms: `platform.add.{preset_id}`, `platform.preset.{preset_id}`, `platform.delete`, `platform.corner.{k}.insert|delete`, `platform.corner.append`, `platform.corners.paste`, `platform.zone.add_rect`, `platform.zone.add_circle`, `platform.zone.{k}.delete`
- Room: `room.corner.{k}.insert|delete`, `room.corner.append`, `room.corners.paste`, `room.make_l`, `room.make_rect`, `room.door.add`, `room.window.add`, `room.door.{id}.delete`
- Misc: `inspector.dismiss_error`, `paste.cancel`

### 6.4 Layout and keyboard
- **Left sidebar** (M0 panel):
  - **Header.** The project name plus an "Edited" pill (fades in over 140 ms while `dirty`), with `document-note` under it. Clicking the header opens `ProjectMenu`: New (Ctrl+N), Open… (Ctrl+O), Open Demo, Save (Ctrl+S), Save As… (Ctrl+Shift+S), Save as Folder…, each with its shortcut right-aligned. Items are disabled while `busy`.
  - **SCENE header** gets a `+` IconButton that opens `PresetMenu` (`platform.add.{id}`), grouped by `group`.
  - **Scene list** (M0 `ListView`): a Room row (kind 2), then each platform (kind 1, depth 0) with its node groups nested (depth 1 and 2, using M0's " · " grouping).
- **Inspector** (right, M0 slide-in, width `Theme.inspector-width`, visible when `selection-kind != 0`, replacing M0's `has-selection`):
  - Content is a `Flickable` with `mouse-drag-pan-enabled: false`, `content-height: layout.preferred-height`, and a height capped at the window height minus insets. No std-widgets.
  - The `inspector-error` banner (danger-soft background, ✕ → `inspector.dismiss_error`) sits under the title.
  - **Platform:**
    - Title TextField (`platform.name`) and subtitle.
    - A kind Segmented [Table | Breadboard | Custom].
    - A "Preset…" PillButton that opens PresetMenu (`platform.preset.{id}`).
    - **SHAPE**:
      - Segmented [Rect | Rounded | L | Custom].
      - Per-kind fields.
      - For L, a 4-way notch-corner Segmented [↙ ↘ ↗ ↖].
      - For Custom, the CornerTable with show-radius.
      - Then the PreviewCard.
    - **PLACEMENT**: 5 fields.
    - **HOLES**:
      - Segmented [None | Metric M6·25 | Imperial ¼-20·1" | Custom]. Custom reveals pitch X/Y, the thread Segmented [M4 | M6 | 8-32 | ¼-20], margin, and origin [Auto | Fixed] with X/Y when fixed.
      - The hole summary and grid note.
      - EXCLUSION ZONES list with "+ Rectangle" and "+ Circle".
    - **SUPPORT**: [Legs | None] plus leg fields. Surface [Stainless | Black].
    - `platform-warning` in warning colour.
    - Footer: a destructive "Delete platform" PillButton with the caption "Also removes N parts" when `platform-node-count > 0`.
  - **Room:**
    - Title "Room" and subtitle.
    - **SIZE**: height and thickness, plus width and depth when rectangular, with "Make L-shape" or "Make rectangle".
    - **OUTLINE**: PreviewCard with wall numbers, and a CornerTable without R.
    - **DOORS & WINDOWS**: rows with a name TextField, a wall dropdown (PopupWindow list from `room-walls`), offset, width, height, sill and ✕. Buttons "+ Door" and "+ Window".
    - **VIEW**: "Show walls" Segmented [Auto | Always | Hidden] (choice `walls`, a preference).
  - **Node:** M0 read-only fields, formatted in the current units.
- **Top centre:** `[3D | Top]` (M0, now a controlled SegmentedControl as in §6), then a compact `[mm | in]` Segmented (choice `units`).
- **Toolbar pill:** `↶` and `↷` IconButtons before Select, enabled by `can-undo`/`can-redo`.
  - Their tooltips use Slint's built-in `Tooltip` element (no std-widgets import). Its `text` is `styled-text`, and a plain string does not convert, so write `Tooltip { text: @markdown("\{root.undo-label}"); }` and the same for `redo-label`. A `Tooltip` with a child `Text` also works.
  - Each element may contain at most one `Tooltip`.
  - "Frame all" stays and now calls `root.action("view.frame_all")`.
- **Status chip:** M0, plus a spinner while `busy`.
- **Toast:** shown when `toast != ""`.
- **Sheet / PasteSheet:** rendered last, on top, with their own focus scopes (§6.1).
- **Keyboard.** The Window sets `forward-focus: shortcuts`, and a root `shortcuts := FocusScope` wraps all content. AppWindow declares `property <int> focus-tick: EditState.focus-viewport; changed focus-tick => { keys.focus(); }`, where `keys` is the M0 viewport FocusScope.

  `shortcuts.capture-key-pressed(event)` compares letters case-insensitively. With Ctrl held, winit reports the logical key, so the text is "z" or "Z" (verified in `i-slint-backend-winit-1.18.1/winitwindowadapter.rs`).

  | Condition | Handling |
  |---|---|
  | `sheet-open` or `paste-target != ""` | reject everything; the focused `sheet-keys` / `paste-keys` scope handles the keys (§6.1) |
  | Ctrl+Z, no Shift | if `!EditState.field-dirty` → `action("edit.undo")`, accept; else reject (the TextInput does text undo) |
  | Ctrl+Shift+Z or Ctrl+Y | same gating → `edit.redo` |
  | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S | `keys.focus()` (blurs the field, which commits through the focus-lost path), then `action("file.new"/"file.open"/"file.save"/"file.save_as")`, accept. The app defers file actions by one event-loop turn (§7.4). |
  | Delete or Backspace | if `EditState.text-focused` reject; else `action("selection.delete")`, accept |
  | anything else | reject |

  The M0 viewport `keys` FocusScope keeps its keys, now routed through `action`:
  - **Shift+F → `view.frame_selection`.** Check this first, with `event.modifiers.shift && (event.text == "f" || event.text == "F")`. With Shift held, `event.text` is "F" (winit's logical key is shift-affected), so the M0 test `event.text == "f" || event.text == "F"` would otherwise catch Shift+F as frame all.
  - F (no Shift) → `view.frame_all`.
  - T, P/3 (view-mode assignment as in M0).
  - Escape (clear selection).

### 6.5 Motion
- Inspector content swap: crossfade 180 ms with `cubic-bezier(0.2, 0.9, 0.25, 1.0)`.
- Sheet: backdrop to α 0.45 over 200 ms, card y −20 → 0 over 260 ms.
- Toast and popovers: 180–220 ms.
- Theme `quick` and `smooth` stay `out` (the reduce-motion setting is out of scope).

### 6.6 T5 acceptance
- `cargo build -p lab-ui` and `cargo run -p lab-ui --example gallery` both work, with no Slint warnings (no recursive imports, no exported non-Window components).
- Gallery screenshots (PrintWindow) of all six states at 1440×900 and 960×620.
- `lab-app` does not compile against the new API until T6c; this is expected.

---

## 7. `lab-app`

### 7.1 Targets and modules
`lab-app` gains a **library target** `src/lib.rs` (crate name `lab_app`) holding the pure, unit-testable logic. The binary `src/main.rs` (`[[bin]] labsetup`) uses it. `cargo test -p lab-app --lib` compiles without the binary.

| File | Owner | Content |
|---|---|---|
| `src/lib.rs` | T6a | `pub mod session; pub mod edit; pub mod views;` |
| `src/session.rs` | T6a | `Session` (§7.2) |
| `src/edit.rs` | T6a | pure UI text/choice/action → `EditOutcome` (§7.3) |
| `src/views.rs` | T6a | pure document → view data (§7.4) |
| `src/main.rs` | T6c | startup, backend selection (M0), rendering notifier (M0), close handling, `mod app_state; mod bridge; mod controller; mod ui_models; mod files; mod dialogs;` |
| `src/app_state.rs` | T6c | `AppState`, `Timers` |
| `src/bridge.rs` | T6c | M0 viewport plumbing moved from main.rs: `wake_ui`, `listen_to_viewport`, `show_newest_frame`, input forwarding |
| `src/ui_models.rs` | T6c | `UiModels` (persistent `Rc<VecModel>`s) and `fill_*` copying views data into Slint |
| `src/controller.rs` | T6c | callback wiring, the edit pipeline, undo/redo, selection, toasts, refresh |
| `src/files.rs` | T6c stub → T6b | file flows, autosave timer, recovery |
| `src/dialogs.rs` | T6c stub → T6b (from T6-spike) | Win32 dialogs |

`Cargo.toml` (T0) adds `lab-geom`, `lab-file` and `serde_json` (workspace), plus windows-sys features `"Win32_UI_Controls_Dialogs"` and `"Win32_System_Com"`.

### 7.2 Session (`session.rs`)
```rust
pub struct Session {
    pub doc: Document,
    pub history: History,
    pub location: Option<ProjectLocation>,
    pub saved_state: Option<u64>,      // None: never saved and edited, or recovered, or repaired on load
    pub autosaved_state: Option<u64>,
    pub migrated_from: Option<u32>,    // older schema loaded and not yet saved → SaveOptions.backup_schema
    pub recovered: bool,
    pub selection: Option<SelectionTarget>,
}
impl Session {
    pub fn new_clean(doc: Document, location: Option<ProjectLocation>) -> Self;   // saved_state = Some(0)
    pub fn from_loaded(p: LoadedProject) -> Self;     // saved_state = Some(0) unless report.repairs non-empty (then None); migrated_from from the report
    pub fn from_recovered(p: LoadedProject) -> Self;  // saved_state = None, recovered = true
    pub fn is_dirty(&self) -> bool { self.saved_state != Some(self.history.state_id()) }
    pub fn needs_autosave(&self) -> bool { self.is_dirty() && self.autosaved_state != Some(self.history.state_id()) }
    pub fn execute(&mut self, cmd: Command, label: Option<String>) -> Result<ChangeSet, CommandError>;
    pub fn undo(&mut self) -> Option<Result<ChangeSet, CommandError>>;
    pub fn redo(&mut self) -> Option<Result<ChangeSet, CommandError>>;
    pub fn mark_saved(&mut self, state: u64, location: ProjectLocation);   // saved_state = Some(state), migrated_from = None, recovered = false
    pub fn title(&self) -> String;                       // location display name, else doc.name
    pub fn fix_selection(&mut self) -> bool;             // clears a stale selection; true if changed
}
```

### 7.3 Edit pipeline (`edit.rs`, pure, unit-tested)
```rust
pub struct EditContext<'a> { pub doc: &'a Document, pub selection: Option<SelectionTarget>, pub units: LengthUnit }
#[derive(Debug)]
pub enum EditOutcome {
    Nothing,                                                       // no-op (unchanged text, same value)
    Command { command: Command, label: String, select: Option<SelectionTarget>, fixes: Vec<OpeningFix> },
    Preference(PreferenceChange),                                  // units / walls
    OpenPaste(String /* "platform" | "room" */),
}
#[derive(Debug, Clone, Copy)] pub enum PreferenceChange { Units(LengthUnit), Walls(WallDisplay) }
#[derive(Debug, thiserror::Error)]
pub enum EditError { #[error("{0}")] Parse(ParseError), #[error("{0}")] Invalid(String), #[error("That item no longer exists.")] Stale, #[error("Unknown field {0}")] UnknownField(String) }
/// Formatted current value of a field id in `units` (the source of truth for the no-op rule and for views).
pub fn field_value(doc: &Document, field: &str, units: LengthUnit) -> Option<String>;
pub fn field_edit(cx: &EditContext, field: &str, text: &str) -> Result<EditOutcome, EditError>;
pub fn choice_edit(cx: &EditContext, choice: &str, index: i32) -> Result<EditOutcome, EditError>;
pub fn action_edit(cx: &EditContext, action: &str) -> Result<EditOutcome, EditError>;   // non-file actions only
/// Live preview of a corner/zone cell: the platform or room with that one cell changed, or the validation error and the row to tint.
pub fn preview_edit(cx: &EditContext, field: &str, text: &str) -> Result<PreviewTarget, (EditError, Option<usize>)>;
pub enum PreviewTarget { Platform(Platform), Room(Room) }
pub fn parse_pasted_corners(text: &str, units: LengthUnit, with_radius: bool) -> Result<Vec<OutlineCorner>, String>; // "line 3: …"
pub fn label_for(field_or_choice: &str) -> String;   // "Change Width", "Move Corner 3", "Use Imperial Holes", …
```
**Rules:**
1. **No-op rule (mandatory).** Trim the text. If it equals `field_value(..)`, return `Nothing`. Re-committing "47.244 in" never turns 1200 into 1199.9976.
2. **Parse.** Lengths use `parse_length` with the display unit as default. `.rotation` uses `parse_angle_deg`. Names are trimmed.
3. **Build.** Clone the target, change exactly that field, and run `validate()`. An error becomes `Invalid(err.to_string())`. An index or id that no longer exists gives `Stale`.
4. **Pinning (D13).** When the edited platform has `nodes_on(id) > 0`, its grid origin is `Auto`, and the edit changes the outline or the grid pitch, margin or thread, set `origin = At(lattice_origin(old grid, old bounds))` in the same new platform value.
5. **Room shape edits** (corners, rectangle size, make L/rect, height, thickness) use the `Room::with_*` helpers. Their `OpeningFix`es are returned in `fixes`. Direct door-field edits do not refit: they validate and fail with the message.
6. **Kind switches** (outline kind):
   - to Rounded: radius 25, clamped to `min(hx,hy)`
   - to L: notch `(round(w/3), round(d/3))` at XPosYPos
   - to Custom: `to_polygon()` (exact)
   - Custom, Rounded or L to Rect: bbox size, centred
   - other cross-switches: via the bbox
7. **Grid choices:**
   - None: `hole_grid = None`
   - Metric or Imperial: standard pitch, thread and margin, origin Auto (subject to pinning), **exclusions kept**
   - Custom: keep the current values (from None, start from metric)
   - Thread, origin mode and pitch changes keep everything else
   - Origin Fixed: `At(current lattice origin)`
8. **Additions and deletions:**
   - **Zone add:** Rect 100×100 or Circle r 50 at the bbox centre, named "Zone n".
   - **Door add:** on the longest wall, width 1000, height `min(2100, wall_height-100)`, sill 0, centred in its `opening_limits`, name "Door n", id `doc.next_free_id()`. If it cannot fit, `Invalid("No wall is long enough for a 1 m door")`.
   - **Window add:** width 1200, height 1200, sill 900, name "Window n".
   - **Platform add:** `platform_from_preset(doc, preset, PlatformId(doc.next_free_id()), selected platform)` → `AddPlatform`, with `select: Some(Platform(id))`.
   - **Delete** (`selection.delete` / `platform.delete`): Node → `RemoveNode`; Platform → `RemovePlatform`; Room → `Nothing`.
9. **Labels:** `label_for` maps the last id segment, e.g. "Change Width", "Change Edge Margin", "Move Corner 3", "Add Corner", "Remove Corner 4", "Change Outline to L-shape", "Use Imperial Holes", "Add Door", "Change Door Width", "Apply 600 × 600 mm".

### 7.4 Views (`views.rs`, pure) and refresh
Pure builders return Rust structs with the same field names as the Slint structs (`SceneRowData`, `EditFieldData`, `CornerRowData`, `ZoneRowData`, `DoorRowData`, `WallChoiceData`, `OutlinePreviewData`, `PreviewLabelData`, `PresetRowData`, `PlatformViewData`, `RoomViewData`, `NodeViewData`):
```rust
pub fn scene_rows(doc: &Document, selection: Option<SelectionTarget>) -> Vec<SceneRowData>;   // Room first, platforms, node groups nested
pub fn platform_view(doc: &Document, p: &Platform, holes: &HoleSet, units: LengthUnit) -> PlatformViewData;
pub fn room_view(doc: &Document, units: LengthUnit) -> RoomViewData;
pub fn node_view(doc: &Document, node: &Node, holes: Option<&HoleSet>, units: LengthUnit) -> NodeViewData; // M0 fields; "Hole" = label of nearest within 0.5 mm
/// SVG "M x y L … Z" with y negated, "{:.2}" numbers; zones as extra sub-paths; square viewbox = bbox padded 8% (≥ 10 mm).
pub fn outline_preview(flat: &[DVec2], zones: &[ExclusionZone], error_edges: &[(DVec2, DVec2)]) -> OutlinePreviewData;  // no px-per-unit field
pub fn corner_labels(corners: &[DVec2], vb: &OutlinePreviewData) -> Vec<PreviewLabelData>;          // 1-based numbers at the corners (viewbox coords, y negated)
pub fn wall_labels(room: &Room, vb: &OutlinePreviewData) -> Vec<PreviewLabelData>;                  // wall numbers at midpoints, offset outward by 4% of vb
pub fn wall_choice_label(room: &Room, wall: usize, units: LengthUnit) -> String;                    // "Wall 2 · East · 5.00 m" (in units for inches)
pub fn preset_rows(units: LengthUnit) -> Vec<PresetRowData>;                                         // current-unit groups first
pub fn window_title(name: &str, dirty: bool) -> String;                                             // "{name} — LabSetup" / "{name} — Edited — LabSetup"
pub fn project_detail(doc: &Document) -> String;
pub fn outside_warning(doc: &Document, p: &Platform) -> String;                                     // nodes whose xy is outside the flattened outline
pub fn invalid_row(err: &EditError) -> Option<usize>;                                               // first corner of the first crossing edge, etc.
```
Hole sets are cached in `AppState.hole_cache: BTreeMap<PlatformId, (Platform, Rc<HoleSet>)>` and recomputed when the cached platform differs.

### 7.5 AppState and controller (T6c)
```rust
pub struct Timers { pub autosave: slint::Timer, pub toast: slint::Timer, pub status: slint::Timer }
pub struct AppState {
    pub session: Session,
    pub settings: Settings,
    pub hole_cache: BTreeMap<PlatformId, (Platform, Rc<HoleSet>)>,
    pub requests: Option<async_channel::Sender<ViewportRequest>>,
    pub viewport_size: (u32, u32),
    pub io: Option<IoWorker>,
    pub autosave_available: bool,
    pub next_ticket: u64,
    pub pending_save: Option<PendingSave>,          // { ticket, state: u64, location, then: AfterSave }
    pub pending_action: Option<PendingAction>,      // New | Open | OpenDemo | Close — resumed after the "Save changes?" sheet
    pub sheet: Option<SheetKind>,                   // which sheet is open and what its buttons mean
    pub recovery: Vec<RecoveryCandidate>,           // shown one at a time
    pub busy: bool,
    pub toast_undo: bool,
    pub timers: Timers,
}
pub enum AfterSave { Nothing, Continue(PendingAction), Quit }
```
`controller.rs` public surface (T6b calls these):
```rust
pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>);
pub fn refresh_all(ui: &AppWindow, app: &AppState, models: &UiModels);
pub fn replace_document(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, session: Session); // sends LoadDocument + Frame(All), clears the selection, refreshes
pub fn show_toast(ui: &AppWindow, app: &AppState, text: &str, undo: bool);   // 2.4 s, or 5 s with an action; single-shot
pub fn show_sheet(ui: &AppWindow, app: &mut AppState, kind: SheetKind, title: &str, message: &str, buttons: &[&str], default: i32, destructive: i32);
```
**M0 wiring changes.** `ui.set_has_selection(..)` is replaced by `ui.set_selection_kind(..)` (0 none, 1 node, 2 platform, 3 room). `ui.on_frame_all(..)` is removed: "Frame all" (toolbar and F key) arrives as `action("view.frame_all")` and sends `Frame(FrameTarget::All)`, and `view.frame_selection` sends `Frame(FrameTarget::Selection)`.

**Edit flow** (every `field-committed`, `choice-selected` and non-file `action`):
1. Borrow, call `edit::*`, and drop the borrow.
2. `Err(e)`: set `inspector-error = e.to_string()`, refresh the inspector (including the choice property, so a refused choice keeps the thumb in place), and return `false` (field-committed).
3. `Nothing`: return `true`.
4. `Preference(p)`: update and save the settings (unit or wall display), send `SetWallDisplay` if needed, refresh. Never touch history.
5. `Command{..}`: `session.execute(cmd, Some(label))`.
   - `Ok(changes)`:
     1. Clear `inspector-error`.
     2. If `changes` is non-empty, send `ApplyChanges { snapshot: Box::new(doc.clone()), changes }`.
     3. Apply `select` (send `Select`).
     4. `fixes` non-empty → toast "Moved Door 1 to fit Wall 2" or "Removed Window 1, which no longer fits", with Undo.
     5. A deleted platform → toast "Deleted Table A and 34 parts" with Undo.
     6. Refresh the scene rows, inspector, title, undo state, and the `dirty` and `project-detail` properties.
     7. `files::arm_autosave(app)`.
     8. Return `true`.
   - `Err(CommandError)`: as step 2.

**Undo and redo:**
1. Borrow and `session.undo()`.
2. If the change touches exactly one platform or the room (`single_target`) and that is not the selection, select it.
3. `fix_selection()`.
4. Send `ApplyChanges`.
5. Toast "Undo Change Width" with no action.
6. Refresh and arm the autosave.

A `History` failure shows the sheet "Undo history was reset because of an internal error."

**Live corner preview** (`field-edited` on a corner or zone id): `edit::preview_edit` updates only `*-preview`, `*-preview-labels`, `*-corner-invalid` and `inspector-error`. It never issues a command; the 3D view updates on commit.

**Paste:** `paste-corners(target, text)` → `parse_pasted_corners` → an `UpdatePlatform` or `SetRoom` with the new corners (a platform converts to Custom first). It returns "" or the error text.

**Viewport events:**
- `SelectionChanged(t)`: if `t` is Some and `!t.exists_in(doc)`, send `Select(None)`; otherwise store it, then refresh the inspector (crossfade) and scene rows.
- Scene row click: set the selection locally, send `Select`, refresh.

**Rule:** a callback borrows, mutates, collects follow-ups, drops the borrow, and only then calls into Slint or sends requests. A borrow is never held across `invoke_from_event_loop`, a dialog, or a Slint call that can re-enter.

### 7.6 Files, dialogs, autosave, recovery (T6b: `files.rs`, `dialogs.rs`)
```rust
// dialogs.rs
pub enum DialogKind { Open, SaveArchive { suggested: String }, SaveFolder { suggested: String } }
pub enum DialogResult { Chosen(PathBuf), Cancelled, Failed(String) }
/// Spawns a thread: CoInitializeEx(null, COINIT_APARTMENTTHREADED as u32), builds OPENFILENAMEW (Default::default(), lStructSize),
/// hwndOwner = owner (HWND passed as isize because HWND is !Send), 32,768-u16 buffer, Get{Open,Save}FileNameW,
/// CommDlgExtendedError on failure, then slint::invoke_from_event_loop(move || done(result)). Non-Windows: done(Cancelled).
pub fn show_file_dialog(owner: isize, kind: DialogKind, initial_dir: Option<PathBuf>, done: impl FnOnce(DialogResult) + Send + 'static);
pub fn owner_hwnd(window: &slint::Window) -> isize;   // as in M0 style_title_bar; 0 if unavailable
```
Dialog settings:
- **Open**: filter `"LabSetup projects (*.labsetup; project.json)\0*.labsetup;project.json\0All files\0*.*\0\0"`, flags `OFN_EXPLORER|OFN_FILEMUSTEXIST|OFN_PATHMUSTEXIST|OFN_HIDEREADONLY|OFN_NOCHANGEDIR`.
- **SaveArchive**: filter `"LabSetup project (*.labsetup)\0*.labsetup\0\0"`, `lpstrDefExt = "labsetup"`, adds `OFN_OVERWRITEPROMPT`.
- **SaveFolder**: filter `"Project folder\0*.*\0\0"`, no default extension and no overwrite prompt. Strip a trailing `.labsetup` from the result; the typed name **becomes the folder**, and `save_project` enforces the empty-or-project rule.

While a dialog is open, `busy = true`: file actions are ignored but editing still works.

```rust
// files.rs (T6c creates these signatures as stubs; T6b implements them)
pub fn handle_file_action(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, action: &str); // file.* ids; deferred via slint::Timer::single_shot(Duration::ZERO, ..)
pub fn handle_sheet_button(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, button: i32); // file/recovery/close sheets
pub fn handle_io_result(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>, result: IoResult);
pub fn arm_autosave(app: &Rc<RefCell<AppState>>);   // single-shot 60 s if none is running and needs_autosave()
pub fn on_close_requested(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>) -> slint::CloseRequestResponse;
pub fn startup(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>);   // after the first frame: recovery sheets
pub fn initial_session(args: &[String], settings: &Settings) -> (Session, Option<String /*error to show*/>);
```
**Flows:**
- **Save.** With no location, do Save As. Otherwise:
  1. Submit `IoJob::Save{doc.clone(), location, SaveOptions{backup_schema: migrated_from}}`.
  2. Record `pending_save = {ticket, state: history.state_id(), ..}` and set `busy` with the note "Saving…".
  3. On `Saved Ok`: `session.mark_saved(pending.state, location)`, update `settings.last_dir`. If now clean, submit `ClearAutosave`. Toast "Saved", clear `busy`, then continue with `AfterSave`.
  4. On `Err`: the sheet "Couldn't save “{title}”" with the error and "Your changes are kept in the autosave. Try Save As to choose another location." [OK] [Save As…]. The document stays dirty.
- **Save As / Save as Folder.** Dialog → Save with the new location. Cancelling during a close cancels the close.
- **New / Open / Open Demo** when dirty: the sheet "Save changes to “{title}”?" with [Save] [Don't Save] [Cancel] (default Save, destructive Don't Save).
  - **Open:** dialog → `IoJob::Load`.
    - On `Loaded Ok`: `replace_document(Session::from_loaded)`, then `ClearAutosave`. A migrated file sets the note "Converted from an older format". Repairs show the sheet "This project was repaired" with the list.
    - On `Err`: the sheet "Couldn't open “{name}”" with the error; the current document is untouched.
  - **New:** `default_document()`, clean, no location.
  - **Open Demo:** `demo_document()`, clean, no location.
- **Close** (`window().on_close_requested`):
  - A save in flight: set `then = Quit` and return `KeepWindowShown`.
  - Dirty: the save sheet, return `KeepWindowShown`. Don't Save quits.
  - Otherwise run the quit sequence and return `HideWindow`.
  - **Quit sequence:** submit `EndSession`, `io.shutdown(10 s)`, `save_settings`, drop the viewport request sender, `slint::quit_event_loop()`.
- **Autosave.** `arm_autosave` starts the 60 s single-shot timer if none is running and `needs_autosave()`. When it fires and the document still needs autosaving, submit `Autosave{doc.clone(), title, location}`. On `Autosaved Ok`, set `autosaved_state`. A failure shows the status chip "Autosave failed" (no sheet) and re-arms. No timer runs while the document is clean.
- **Startup** (`main.rs`):
  1. `load_settings()`.
  2. `AutosaveSession::start(default_root(), title)`; on failure set the status "Autosave unavailable" and continue.
  3. `find_recoverable(root, Some(own_dir))`.
  4. `initial_session`: argv[1] if present (loaded synchronously before the window shows; on failure use the demo and show the error after the window appears), else `demo_document()`, clean and untitled.
  5. `IoWorker::start`.
  6. Window and viewport as in M0, plus `SetWallDisplay(settings.wall_display)`.
  7. After the first frame, `files::startup` shows one sheet per candidate, newest first: "Recover unsaved changes?" with "“{title}” — autosaved {HH:MM}{, original file: path}" and [Restore] [Discard] [Later].
     - **Restore:** `candidate.load()` → `replace_document(Session::from_recovered)` with `location = info.location`. Immediately submit an Autosave into the new session, and on `Autosaved Ok` call `candidate.discard()`. If loading fails, show an error sheet and `release()` the candidate.
     - **Discard:** `discard()`.
     - **Later:** `release()`.
     - **Newer format:** only [Later].

### 7.7 Event flow
```
Slint field/choice/action/key ─► controller (borrow → edit.rs pure → drop borrow)
      │                                   │ EditOutcome::Command
      │                                   ▼
      │                 Session.execute/undo/redo (History, state ids) ── ChangeSet ──► ViewportRequest::ApplyChanges{snapshot, changes}
      │                                   │                                                     │ sync.rs plan → spawn/despawn/place (1–3 frames, then idle)
      │                                   ├─► title/dirty, undo labels, scene + inspector models (in place)
      │                                   └─► files::arm_autosave (single-shot 60 s, only when dirty)
      │ EditOutcome::Preference ─► Settings (save_settings) ─► SetWallDisplay / refresh   (no history, no dirty)
Viewport thread ─► ViewportEvent ─► wake_ui ─► bridge ─► SelectionChanged ─► Session.selection ─► inspector
IoWorker thread ─► IoResult ─► invoke_from_event_loop ─► files::handle_io_result
Dialog thread ─► DialogResult ─► invoke_from_event_loop ─► files flow continues
```

### 7.8 lab-app tests (`cargo test -p lab-app --lib`)
- **E1.** `field_edit` on every platform and room field id. A valid value gives the expected command. `"abc"` gives Parse. Unchanged text gives `Nothing`. **In inches, re-committing "47.244" on a 1200 mm width gives `Nothing`.** Inch input works on a mm display.
- **E2.** A corner edit producing a bow-tie gives `Invalid` containing "cross", and `invalid_row` points at the right row.
- **E3.** A stale corner index gives Stale.
- **E4.** A room corner move that strands a door gives a `Command` with a `Moved` or `Removed` fix.
- **E5.** Switching the grid standard keeps the zones.
- **E6.** Outline kind switches as specified.
- **E7.** Adding a door to a room whose walls are all short gives Invalid.
- **E8.** Deleting a platform gives `RemovePlatform`.
- **E9.** Pinning: an outline edit on a platform with nodes converts Auto to `At`, and hole positions are unchanged for every node position.
- **E10.** A units or walls choice gives `Preference`, never a command.
- **E11.** `parse_pasted_corners` handles commas, tabs and an error line number.
- **V1.** Subtitle, summary and preview for a rectangle and an L. **V2.** SVG y negation. **V3.** Scene rows: Room first, nodes nested. **V4.** `wall_choice_label` compass. **V5.** `window_title`.
- **F1.** Session dirty logic: edit → dirty; undo → clean; save at state 3, undo to 2 → dirty, redo → clean; a new edit after undoing past the save stays dirty; `from_recovered` is dirty.
- **F2.** `needs_autosave`.

---

## 8. Task breakdown

### 8.1 Tasks (file ownership is exclusive; a later task may take over a file only if it depends on the earlier owner)

| Id | Title | Owns | Depends on |
|---|---|---|---|
| T0 | Scaffold + v1 fixture | root `Cargo.toml`, `Cargo.lock`, all crate `Cargo.toml`s, `crates/lab-geom/src/lib.rs` (stub), `crates/lab-file/src/lib.rs` (stub), `crates/lab-core/tests/fixtures/demo_v1.json` | — |
| T1 | lab-geom | `crates/lab-geom/src/**` | T0 |
| T2 | lab-core | `crates/lab-core/src/**`, `crates/lab-core/tests/**` except the fixture | T0, T1 |
| T3 | lab-file | `crates/lab-file/src/**`, `crates/lab-file/tests/**` | T0, T2 |
| T4a | Hole shader spike | `crates/lab-viewport/src/hole_material.rs`, `crates/lab-viewport/src/hole_grid.wesl` (built and measured in a scratch worktree) | T0 |
| T4b | Viewport integration | `crates/lab-viewport/src/**` except the T4a files and the final `room.rs` | T1, T2, T4a |
| T4c | Room walls and fade | `crates/lab-viewport/src/room.rs` | T4b |
| T5 | lab-ui | `crates/lab-ui/ui/**` (including the new `types.slint`), `crates/lab-ui/examples/gallery.rs` | T0 |
| T6-spike | Dialog spike | `crates/lab-app/src/dialogs.rs` (built in a scratch worktree, committed by T6b) | T0 |
| T6a | App logic library | `crates/lab-app/src/{lib.rs, session.rs, edit.rs, views.rs}` | T2, T3 |
| T6c | App shell and controller | `crates/lab-app/src/{main.rs, app_state.rs, bridge.rs, ui_models.rs, controller.rs}` + stubs `files.rs`, `dialogs.rs` | T4b, T5, T6a |
| T6b | Files, dialogs, autosave flows | `crates/lab-app/src/{files.rs, dialogs.rs}` | T3, T6c, T6-spike |
| T7 | Verification + docs | `docs/DESIGN.md`, `README.md`, `scripts/**` (optional helpers) | all |

**Phase plan:** T0 → {T1, T4a, T5, T6-spike} in parallel → T2 (may start on T1's first signature-only commit) → {T3, T4b} → {T4c, T6a} → T6c → T6b → T7.

### 8.2 Which crates build at each point (per-crate gates, never `--workspace` until T6b)
- After T0: the whole workspace builds (M0 code plus empty stubs).
- After T2 merges: `lab-viewport` and `lab-app` **do not compile** (they use removed M0 APIs). This is expected. Gates: `cargo test -p lab-geom`, `-p lab-core`.
- After T4b: `lab-viewport` compiles again (`cargo test -p lab-viewport`).
- After T5: `lab-ui` compiles, and `lab-app`'s binary still does not. T6a's gate is `cargo test -p lab-app --lib` (the library target does not need the binary).
- After T6c: the workspace compiles, with file actions as stubs (no-ops plus the toast "Not available yet").
- After T6b: `cargo test --workspace` is green and the app is fully functional.

### 8.3 Per-task acceptance (summary; full criteria in the structured task list)
Every task also requires:
- `cargo clippy -p <crate> -- -D warnings` (if clippy is installed)
- no Bevy feature change
- `scripts/check-offline.ps1` passing once the workspace compiles

---

## 9. Exit criteria: how each is verified (T7)

| Criterion | Verification |
|---|---|
| Round-trip save/load tests pass | lab-file P1–P7 (byte-identical re-save; folder; plain JSON; v1 fixture), lab-core M1–M5, S1 (crash recovery with a real child process) |
| A custom polygon table renders with correct holes | lab-core H1–H8 (oracle). lab-viewport mask, energy and no-pop tests. Manual: a 7-corner custom polygon with fillets and 2 zones, plus an L table, in 3D and Top, PrintWindow screenshots at 5 zoom levels (0.05–40 m). No hole outside the outline or inside the margin or zones; the visible count matches `HoleSet::len()` on a small board. |
| App idles at 0% CPU/GPU | After startup, an edit, an undo, a wall fade, a save and with a dirty document (autosave armed): 12 s of `\GPU Engine(pid_*)\Utilization Percentage` and process CPU at 0.00% (M0 PostMessage/PrintWindow method). `FrameReady` stops within 3 frames of an edit and within 300 ms of a fade. The only allowed wakeup is the single 60 s autosave timer. |
| All existing tests still pass | 21 of 22 M0 tests kept or ported with identical expected values. `hole_tile_has_full_mip_chain` is retired with `hole_tile_image` (documented) and replaced by the mask tests. |
| Offline check passes | `scripts/check-offline.ps1`: 419 runtime crates + `lab-geom` + `lab-file`, no new external crates. |
| End-to-end | New → add an MB6060/M breadboard on Table A → switch it to Custom, edit corner 2, insert a corner, add a circle zone → add a door, make the room an L (the door is refit, toast) → Ctrl+Z ×3 / Ctrl+Y ×2 → Save As (archive) → close → reopen (identical, title clean) → Save as Folder → open `project.json` → make edits, wait 60 s, kill the process → restart → Restore → same content. Also: inch display + Enter on an unchanged field leaves the title clean. |

T7 also records the frame time of a full-resolution frame against M0 (LABSETUP_TRACE) and the first-fade hitch, and updates DESIGN.md: §4.2 (units preference, v2 model), §4.5 (container details), §11 (M1 done) and a new §14 "M1 results". It verifies the preset part-number hints against thorlabs.com and blanks any unconfirmed ones.

---

## 10. Risks and de-risking

| # | Risk | Mitigation |
|---|---|---|
| R1 | The WESL ExtendedMaterial (imports, `constants::MATERIAL_BIND_GROUP`) fails to compile or render on DX12 / Intel UHD | T4a is a gate in a scratch worktree before any viewport work. Fallbacks in order: (a) a custom `Material` with its own WESL that imports only `forward_io::VertexOutput` and does Lambert + ambient with the two scene lights hard-coded, using the same hole logic; (b) the M0 tiled hole texture multiplied by the presence mask (loses crispness at extreme zoom). Record the outcome in DESIGN.md. No new Bevy features in any case. |
| R2 | On-demand rendering vs. GPU asset readiness | Synchronous shader insert; `SettleFrames(2)` after every spawn; data-driven framing. T4b acceptance checks that `FrameReady` stops within 3 frames. |
| R3 | The first Blend pipeline (wall fade) or HoleTop pipeline compiles synchronously, causing a hitch on Intel UHD | Measure in T7. If the first fade hitches by more than 100 ms, pre-warm in the 3 warm-up frames by rendering one wall at alpha 0.99 for one frame. |
| R4 | Win32 dialog on a non-UI thread with a cross-thread owner (attached input queues) | The T6-spike proves it in the running app (open, cancel and save with the viewport rendering, no hang). Fallback: run the dialog on the UI thread from `Timer::single_shot(0)` with no `RefCell` borrow alive and `busy` guarding re-entrant actions. |
| R5 | Slint key routing (TextInput swallows Ctrl+Z; capture ordering; the focus-tick trick; sheet and PasteSheet focus scopes) | Covered in the T5 gallery with a shortcut test page. Fallback for Ctrl combinations: `KeyBinding { keys: @keys(Control + S); }` inside the root FocusScope (verified syntax in `i-slint-compiler-1.18.1/tests/syntax/elements/key_binding_valid.slint`). |
| R6 | Geometry robustness (near-collinear points, concave fillets, ear-less polygons) | Strict validation, re-validation after flattening, a deterministic triangulation fallback, seeded fuzz tests against oracles. On a mesh failure the platform is skipped with `SceneError`, never a crash. |
| R7 | Atomic rename blocked by OneDrive or antivirus | Retries with backoff, typed Locked/PermissionDenied errors, the original never touched, the autosave keeps the data, Save As offered. |
| R8 | Hand-written zip interoperability | Strict bounded reader, a data-descriptor test, a manual Expand-Archive/Compress-Archive check (Z8). |
| R9 | Slint focus loss on refresh | Persistent VecModels and in-place updates (D16); the NumberField explicit-sync contract. |
| R10 | Wall fade flicker or wrong walls in concave rooms | Hysteresis, the segment test, a pure tested predicate with the judges' counterexample, the eye taken from the current-frame camera `Transform` (not the stale `GlobalTransform`), Top view exempt, and a Show walls preference as a user escape hatch. |
| R11 | Dense grids exceed the texture limit or CPU budget | Validation limits (2046 per axis, 250k), the row-bucket algorithm, opt-level 2 for lab-core and lab-geom, hole caching in the app. |
| R12 | An accidental Bevy, wgpu or png feature change triggers a 15–60 min rebuild | Bevy features are frozen; flate2 is used with exactly its in-tree default features; T0 checks that `cargo build -v` compiles no `bevy_*` crate. |
| R13 | Preset part numbers wrong | Hints are display-only, sizes are editable, and T7 verifies or blanks them. |
| R14 | Selection races (a deleted item still selected) | The viewport clears the selection on snapshot changes; the app validates `SelectionChanged` against the document. |
| R15 | Scope (two new crates, mitred walls with reveals) | The phase plan lets lab-geom, lab-ui and both spikes run in parallel; out-of-scope polish is listed in §11. |
| R16 | Slint compile pitfalls (recursive imports, `<=>` with `in` properties, binding breakage by assignment, styled-text tooltips) | Structs live in the leaf `types.slint`; SegmentedControl is controlled; Tooltip uses `@markdown("\{…}")`; T5's no-warnings gate. |

## 11. Out of scope for M1
- Dragging corners or platforms in the viewport, and gizmos (M2). Coalesced drag undo is implemented in History but unused.
- Platform stacking as a real parent relation (a breadboard is only *placed* on the selected table). Isolators.
- Polygon exclusion zones, individual extra or removed holes, door leaves, room fixtures, multiple rooms, ceilings.
- A corner marker in the 3D viewport, a selection ribbon, a ground shadow, the hover hole readout.
- Recent files, Revert, Show in Explorer, reduce-motion, an expression parser in fields, duplicate platform.
- `thumbnail.png` and `parts/` inside the archive (M4/M5).


---

## Appendix: open questions, answered (2026-10-18)

Answers from the user (product decisions) and defaults confirmed by the lead (technical):

| Question | Answer |
|---|---|
| Startup document | **Demo project** (keep M0 behaviour: the demo document, clean and untitled, when no file is given). `default_document()` is still used for File > New. |
| Unsaved-changes title | **Apple style**: "{name} — Edited — LabSetup" plus the "Edited" pill. |
| Hole-lattice pinning | **Re-centre until parts exist**: `GridOrigin::Auto` is pinned only when the platform has nodes (D13 as written). |
| Door refit on room shape edits | As specified: move or remove automatically inside the same command, toast with Undo; direct invalid door edits are rejected. |
| Preset part numbers | As specified: T7 verifies MB-series hints against thorlabs.com and blanks unconfirmed ones; tables are labelled by size. |
| Win32 dialog fallback | Approved in advance (UI-thread dialog via `Timer::single_shot(0)` if the STA-thread spike fails). |
| Non-ASCII paths | Yes: T7's end-to-end run includes a project saved in a folder with a Thai name. |
| Paste sheet | Ctrl+Enter or the Apply button. |
| Preview card strokes | Uniform 1.5 px strokes; error edges in red. |
