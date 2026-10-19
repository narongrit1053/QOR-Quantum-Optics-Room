# M2 plan: Library + placement

Milestone M2 of DESIGN.md §11, built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus its one happy-path check), and other flaws go to the Defect log. Read this file and your own section; open only the files your task touches.

**Exit criterion (§11):** build a 20-component table at 75 mm beam height without typing coordinates.

Conventions for every task: millimetres and radians, +Z up. Part-local frame: the part stands upright, origin at its **bottom centre** (optics: their centre), optical axis along **+Y**. Connector frames: `origin`, `z` (mating axis, pointing **out of** the part), `x` (reference for twist).

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 2 | POLA-36 | lab-catalog | Engineer Opus Low | `crates/lab-catalog/**`, `crates/lab-core/src/part.rs` (+ `pub mod part;` in `lab-core/src/lib.rs`), workspace `Cargo.toml` (member, `toml` dep) |
| 2 | POLA-37 | lab-parts | Engineer Opus Low (was Opus) | `crates/lab-parts/**`, workspace `Cargo.toml` (member) |
| 2 | POLA-38 | lab-mates: connectors + snap | Engineer Opus Low (was Opus) | `crates/lab-mates/**` except `solver.rs`, workspace `Cargo.toml` (member) |
| 3 | POLA-39 | Built-in library content | Engineer Sonnet | `library/parts/**/*.toml` |
| 3 | POLA-40 | Beam-height solver | Engineer Opus Low | `crates/lab-mates/src/solver.rs` (+ `pub mod solver;`) |
| 3 | POLA-41 | Document: parts, attach, assemblies | Engineer Opus | `lab-core` model/command/migrate/validate/history, compile fixes in `lab-viewport` and `lab-app` |
| 3 | POLA-42 | Library sidebar (moved from stage 4) | Engineer Opus Low | `lab-ui` library section, `lab-app/src/library.rs`, catalog load in `lab-app/src/main.rs` + `app_state.rs`, `ViewportRequest` additions in `lab-viewport/src/lib.rs` |
| 4 | POLA-43 | Viewport placement | Engineer Opus | `lab-viewport/src/place.rs` (new), `scene.rs` (part meshes), `lab-app/src/place.rs` (new) |
| 4 | POLA-44 | Platform stacking and supports | Engineer Opus Low | `lab-viewport/src/platform.rs`, platform inspector section (`lab-app/src/edit.rs` platform part, `lab-ui/ui/inspector.slint` support rows) |
| 5 | POLA-45 | Gizmo + part inspector | Engineer Opus | `lab-viewport/src/gizmo.rs` (new), node inspector (`lab-app/src/edit.rs` node part, `inspector.slint` node rows) |
| 5 | POLA-46 | Stack builder UI | Engineer Opus Low | `lab-ui/ui/stack.slint` (new), `lab-app/src/stack.rs` (new), `DragItem::Stack` |
| 5 | POLA-47 | Scene tree | Engineer Sonnet | scene section of `app.slint`, `SceneRow` in `types.slint`, scene rows in `lab-app/src/ui_models.rs` |

Tier changes: POLA-37 and POLA-38 drop to Opus Low (generators are boolean-free boxes and lathes over `lab-geom` helpers; snap is a wrapper over `HoleSet::nearest`, which already handles outlines and exclusion zones). POLA-42 moves to stage 3: it needs only the catalog, and it must land the `ViewportRequest` variants before POLA-43 implements them. Platform pose composition moves into POLA-41 so POLA-43 and POLA-44 can run in parallel.

**Merges (Engineer Sonnet).** Tasks in one stage all add lines to the workspace `Cargo.toml` and sometimes to `lab-app/src/controller.rs` or `main.rs`; keep both sides (`Integration fix:` commit if needed). In stage 2, `lab-core/src/part.rs` is added by all three branches **byte-identical** (see below); if git still conflicts, take the draft side.

## Offline deviation: no SQLite

`rusqlite`/`libsqlite3-sys` are not in the offline registry (`D:/dev/cargo/registry`), so §5's `catalog.sqlite` + FTS5 cannot be built. lab-catalog keeps an **in-memory token index** built at load time (a few hundred TOML files parse in well under 100 ms). Record this in DESIGN.md §5 during Hardening. `toml 1.1.8` is already in `Cargo.lock` and cached: add `toml = "1.1"` to `[workspace.dependencies]`, nothing else new. No `cargo update`.

## Shared interface: `crates/lab-core/src/part.rs`

Stage-2 tasks all compile against this. POLA-36 owns it; POLA-37 and POLA-38 create the **same file, byte for byte**, plus `pub mod part;` directly after `pub mod outline;` in `lab-core/src/lib.rs` (and nothing else in lab-core). Do not change it in stage 2; changes later go through the Lead.

```rust
//! Catalog part definitions (DESIGN.md §4.1). Pure data, shared by lab-catalog, lab-parts and
//! lab-mates. Part-local frame: upright, origin at the bottom centre (optics: centre), +Y optical axis.

use std::collections::BTreeMap;

use glam::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    /// `vendor:part_number`, e.g. `thorlabs:KM100`, or `generic:<name>`.
    pub id: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub part_number: String,
    pub name: String,
    /// Slash path, e.g. `mounts/kinematic` (see the category list in docs/M2-PLAN.md).
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub geometry: Geometry,
    #[serde(default)]
    pub connectors: Vec<ConnectorDef>,
    #[serde(default)]
    pub optical_ports: Vec<OpticalPort>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    1
}

impl PartDef {
    pub fn connector(&self, name: &str) -> Option<&ConnectorDef> {
        self.connectors.iter().find(|c| c.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    Parametric {
        generator: String,
        #[serde(default)]
        params: Params,
    },
    /// Content-addressed GLB (`sha256:…`), M4.
    Mesh { asset: String },
}

pub type Params = BTreeMap<String, ParamValue>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Number(f64),
    Text(String),
    Flag(bool),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Number(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectorDef {
    pub name: String,
    #[serde(flatten)]
    pub kind: ConnectorKind,
    pub frame: Frame,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorKind {
    /// `spec`: "M4", "M6", "8-32", "1/4-20".
    Thread { spec: String, gender: Gender },
    /// Post in a holder. On the female side `range = [min, max]` bounds the slide (mm along the
    /// female frame's +Z; negative = deeper into the bore).
    Bore {
        diameter: f64,
        gender: Gender,
        #[serde(default)]
        range: Option<[f64; 2]>,
    },
    /// Optic in a mount: female = the mount's seat, male = the optic's edge.
    OpticBore { diameter: f64, gender: Gender },
    /// Bolts to a platform hole. Mates with platform holes, never with another connector.
    HoleGridSeat,
    CageRod { spacing: f64, gender: Gender },
    /// `thread`: "SM05", "SM1", "SM2".
    LensTube { thread: String, gender: Gender },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: DVec3,
    /// Mating axis, pointing out of the part. Unit length.
    pub z: DVec3,
    /// Twist reference, perpendicular to `z`. Unit length.
    pub x: DVec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpticalPort {
    pub name: String,
    /// "mirror", "lens", "splitter", "waveplate", "source", "detector", "dump", "fiber".
    pub kind: String,
    #[serde(default)]
    pub clear_aperture: Option<f64>,
    pub frame: Frame,
}
```

TOML shape (one part per file; the example lab-catalog's test parses):

```toml
id = "thorlabs:KM100"
vendor = "Thorlabs"
part_number = "KM100"
name = "Kinematic Mirror Mount, Ø1\" Optics"
category = "mounts/kinematic"
tags = ["mirror", "kinematic", "1in"]

[geometry]
kind = "parametric"
generator = "kinematic_mount"
params = { optic_diameter = 25.4, width = 38.1, height = 38.1, thickness = 12.7, axis_height = 20.3 }

[[connectors]]
name = "base_tap"
kind = "thread"
spec = "8-32"
gender = "female"
frame = { origin = [0, 0, 0], z = [0, 0, -1], x = [1, 0, 0] }

[[connectors]]
name = "optic_seat"
kind = "optic_bore"
diameter = 25.4
gender = "female"
frame = { origin = [0, 0, 20.3], z = [0, 1, 0], x = [1, 0, 0] }

[[optical_ports]]
name = "surface"
kind = "mirror"
clear_aperture = 22.9
frame = { origin = [0, 0, 20.3], z = [0, 1, 0], x = [1, 0, 0] }
```

## Contracts used across tasks

**Categories** (POLA-39 uses exactly these; the sidebar groups by the first segment):
`posts/posts`, `posts/pedestals`, `posts/holders`, `posts/bases`, `mounts/kinematic`, `mounts/fixed`, `mounts/rotation`, `mounts/periscopes`, `optics/mirrors`, `optics/lenses`, `optics/cubes`, `optics/waveplates`, `beam/irises`, `beam/dumps`, `fiber/couplers`, `modulators/aom-eom`, `detectors/photodiodes`, `detectors/cameras`, `detectors/spcm`, `crystals/ovens`, `lasers/heads`, `room/racks`, `room/desks`, `room/shelves`, `room/curtains`.

**Stack connector names** (the solver and the stack builder look these up by name):

| Role | Category | Connectors |
|---|---|---|
| base / fork | `posts/bases` | `seat` (hole_grid_seat, origin 0, z −Z) · `top` (thread male, origin at the base's top, z +Z) |
| holder | `posts/holders` | `bottom` (thread female, origin 0, z −Z) · `bore` (bore female, `range`, origin at the holder's top, z +Z); also `seat` so it can bolt to the table alone |
| post | `posts/posts` | `bottom` (bore male, origin 0, z −Z) · `top` (thread male: the set screw, origin at z = length, z +Z) |
| mount | `mounts/*` | `base_tap` (thread female, origin 0, z −Z) · `optic_seat` (optic_bore female, origin at the axis height, z +Y) |
| optic | `optics/*` | `edge` (optic_bore male, origin 0, z −Y) |
| anything bolted to a table | any | `seat` (hole_grid_seat, origin 0, z −Z) |

**Mate pose.** Child connector origin = parent connector origin + `slide`·parent z; child z = −parent z; child x = parent x turned by `twist` about parent z. As frames: `F = P · T(0, 0, slide) · Rz(twist) · Rx(π)`, child pose `= F · C⁻¹`. A platform hole is the frame `(hole_x, hole_y, 0)`, z +Z, x +X.

---

## Stage 2

### POLA-36 lab-catalog (Engineer Opus Low)
Owns `crates/lab-catalog/` (deps: `lab-core`, `serde`, `toml`, `thiserror`), `lab-core/src/part.rs`, the workspace member and `toml` dependency.

```rust
pub enum Source { BuiltIn, User }
pub struct LibraryDir { pub path: PathBuf, pub source: Source }
pub struct CatalogWarning { pub path: PathBuf, pub message: String }   // one complete sentence

pub struct Catalog { /* defs by id, token index */ }                     // Send + Sync, shared as Arc<Catalog>
impl Catalog {
    /// Reads `<dir>/parts/**/*.toml`; later dirs override earlier ones by id. Bad files are skipped with a warning, never a panic.
    pub fn load(dirs: &[LibraryDir]) -> (Catalog, Vec<CatalogWarning>);
    /// builtin_dir() then user_dir() (if it exists).
    pub fn load_default() -> (Catalog, Vec<CatalogWarning>);
    pub fn from_defs(defs: Vec<PartDef>) -> Catalog;                     // tests and other crates' tests
    pub fn get(&self, id: &str) -> Option<&PartDef>;
    pub fn source(&self, id: &str) -> Option<Source>;
    /// Sorted by category, then name.
    pub fn all(&self) -> Vec<&PartDef>;
    pub fn categories(&self) -> Vec<String>;                             // sorted, unique, full paths
    pub fn in_category(&self, prefix: &str) -> Vec<&PartDef>;            // "posts" matches "posts/holders"
    /// Case-insensitive; every whitespace token must prefix-match a word of name, vendor, part
    /// number, id, tags, category or connector kinds. Exact part-number match ranks first, then
    /// name-prefix matches, then the `all()` order. Empty query = all().
    pub fn search(&self, query: &str, limit: usize) -> Vec<&PartDef>;
    pub fn len(&self) -> usize;
}
/// LABSETUP_LIBRARY_DIR, else `<exe dir>/library`, else `<CARGO_MANIFEST_DIR>/../../library`.
pub fn builtin_dir() -> PathBuf;
/// `%APPDATA%\LabSetup\library`.
pub fn user_dir() -> Option<PathBuf>;
```
**Works when:** one unit test loads two temp dirs (the KM100 TOML above in both, user copy renamed) and shows the user copy wins, and `search("km1")`, `search("thorlabs mirror")`, `search("8-32")` and `search("kinematic")` each find it.

### POLA-37 lab-parts (Engineer Opus Low)
Owns `crates/lab-parts/` (deps: `lab-core`, `lab-geom`). Meshes are `lab_geom::TriMesh` in part-local mm (+Z up, positions in the part frame, outward normals, CCW).

```rust
pub struct PartMesh { pub pieces: Vec<(lab_core::Finish, TriMesh)>, pub min: DVec3, pub max: DVec3 }
#[derive(Debug, thiserror::Error)]
pub enum GenError { #[error("Unknown part generator \"{0}\".")] Unknown(String),
                    #[error("Parameter {name} {reason}.")] BadParam { name: String, reason: String } }
pub struct GeneratorInfo { pub name: &'static str, pub params: &'static [(&'static str, f64)] } // name, default
/// Missing params take defaults; non-positive or non-finite sizes are BadParam. Cached by a hash
/// of the generator name and the params' sorted (name, value bits); same inputs return the same Arc.
pub fn generate(generator: &str, params: &Params) -> Result<Arc<PartMesh>, GenError>;
pub fn generators() -> &'static [GeneratorInfo];
```
Generators (names are the contract with POLA-39; parameters in mm, defaults in brackets; `axis_height` = optical axis above the bottom):
`post` (diameter 12.7, length 50) · `pedestal_post` (diameter 25, length 50, flange_diameter 38) · `post_holder` (bore 12.7, height 50, base_diameter 30) · `base` (length 75, width 25, thickness 10) · `clamping_fork` (length 50, width 25, thickness 10) · `kinematic_mount` (optic_diameter 25.4, width 38.1, height 38.1, thickness 12.7, axis_height 20.3) · `fixed_mount` (optic_diameter 25.4, outer_diameter 38, thickness 10, axis_height 20.3) · `rotation_mount` (optic_diameter 25.4, outer_diameter 50, thickness 15, axis_height 30) · `periscope` (width 50, height 150, axis_height 50) · `mirror` (diameter 25.4, thickness 6) · `lens` (diameter 25.4, thickness 4) · `cube` (size 25.4) · `waveplate` (diameter 25.4, thickness 2) · `iris` (outer_diameter 50, aperture 12, thickness 8, axis_height 30) · `beam_dump` (diameter 25, length 40, axis_height 25) · `fiber_coupler` (diameter 25, length 30, axis_height 25) · `modulator_box` (length 60, width 40, height 30, axis_height 20) · `photodiode` (length 40, width 20, height 50, axis_height 25) · `camera` (length 50, width 30, height 30, axis_height 15) · `spcm` (length 120, width 60, height 50, axis_height 25) · `crystal_oven` (length 60, width 40, height 30, axis_height 20) · `laser_head` (length 200, width 80, height 60, axis_height 40) · `rack_19in` (units 42, depth 800) · `desk` (width 1500, depth 750, height 750) · `shelf` (width 1200, depth 400, thickness 20, height 500) · `laser_curtain` (width 2000, height 2000).

**Works when:** one unit test per generator family checks bounds against the nominal sizes (e.g. `post` 50 mm long has `min.z = 0`, `max.z = 50`, x/y extent 12.7 ± 0.1), and a second `generate` with the same params returns the same `Arc` (`Arc::ptr_eq`).

### POLA-38 lab-mates: connectors and snap (Engineer Opus Low)
Owns `crates/lab-mates/` (deps: `lab-core`) except `solver.rs`.

```rust
/// Same variant, matching spec/diameter (±0.05 mm)/spacing/thread, opposite genders. HoleGridSeat → false.
pub fn compatible(a: &ConnectorKind, b: &ConnectorKind) -> bool;
/// (child connector index, parent connector index) for every compatible pair, in index order.
pub fn compatible_pairs(child: &PartDef, parent: &PartDef) -> Vec<(usize, usize)>;
/// Slide limits when `parent` is a female bore with a range (else (0, 0)).
pub fn slide_range(parent: &ConnectorKind) -> (f64, f64);
/// Child part pose in the parent part's frame (formula under "Mate pose").
pub fn mate_pose(parent: &Frame, child: &Frame, slide: f64, twist: f64) -> (DVec3, DQuat);
/// Part pose in the platform frame when its `seat` connector is bolted to the hole at `hole`,
/// turned by `angle` about +Z. A part without a seat stands with its origin on the hole.
pub fn seat_pose(part: &PartDef, hole: DVec2, angle: f64) -> (DVec3, DQuat);

pub struct SnapHit { pub hole: LatticeIndex, pub position: DVec2, pub distance: f64 }
/// Nearest existing hole to `p` (platform-local) within `max_distance`, using HoleSet::nearest
/// (outlines and exclusion zones are already applied there).
pub fn snap(holes: &HoleSet, p: DVec2, max_distance: f64) -> Option<SnapHit>;
```
**Works when:** unit tests: an 8-32 male/female pair is compatible and male/male is not; `mate_pose` for a post (`bottom` at origin, z −Z) into a holder bore at (0, 0, 50) with slide −10 puts the post origin at (0, 0, 40) upright; `snap` on a platform with a circle exclusion zone never returns a hole inside the zone.

---

## Stage 3

### POLA-39 Built-in library content (Engineer Sonnet)
Owns `library/parts/**/*.toml`, one file per part, path = category (`library/parts/posts/holders/thorlabs-PH50_M.toml`). Use the generators and connector names above. Metric Thorlabs parts first (M4 posts/M6 table), plus the imperial counterparts for the posts, holders, bases and the three mount sizes.

Minimum set: posts TR20/TR30/TR40/TR50/TR75/TR100 (/M) and TR1–TR4 (imperial); pedestal RS-series 2; holders PH20/PH30/PH40/PH50/PH75/PH100 (/M), `bore.range` = `[-(height − 5), -5]`; bases BA1/M, BA2/M; forks CF125; kinematic KM05, KM100, KM200; fixed FMP1; rotation RSP1; periscope RS99; optics: mirror ½"/1"/2", lens ½"/1"/2", PBS and BS cubes ½"/1", waveplate ½ and ¼ 1"; iris ID25; beam dump LB1; fiber coupler PAF2; AOM and EOM box; photodiode PDA36A; camera DCC1545M; SPCM (generic); crystal oven (generic); generic laser head; room: 19" rack 42 U, desk, shelf, laser curtain. Every optic and active device gets an `optical_ports` entry; everything that stands on a table gets a `seat`.

**Works when:** a throwaway test (or `cargo run` scratch in your worktree, not committed) shows `Catalog::load_default()` loads every file with zero warnings and `lab_parts::generate` succeeds for every part. Check by the solver formula below that KM100 on the built-in posts, holders and bases reaches a 75 mm beam height.

### POLA-40 Beam-height solver (Engineer Opus Low)
Owns `crates/lab-mates/src/solver.rs`.

```rust
pub struct StackSolution { pub base: Option<String>, pub holder: String, pub post: String,
                           pub slide: f64, pub margin: f64, pub beam_height: f64 }
pub enum NoFit { MountHasNoTap, NoCandidates, OutOfRange { closest: f64 } }  // + Display: one sentence
pub fn solve_stack(mount: &PartDef, beam_height: f64, candidates: &[&PartDef]) -> Result<StackSolution, NoFit>;
```
Roles by category (`posts/bases`, `posts/holders`, `posts/posts`) and connectors by name. With `a` = mount `optic_seat.z − base_tap.z`, `b` = base `top.z` (0 without a base), `h` = holder `bore.z`, `L` = post `top.z`: `slide = H − a − b − h − L` must lie in the holder's `bore.range`, and the post's `bottom` must be `compatible` with the holder's `bore`. Try no base and every base. Prefer the shortest post, then the largest margin `min(slide − min, max − slide)`, then ids in order (deterministic). `OutOfRange.closest` is the nearest reachable beam height.

**Works when:** one unit test with inline defs (KM100, TR50/M, TR75/M, PH50/M, BA1/M) solves 75 mm to the post and holder you work out by hand from the formula, and 500 mm returns `OutOfRange`.

### POLA-41 Document: placed parts, attachments and assemblies (Engineer Opus)
Owns `lab-core` (`model.rs`, `command.rs`, `migrate.rs`, `validate.rs`, tests) and the compile fixes this causes in `lab-viewport` (`sync.rs`, `scene.rs`) and `lab-app`. **Schema 3.**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parent { Room, Platform(PlatformId), Node(NodeId) }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Attach {
    Free,
    Hole { hole: LatticeIndex },
    Mate { self_conn: String, parent_conn: String, slide: Mm, twist: f64 },
}

pub struct Node {
    pub id: NodeId, pub name: String,
    pub part: String,                 // catalog id
    pub parent: Parent,               // replaces `platform`
    pub attach: Attach,
    /// Relative to the parent frame: room, platform frame, or the parent node's part frame.
    /// For Shape::Box/Cylinder: centre of the shape (as in M1). For Shape::Part: the part origin.
    pub position: DVec3, pub rotation: DQuat,
    pub shape: Shape,                 // + new variant `Part`: mesh from the catalog def + params
    pub finish: Finish,
    #[serde(default)] pub params: Params,   // overrides of the def's generator params
    pub pickable: bool,
    #[serde(default)] pub locked: bool,
}

pub enum Support {                    // existing Legs, Fixed, plus:
    Legs { size: Mm, inset: Mm },
    Fixed,
    /// Pneumatic isolators under the corners (rendered by POLA-44).
    Isolators { size: Mm, inset: Mm },
    /// Sits on `parent` on standoffs: `position`/`rotation_z` are in the parent's platform frame
    /// and the platform's top is `standoff + thickness` above the parent's top (`top_height` unused).
    OnPlatform { parent: PlatformId, standoff: Mm },
}

impl Document {
    pub fn root_platform(&self, id: NodeId) -> Option<PlatformId>;       // None for room-rooted chains
    /// Pose in the root frame (root platform frame, or room frame for Parent::Room chains).
    pub fn node_pose(&self, id: NodeId) -> Option<(DVec3, DQuat)>;
    pub fn children(&self, parent: Parent) -> impl Iterator<Item = &Node>;
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId>;               // depth-first, document order
    /// Room-frame origin of the platform frame (on its top surface) and rotation about Z,
    /// composing OnPlatform chains.
    pub fn platform_pose(&self, id: PlatformId) -> Option<(DVec3, f64)>;
    // nodes_on(platform) keeps its signature: nodes whose root platform is `platform`.
}
```
Commands (each validates first, returns its inverse):
- `AddNode { node, index }` (unchanged) · `Rename` (unchanged) · `SetPlacement { id, position, rotation }` (this is §4.3's SetTransform)
- `RemoveNodes(Vec<NodeId>)` replaces `RemoveNode`: also removes descendants; inverse `RestoreNodes(Vec<(usize, Node)>)` (ascending index)
- `SetAttach { id, parent, attach, position, rotation }` (inverse = old values); `Command::mate(id, parent: NodeId, self_conn, parent_conn, slide, twist, position, rotation)` and `Command::unmate(id, position, rotation)` (to its root platform, `Attach::Free`) build it, with labels "Mate …"/"Unmate …"
- `SetParams { id, params }` · `SetLocked { id, locked }`
- `PlaceAssembly { nodes: Vec<Node> }`: adds all (parents may be earlier nodes of the list); ids are consecutive from `Document::next_free_id()`, checked like `AddNode`; inverse `RemoveNodes`; one undo step
- New errors: `UnknownParent`, `ParentCycle` (also for OnPlatform chains in `UpdatePlatform`), `PlatformHasChildren` (RemovePlatform of a platform others sit on)

ChangeSet: unchanged fields; a node update lists only that node. The viewport re-places the descendants of every added/updated node and every node on an updated platform (and on platforms stacked on it). Migration 2→3: `platform: p` → `parent: Platform(p)`, `attach: Free`, empty params, `locked: false`, note "Converted from an older format".

**Works when:** `cargo test -p lab-core --offline` passes with the existing tests updated and one new test: PlaceAssembly of base→holder→post→mount, undo restores the document, redo restores it; a v2 file migrates; the app builds and still shows the M1 sample.

### POLA-42 Library sidebar (Engineer Opus Low; moved to stage 3)
Owns the library section of `lab-ui/ui/app.slint` (+ `LibraryRow` in `types.slint`), `lab-app/src/library.rs` (new), catalog load in `main.rs`/`app_state.rs` (`pub catalog: Arc<Catalog>` on AppState; warnings go to the log and one toast), and these additions to `lab-viewport/src/lib.rs` (+ matching `FrameRequest` arms; the viewport stores the catalog and ignores drags until POLA-43):

```rust
pub enum ViewportRequest { /* … */ SetCatalog(Arc<lab_catalog::Catalog>), DragPart(PartDrag) }
#[derive(Clone, Debug, PartialEq)] pub enum DragItem { Part(String) }      // catalog id; POLA-46 adds Stack
#[derive(Clone, Debug, PartialEq)] pub enum PartDrag {
    /// Pointer over the viewport while dragging (physical px, viewport-relative).
    Move { item: DragItem, pos: Vec2, mods: Modifiers },
    Drop { pos: Vec2, mods: Modifiers },
    Cancel,
}
```
UI: search field on top; collapsible groups by first category segment (Posts, Mounts, Optics, Beam, Fiber, Modulators, Detectors, Crystals, Lasers, Room); rows show name and part number. `LibraryRow { id: string, label: string, detail: string, letter: string, tint: color, header: bool, expanded: bool }`. Callbacks `library-search(string)`, `library-toggle(string /*group*/)`, `library-drag(string /*part id*/, int /*0 move, 1 drop, 2 cancel*/, length /*window x*/, length /*window y*/)`. The row's TouchArea keeps the pointer grab while pressed; `library.rs` converts window → viewport physical px and sends `DragPart` only while the pointer is inside the viewport (Cancel when it leaves or on Esc).

**Works when:** the app starts with the built-in library listed by group, typing "km" filters to the kinematic mounts, and a drag from a row reaches the viewport (log line from the viewport per Move/Drop).

---

## Stage 4

### POLA-43 Viewport placement: drag-drop, snap and mating (Engineer Opus)
Owns `lab-viewport/src/place.rs` (new), `Shape::Part` rendering in `scene.rs` (meshes from `lab_parts::generate`, one Bevy mesh handle per `Arc<PartMesh>` piece, cached by `Arc` pointer), `lab-app/src/place.rs` (new), and:

```rust
pub enum ViewportEvent { /* … */ PlaceRequested(PlaceRequest) }
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceRequest { pub item: DragItem, pub parent: lab_core::Parent, pub attach: lab_core::Attach,
                          pub position: DVec3, pub rotation: DQuat }   // relative to `parent`, as Node stores it
```
During `PartDrag::Move` the viewport ray-casts (existing picking meshes): over a placed part with a compatible connector (`compatible_pairs`, nearest connector to the hit) the ghost takes `mate_pose`; over a platform top it takes `seat_pose` at `snap(...)` (max distance one pitch); Shift = free on the platform plane; elsewhere on the floor = `Parent::Room`, `Attach::Free`. The ghost is the part mesh with a translucent material, one frame per move, idle otherwise. `Drop` sends `PlaceRequested`; `lab-app/src/place.rs` turns it into `Command::AddNode` (name = def name, `Shape::Part`, finish from the mesh pieces, empty params) through the controller's existing `execute` path and selects the new node.

**Works when:** dragging PH50/M onto a table snaps it to a hole and drops it; dragging TR50/M onto that holder mates the post into its bore; undo removes the post.

### POLA-44 Platform stacking and supports (Engineer Opus Low)
Owns `lab-viewport/src/platform.rs` (standoffs: four posts from the parent top to this platform's bottom, inset like legs; isolators: four short cylinders under the corners), placement of stacked platforms through `Document::platform_pose`, and the Support row of the platform inspector (`edit.rs` platform part, `inspector.slint`): Legs / Isolators / Fixed / On platform (choice of parent platform + standoff height). Add a "Raised breadboard" entry next to the existing platform presets that creates a 300 × 300 breadboard `OnPlatform` the selected table with a 100 mm standoff.

**Works when:** a breadboard set On platform sits above the table on standoffs; moving the table carries the breadboard and the parts on both; isolators render.

---

## Stage 5

### POLA-45 Move/rotate gizmo and part inspector (Engineer Opus)
Owns `lab-viewport/src/gizmo.rs` (new), the node section of the inspector (`edit.rs` node part, `inspector.slint` node rows), and:

```rust
pub enum ViewportRequest { /* … */ SetTool(Tool) }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub enum Tool { #[default] Select, Move, Rotate }
pub enum ViewportEvent { /* … */ TransformRequested { id: NodeId, attach: Attach, position: DVec3, rotation: DQuat, done: bool } }
```
Move: drag on the platform plane, hole snap via `snap` (Shift free, Ctrl 1 mm steps); for a mated node, a vertical handle changes `slide` within `slide_range`. Rotate: about the vertical axis in 15° steps (Shift free). The app applies each event with `History::execute_coalescing` (`SetPlacement`, or `SetAttach` when the attach changes) and calls `end_gesture` on `done`. Locked nodes show no gizmo. Inspector for a part node: name, part number, attach ("Hole C14", "On TR50/M · slide −12 mm", "Free"), position x/y/z and rotation (editable, mm/in like M1 fields), slide, and the def's numeric params (`SetParams`). Toolbar Select/Move/Rotate buttons send `SetTool`.

**Works when:** a placed post moves from hole to hole with the gizmo, rotates in 15° steps, undo returns it, and typing a rotation in the inspector applies it.

### POLA-46 Beam-height stack builder UI (Engineer Opus Low)
Owns `lab-ui/ui/stack.slint` (new, mounted in the left sidebar under the library), `lab-app/src/stack.rs` (new), `DragItem::Stack(StackSpec)` in `lab-viewport/src/lib.rs` and its ghost in `place.rs` (all nodes of the stack).

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct StackSpec { pub optic: Option<String>, pub mount: String, pub base: Option<String>,
                       pub holder: String, pub post: String, pub slide: f64 }
```
Panel: optic picker, mount picker (filtered to `mounts/*`), beam height field (default 75 mm, mm/in), live solver result ("TR50/M in PH50/M on BA1/M · 4.2 mm margin") or the `NoFit` sentence in place of the drag handle. Dragging the result places the stack: `PlaceRequested` with `DragItem::Stack` → `Command::PlaceAssembly` of base (Hole) → holder (Mate bottom↔top) → post (Mate bottom↔bore, slide) → mount (Mate base_tap↔top) → optic (Mate edge↔optic_seat); without a base the holder sits on the hole by its `seat`.

**Works when:** KM100 + 1" mirror at 75 mm shows a solution, one drag places five nodes as one undo step, and the mirror centre is 75 ± 0.1 mm above the table top; 500 mm shows the no-fit sentence and no drag handle.

### POLA-47 Scene tree for placed parts (Engineer Sonnet)
Owns the scene section of `app.slint`, `SceneRow` in `types.slint` (add `locked: bool`, `expandable: bool`, `expanded: bool`), scene rows in `lab-app/src/ui_models.rs`, and the handlers in `lab-app/src/scene.rs` (new). Rows: room → platforms (stacked platforms under their parent) → nodes by `Parent` chain (`depth` = nesting). Callbacks `scene-row-renamed(int /*kind*/, int /*id*/, string)`, `scene-row-action(int /*kind*/, int /*id*/, string /*"delete" | "lock" | "unlock" | "toggle"*/)`; delete = `RemoveNodes`, lock = `SetLocked`, rename = `Rename`; selection stays the existing `scene-row-clicked`.

**Works when:** a placed stack shows nested under its table; rename, lock and delete from the tree work and undo.

---

## M2 done

After stage 5 merges, the Lead checks the exit criterion from the reports only: a 20-component table at 75 mm beam height built with the stack builder and drag-drop. Hardening (M5) tests it end to end.
