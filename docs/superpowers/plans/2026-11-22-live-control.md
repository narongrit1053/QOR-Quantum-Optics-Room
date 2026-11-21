# Live Control (phase 1: pipe + CLI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While LabSetup runs, an AI agent reads and edits the open project through `labsetup-cli live …`. Every edit shows live and can be undone like a hand edit.

**Architecture:** The protocol types, the pure request→`Command` resolver, the pure read answers and the Windows named-pipe server/client live in the `lab_app` library (`crates/lab-app/src/live/`, no Slint). The app binary adds `agent.rs`, which runs the pipe on a background thread, hands each request to the UI thread, applies writes through `controller::execute`, and paces them. The CLI gains a `live` subcommand that is a thin pipe client. The MCP server (phase 2) gets its own plan once this ships.

**Tech Stack:** Rust 2024, Slint, `windows-sys` 0.60 (named pipes, security), `serde`/`serde_json`, existing `lab-core`, `lab-mates`, `lab-import`, `lab-viewport::stack_chain`, `lab-optics`.

**Spec:** `docs/superpowers/specs/2026-11-22-live-control-design.md` (read it with this plan; section names below refer to it).

## Global Constraints

- Units on the wire: millimetres and degrees, always. Rotations are ZYX Euler degrees `[z, y, x]`, as in `labsetup-cli list`.
- Pipe name: `\\.\pipe\labsetup-<username>-<pid>`. Created with `PIPE_REJECT_REMOTE_CLIENTS`, DACL granting the current user's SID only.
- Off by default: `Settings.allow_agents: bool` (`#[serde(default)]`). No pipe exists while it is false.
- Framing: UTF-8 JSON, one object per line each way. Request `{"v":1,"cmd":…,"args":{…},"doc":…?}`. Response `{"ok":true,"doc":…,"result":…}` or `{"ok":false,"doc":…,"error":{"code":…,"message":…}}`.
- Error codes, exactly: `bad_request`, `not_found`, `refused`, `busy`, `doc_changed`, `disabled`.
- Undo labels of agent writes start with `Agent: ` (for example `Agent: place KM100`).
- Writes are applied no faster than one every 300 ms. Reads are never delayed.
- Badge visible while a client connected in the last 10 s.
- CLI exit codes: 0 ok, 1 error response, 2 usage or connection failure.
- `scripts/check-offline.ps1` must still pass. No new network-capable crate.
- Pipe I/O never on the UI thread.
- Test fixtures are synthetic (no Thorlabs data).

## Review Focus

1. **Stale ids after a user edit.** The user deletes node 12, then the agent sends `move 12`. Expect `not_found`, nothing changes, no panic. Test in Task 3 (`move_unknown_node_is_not_found`).
2. **The user opens another file mid-session.** The agent's next write with the old `doc` token gets `doc_changed` and nothing changes. Test in Task 8 (`write_with_old_token_is_doc_changed`).
3. **Malformed or partial input.** An empty line, invalid JSON, `"v":2`, an unknown cmd, a missing arg, a line over 1 MiB. Expect `bad_request` with a message naming the problem, and the connection stays usable. Tests in Task 1 (`rejects_bad_requests`) and Task 7 (`oversized_line_is_bad_request`).
4. **Hole labels that don't exist.** `Z99`, `c14` lower case, `A0`, an excluded or cut-out hole. Expect `not_found` for missing holes, case-insensitive parsing, and no placement. Tests in Task 2 (`parses_hole_labels`) and Task 3 (`place_in_missing_hole_is_not_found`).
5. **The client disconnects before the answer.** For example the CLI is killed during the 300 ms pacing wait. The write still applies or is dropped cleanly, the listener keeps serving, and the app does not hang. Test in Task 7 (`client_gone_before_reply_keeps_serving`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/lab-app/src/live/mod.rs` | `pub mod protocol; pub mod resolve; pub mod read; pub mod features; #[cfg(windows)] pub mod pipe;` |
| `crates/lab-app/src/live/protocol.rs` | `Request`, `Response`, `ErrorCode`, `LiveError`, line parse/serialise |
| `crates/lab-app/src/live/features.rs` | `FeatureLookup` trait, stable sorting, ids, descriptions, connector→`Feature` |
| `crates/lab-app/src/live/resolve.rs` | `resolve(&WriteRequest, &Ctx) -> Result<Resolved, LiveError>` for every write |
| `crates/lab-app/src/live/read.rs` | `read(&ReadRequest, &Ctx) -> Result<serde_json::Value, LiveError>` for every read except `snapshot` |
| `crates/lab-app/src/live/pipe.rs` | pipe server (multi-instance, secured) and client |
| `crates/lab-core/src/holes.rs` | `impl FromStr for HoleRef` |
| `crates/lab-app/src/agent.rs` (bin) | listener startup/shutdown, UI-thread dispatch, pacing queue, busy check, doc token, snapshot, badge |
| `crates/lab-app/src/main.rs`, `controller.rs:273`, `app_state.rs` | wiring, doc token bump, state fields |
| `crates/lab-file/src/settings.rs`, `crates/lab-app/src/settings_ui.rs`, `crates/lab-ui/ui/settings.slint`, `crates/lab-ui/ui/app.slint` | the setting toggle and the badge |
| `crates/lab-app/src/bin/labsetup-cli.rs` | `live` subcommand |

---

### Task 1: Protocol types

**Files:**
- Create: `crates/lab-app/src/live/mod.rs`, `crates/lab-app/src/live/protocol.rs`
- Modify: `crates/lab-app/src/lib.rs` (add `pub mod live;`), `crates/lab-app/Cargo.toml` (add `serde.workspace = true` if not already reachable)

**Interfaces:**
- Produces:
  - `pub struct Envelope { pub v: u32, pub doc: Option<String>, pub request: Request }`
  - `pub enum Request { Read(ReadRequest), Write(WriteRequest) }`
  - `pub enum ReadRequest { Status, List { platform: Option<u64> }, Platforms, Holes { platform: u64 }, Part { id: String }, Catalog { search: String, limit: Option<usize> }, Features { node: u64 }, Beams, Snapshot { path: Option<PathBuf>, width: Option<u32>, height: Option<u32> } }`
  - `pub enum WriteRequest { Place(PlaceArgs), Stack(StackArgs), Constrain(ConstrainArgs), Move { node: u64, position: Option<[f64;3]>, rotation: Option<[f64;3]> }, SetParams { node: u64, params: serde_json::Map<String, Value> }, Rename { node: u64, name: String }, Note { node: u64, text: String }, Remove { nodes: Vec<u64> }, Undo, Redo }`
  - `pub struct PlaceArgs { pub part: String, #[serde(flatten)] pub at: PlaceAt, pub name: Option<String>, pub rotate_z: Option<f64> }`. `pub enum PlaceAt { Hole { platform: u64, hole: HoleArg }, Mate { on: u64, conn: String, parent_conn: String, slide: Option<f64>, twist: Option<f64> }, Free { parent: Option<ParentArg>, position: [f64;3], rotation: Option<[f64;3]> } }`, untagged and flattened, so its keys sit flat in `args` (`{"part":…,"platform":1,"hole":"C14"}`) and the variant is told apart by which keys are present.
  - `pub enum HoleArg { Label(String), Index([i32;2]) }`, untagged. `pub enum ParentArg { Room, Platform(u64), Node(u64) }`, written as `"room"`, `{"platform":1}`, `{"node":5}`.
  - `pub struct StackArgs { pub platform: u64, pub hole: HoleArg, pub holder: String, pub post: String, pub mount: String, pub base: Option<String>, pub optic: Option<String>, pub axis_height: Option<f64>, pub slide: Option<f64>, pub rotate_z: Option<f64> }`
  - `pub struct ConstrainArgs { pub node: u64, pub to: ParentArg, pub constraints: Vec<ConstraintArg> }`. `pub struct ConstraintArg { pub child: String, pub parent: String, pub kind: Option<ConstraintKind>, pub offset: Option<f64>, pub angle: Option<f64>, pub flip: Option<bool>, pub lock_rotation: Option<bool> }` (`kind` as `"mate"|"flush"|"concentric"|"angle"`).
  - `pub enum ErrorCode { BadRequest, NotFound, Refused, Busy, DocChanged, Disabled }`, serialised snake_case.
  - `pub struct LiveError { pub code: ErrorCode, pub message: String }` plus `LiveError::new(code, impl Into<String>)`.
  - `pub fn parse_line(line: &str) -> Result<Envelope, LiveError>`
  - `pub fn ok_line(doc: &str, result: Value) -> String` and `pub fn err_line(doc: &str, e: &LiveError) -> String`, each ending in `\n`.
  - `pub const MAX_LINE: usize = 1 << 20;`

- [ ] **Step 1: Write failing tests** in `protocol.rs` `mod tests`:
  - `parses_every_command`: one JSON line per command above round-trips into the expected variant. Includes `place` in all three `PlaceAt` forms, `hole:"C14"` and `hole:[13,2]`, `to:"room"`/`{"platform":1}`/`{"node":5}`, and a constraint with `kind` left out.
  - `rejects_bad_requests`: `""`, `"{"`, `{"v":2,"cmd":"status"}`, `{"v":1,"cmd":"frobnicate"}`, `{"v":1,"cmd":"rename","args":{"node":1}}` each give `ErrorCode::BadRequest`, and the message contains `"v"`, `"frobnicate"` or `"name"` respectively where applicable.
  - `response_lines`: `ok_line("d1", json!({"x":1}))` == `"{\"ok\":true,\"doc\":\"d1\",\"result\":{\"x\":1}}\n"`. `err_line` of `NotFound` contains `"code":"not_found"`.
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::protocol`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement** with serde: parse into `serde_json::Value` first, check `v == 1`, then deserialize `{cmd, args}` with `#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]` on an internal enum that is split into `Read`/`Write`. Map serde errors to `BadRequest` with the serde message.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "live: protocol types and line parsing"`.

### Task 2: Hole label parsing

**Files:** Modify `crates/lab-core/src/holes.rs`

**Interfaces:**
- Produces: `impl std::str::FromStr for HoleRef { type Err = String; }`. It inverts `Display`: row letters (spreadsheet-style, case-insensitive) then a 1-based column number.

- [ ] **Step 1: Write the failing test** `parses_hole_labels`: `"A1"`→`{col:0,row:0}`, `"C14"`→`{col:13,row:2}`, `"c14"`→same, `"AA3"`→`{col:2,row:26}`. Round-trip: for row in 0..60 and col in [0, 47], `HoleRef{col,row}.to_string().parse() == Ok(..)`. Errors: `""`, `"14"`, `"C"`, `"C0"`, `"C-1"`, `"1C"`.
- [ ] **Step 2: Run** `cargo test -p lab-core parses_hole_labels`. Expected: FAIL.
- [ ] **Step 3: Implement `FromStr for HoleRef`.**
- [ ] **Step 4: Run** it again. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "lab-core: parse hole labels"`.

### Task 3: Resolve simple writes (place, move, rename, note, set_params, remove)

**Files:** Create `crates/lab-app/src/live/resolve.rs`.

**Interfaces:**
- Consumes: Task 1 types, Task 2 `HoleRef: FromStr`.
- Produces:
  - `pub struct Ctx<'a> { pub doc: &'a Document, pub catalog: &'a Catalog, pub holes: &'a dyn Fn(PlatformId) -> Option<Rc<HoleSet>>, pub features: &'a dyn FeatureLookup, pub trace: &'a lab_optics::Trace }` (`FeatureLookup` is from Task 5; until then declare the trait in `features.rs` with just its signature).
  - `pub struct Resolved { pub command: Command, pub label: String, pub select: Option<SelectionTarget> }`
  - `pub fn resolve(req: &WriteRequest, ctx: &Ctx) -> Result<Resolved, LiveError>`. `Undo` and `Redo` are not resolved (the app handles them). They return `BadRequest`.
  - `pub(crate) fn new_part_node(catalog: &Catalog, part: &str, id: u64, parent: Parent, attach: Attach, position: DVec3, rotation: DQuat) -> Option<Node>`, **moved** from `place.rs::new_node` into the library so both use it. `place.rs` calls `lab_app::live::resolve::new_part_node`. Its behaviour is unchanged.
  - `pub fn euler_deg_to_quat([z,y,x]: [f64;3]) -> DQuat` and `pub fn quat_to_euler_deg(DQuat) -> [f64;3]` (ZYX).
- Rules:
  - New ids come from `doc.next_free_id()`. Labels: `Agent: place <part name>`, `Agent: move <node name>`, `Agent: rename <old> to <new>`, `Agent: note on <name>`, `Agent: set parameters of <name>`, `Agent: remove <n> parts`.
  - `place` Hole: parse the label (or take the index), look the hole up with `HoleSet::hole_at` (missing → `NotFound "no hole C14 on platform <name>"`). The node gets `parent: Platform`, `attach: Attach::Hole { hole: <LatticeIndex> }`, position `(x, y, 0)` and rotation `rotate_z` about +Z. Check this against how the viewport builds a hole drop (`PlaceRequest` for `Attach::Hole`), so a hand drop and an agent place produce equal nodes.
  - `place` Mate: `on` must exist. Its part must have `parent_conn`, and the new part must have `conn`, each `NotFound` otherwise. They must be `lab_mates::compatible` (`Refused` otherwise). Pose from `lab_mates::mate_pose(parent_frame, child_frame, slide, twist_rad)`, `Attach::Mate { … }`, `parent: Node(on)`.
  - `place` Free: `Attach::Free`, the parent given or `Room`.
  - `move`: `SetPlacement`. `Refused "node <name> is attached; detach it first"` unless `attach` is `Free`, mirroring what the UI allows. Read the transform code in `transform.rs` and match its rule.
  - `set_params`: JSON numbers → `ParamValue::Number`, strings → the matching `ParamValue` variant, merged over existing `node.params`.
  - Every command is validated with `command.clone().apply(&mut ctx.doc.clone())` before it is returned. An `Err` becomes `Refused` with the `CommandError` text. This is also how the app gets refusal text without changing `controller::execute`.

- [ ] **Step 1: Write failing tests** in `resolve.rs` using `lab_core::sample::demo_document()` and the default built-in catalog (`Catalog::load_default` or the built-in-only constructor the catalog tests use). Use a stub `FeatureLookup` that returns `vec![]`:
  - `place_in_hole_matches_hole_position`: place `generic:post-holder` (or whichever built-in has `HoleGridSeat`) at `C14` on the demo breadboard. The `AddNode` node has `Attach::Hole`, `position.truncate() == holes.hole_at(C14)`, and the label `"Agent: place …"`.
  - `place_in_missing_hole_is_not_found`: `Z99` → `NotFound`.
  - `place_mate_uses_mate_pose`: mate a post's `bottom` into an existing holder's `bore` with slide 10. The pose equals `mate_pose(...)` within 1e-9.
  - `place_mate_incompatible_is_refused`.
  - `move_unknown_node_is_not_found`: `move 9999` → `NotFound`.
  - `move_attached_node_is_refused`.
  - `rename_and_remove_labels`.
  - `euler_round_trip`: for a few angle triples, `quat_to_euler_deg(euler_deg_to_quat(a)) ≈ a`.
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::resolve`. Expected: FAIL.
- [ ] **Step 3: Implement** `resolve` for these variants and move `new_node`.
- [ ] **Step 4: Run** the tests plus `cargo test -p lab-app` (place.rs still compiles and passes). Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "live: resolve place, move, rename, note, params, remove"`.

### Task 4: Resolve `stack`

**Files:** Modify `crates/lab-app/src/live/resolve.rs`.

**Interfaces:**
- Consumes: `lab_viewport::{StackSpec, stack_chain}`, Task 3 helpers.
- Produces: `pub fn slide_for_axis_height(catalog: &Catalog, spec_without_slide: &StackSpec, axis_height: f64) -> Result<f64, LiveError>`.
- Rules:
  - The root is the holder (or `base` if given) in the hole, as in Task 3's hole placement. The rest comes from `stack_chain`, ids consecutive from `next_free_id()`, one `Command::PlaceAssembly`, select the root, label `Agent: stack <mount name> at C14`.
  - `axis_height`: optical axis height above the platform top = the height of the topmost part's axis frame (the optic's `edge` frame origin if `optic`, else the mount's `optic_seat`) computed by `stack_chain` with slide 0, plus the slide. So `slide = axis_height - height_at_slide_0`. Refused with `"beam height X mm is out of reach for this post (Y–Z mm)"` if the slide falls outside the holder `bore` connector's `range`.
  - Exactly one of `axis_height` and `slide` is required (`BadRequest` otherwise).

- [ ] **Step 1: Write failing tests:**
  - `stack_matches_library_chain`: the `PlaceAssembly` nodes after the root equal `stack_chain`'s output for the same spec (part, attach, pose).
  - `stack_axis_height_lands_at_height`: with `axis_height: 75.0`, the world z of the top optic axis (via `lab_mates::constraints::world_pose` on a doc with the command applied) is 75.0 ± 1e-6 above the platform top.
  - `stack_height_out_of_range_is_refused`: `axis_height: 1000.0` → `Refused` naming the range.
  - `stack_needs_one_of_height_or_slide`.
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::resolve::tests::stack`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "live: resolve stack with beam height"`.

### Task 5: Features and `constrain`

**Files:** Create `crates/lab-app/src/live/features.rs`. Modify `resolve.rs`.

**Interfaces:**
- Produces:
  - `pub trait FeatureLookup { fn detected(&self, node: &Node, def: &PartDef) -> Vec<Feature>; }`, the raw detected features in any order.
  - `pub struct NamedFeature { pub id: String, pub feature: Feature, pub description: String, pub source: &'static str /* "connector" | "detected" */ }`
  - `pub fn node_features(node: &Node, def: &PartDef, lookup: &dyn FeatureLookup) -> Vec<NamedFeature>`: connectors first (id = connector name), then detected sorted and numbered `f1…`.
  - `pub fn platform_features(platform: &Platform, holes: &HoleSet) -> impl Fn(&str) -> Option<Feature>`. `"top"` → `Plane { origin: 0, normal: +Z }`, `"hole:C14"` → `Axis { origin: (x,y,0), dir: +Z, radius: holes.thread_radius, hole: true }`.
  - `pub fn connector_feature(c: &ConnectorDef) -> Feature`. `Bore`/`OpticBore`/`Thread`/`LensTube`/`CageRod` → `Axis { origin: frame.origin, dir: frame.z, radius: diameter/2 (thread: from the spec's nominal size; cage rod: 3.0), hole: gender == Female }`. `HoleGridSeat` and anything else → `Plane { origin: frame.origin, normal: frame.z }`.
- Sorting key for detected features (designer's call): planes before axes, then origin `z` descending, `y` ascending, `x` ascending (each rounded to 0.01 mm), then radius ascending.
- Description format, exactly:
  - plane: `plane, normal +Z, through (0.00, 0.00, 12.50)`
  - axis: `hole axis Ø6.00 along +Z, at (0.00, 0.00, 12.50)` / `shaft axis …`
  - Axis-aligned directions within 1e-6 print as `±X/±Y/±Z`. Others print as `(0.71, 0.00, 0.71)`.
- `resolve` `Constrain`: map each `ConstraintArg` id to a `Feature` (child from `node_features` of `node`, parent from `node_features` of the `to` node or from `platform_features`). An unknown id → `NotFound "node 7 has no feature 'f9'; use features to list them"`. A missing `kind` → `lab_mates::constraints::default_kind(child, parent)`, and `Refused "these features can't be constrained to each other"` if that's `None`. Offset is mm, angle degrees→radians. Then `lab_mates::constraints::constrain(doc, id, parent, constraints)`. Label `Agent: constrain <child name> to <parent name>`, select the node.
- In the app (Task 8), `FeatureLookup` uses `def.features` when non-empty, else `lab_import::features::detect(&constrain::part_mesh(def, node)?)`, cached by `(part id, serde_json of node.params)`.

- [ ] **Step 1: Write failing tests:**
  - `features_are_stable_and_described`: a stub lookup returns three features in a shuffled order. `node_features` gives connectors first, then `f1..f3` in the documented order, with descriptions matching the exact format above.
  - `connector_features_map_kinds`: a `Bore` female → `Axis{hole:true, radius: d/2}`, `HoleGridSeat` → `Plane`.
  - `constrain_with_connector_ids`: constrain a free post's `bottom` concentric to a holder's `bore`. The result is `SetAttach` with `Attach::Constrained` holding one `Concentric`, and it equals `lab_mates::constraints::constrain` called directly.
  - `constrain_default_kind`: `kind` left out → equals `default_kind`.
  - `constrain_to_platform_hole`: parent id `hole:C14` on the platform.
  - `constrain_unknown_feature_is_not_found`.
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::features live::resolve::tests::constrain`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "live: feature ids and constrain"`.

### Task 6: Read answers

**Files:** Create `crates/lab-app/src/live/read.rs`.

**Interfaces:**
- Consumes: `Ctx` (Task 3), `node_features` (Task 5).
- Produces: `pub fn read(req: &ReadRequest, ctx: &Ctx, status: &StatusInfo) -> Result<Value, LiveError>`, where `pub struct StatusInfo { pub version: String, pub title: String, pub path: Option<PathBuf>, pub dirty: bool, pub undo_label: Option<String> }`. `Snapshot` returns `BadRequest` here (the app handles it).
- Result shapes (field names exact):
  - `status`: `{version, title, path, dirty, undo}`. The doc token is in the envelope.
  - `list`: `[{id, name, part, vendor, part_number, parent, attach, position, rotation, locked}]`. `parent` like `ParentArg` JSON. `attach` is one of `"free"|"hole"|"mate"|"constrained"|"joint"`. Filtered by `platform` (root platform) when given.
  - `platforms`: `[{id, name, kind, top_height, bounds:[[x0,y0],[x1,y1]], hole_grid: bool}]`
  - `holes`: `{pitch:[x,y], first:"A1", last:"…", thread_radius, occupied:[{hole:"C14", node: id}]}`, occupied = nodes with `Attach::Hole` on that platform.
  - `part`: `{id, name, vendor, part_number, category, connectors:[{name, kind, frame:{origin, z, x}}]}`. `NotFound` if not in the catalog.
  - `catalog`: `[{id, vendor, part_number, name}]`, `limit` default 50.
  - `features`: `[{id, source, description, feature}]`, where `feature` is the serde of `Feature`.
  - `beams`: `[{start, end, hits: node id or null, radius}]` from `ctx.trace.segments`. Read `lab_optics::Segment` for the field names to map.

- [ ] **Step 1: Write failing tests** on the demo document: `list_has_every_node`, `list_filters_by_platform`, `holes_reports_occupied`, `part_unknown_is_not_found`, `catalog_respects_limit`, `features_shape`, `beams_shape` (use a doc that contains a laser, from the sample or built in the test).
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::read`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "live: read answers"`.

### Task 7: Named-pipe server and client

**Files:** Create `crates/lab-app/src/live/pipe.rs` (`#[cfg(windows)]`). Modify `crates/lab-app/Cargo.toml` to add the `windows-sys` features `Win32_System_Pipes`, `Win32_Storage_FileSystem`, `Win32_Security`, `Win32_Security_Authorization`, `Win32_System_Threading`, `Win32_System_IO`.

**Interfaces:**
- Produces:
  - `pub fn pipe_name(pid: u32) -> String`, which gives `\\.\pipe\labsetup-<username>-<pid>`. Username from `GetUserNameW`, with characters outside `[A-Za-z0-9_.-]` replaced by `_`.
  - `pub struct Server { … }`. `pub fn Server::start(name: &str, handle: impl Fn(String) -> Receiver<String> + Send + Sync + 'static) -> io::Result<Server>`. Each line read is passed to `handle`, which returns a channel the reply arrives on (the reply is written back as is). `Server::stop(self)` closes every instance and joins the threads. `Drop` calls stop.
  - `pub fn Server::last_seen(&self) -> Option<Instant>` (time of the last request, for the badge).
  - `pub fn find_pipes() -> Vec<(u32 /*pid*/, String /*name*/)>` lists `\\.\pipe\` entries matching `labsetup-<username>-*` (`FindFirstFileW` on `\\.\pipe\*`).
  - `pub fn call(name: &str, line: &str, timeout: Duration) -> io::Result<String>`. It connects (`WaitNamedPipeW` up to `timeout`), writes the line, reads one line back.
- Rules:
  - Up to 4 instances at once (`nMaxInstances = 4`), each served on its own thread. After a client disconnects, the instance goes back to waiting.
  - Security descriptor: an owner-only DACL built from the process token's user SID (`ConvertStringSecurityDescriptorToSecurityDescriptorW` with `D:P(A;;GA;;;<sid>)` is acceptable), plus `PIPE_REJECT_REMOTE_CLIENTS`.
  - A line longer than `MAX_LINE` → write an `err_line("", bad_request "request too long")` and close that connection.
  - Client gone before the reply: the write fails, the error is ignored, and the instance goes back to waiting.

- [ ] **Step 1: Write failing tests** (`#[cfg(windows)]`, each with a unique pipe name built from `pipe_name(std::process::id())` plus a test suffix):
  - `echo_round_trip`: start a server whose handler echoes, `call` gets the same line back.
  - `two_clients_at_once`: one client holds a connection open while a second `call` succeeds.
  - `oversized_line_is_bad_request`.
  - `client_gone_before_reply_keeps_serving`: the handler sleeps 200 ms, the first client disconnects right after writing, and a second `call` still succeeds.
  - `pipe_dacl_is_owner_only`: `GetSecurityInfo` on the server handle shows exactly one ACE for the current user's SID.
- [ ] **Step 2: Run** `cargo test -p lab-app --lib live::pipe`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS. Then `pwsh scripts/check-offline.ps1` passes.
- [ ] **Step 5: Commit** `git commit -m "live: secured named-pipe server and client"`.

### Task 8: App wiring: setting, dispatch, pacing, doc token, snapshot

**Files:**
- Create: `crates/lab-app/src/agent.rs`
- Modify: `crates/lab-file/src/settings.rs` (`#[serde(default)] pub allow_agents: bool`), `crates/lab-app/src/app_state.rs` (fields below), `crates/lab-app/src/controller.rs:273` (bump the token where the session is replaced), `crates/lab-app/src/main.rs` (`mod agent;` and `agent::connect(&ui, &app, &models)` after `controller::connect`), `crates/lab-app/src/settings_ui.rs`, `crates/lab-ui/ui/settings.slint`, `crates/lab-ui/ui/app.slint` (settings `apply` gains a 5th `bool` argument, `allow-agents`, shown as a switch labelled "Allow AI agents to control LabSetup" with the hint "Agents on this PC can read and edit the open project through labsetup-cli.")

**Interfaces:**
- Consumes: Tasks 1, 3–7.
- Produces:
  - `AppState` fields: `pub doc_token: u64`, `pub agent: Option<agent::Agent>`.
  - `pub fn agent::connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>, models: &Rc<UiModels>)`, which starts the server if `settings.allow_agents`.
  - `pub fn agent::set_enabled(ui, app, models, on: bool)`, called from `settings_ui` when the toggle changes. It starts or stops the server.
  - `pub fn agent::is_busy(a: &AppState) -> bool`: `a.busy || a.sheet.is_some() || a.pending_action.is_some()` plus the constrain and joint panels being open and a viewport drag being in progress. Find those flags in `constrain.rs`, `joint.rs` and `bridge.rs`, and add an `AppState` flag if a state is only known on the viewport side.
- Rules:
  - The server handler sends each line to the UI thread with `slint::invoke_from_event_loop`. There, `parse_line`, then:
    - Read → answer at once with `read(...)`. `Snapshot` → `export_png::render_to(app, path or %TEMP%\labsetup-agent-<n>.png, width or 1280, height or 800, done)`, reply `{path}` from `done(true)`, or `refused "the view could not be rendered"` on false.
    - Write → if `doc` is given and `!= doc_token.to_string()` → `doc_changed`. If `is_busy` → `busy "LabSetup is in the middle of an edit; retry shortly"`. Otherwise push onto a FIFO queue drained by a `slint::Timer`, at most one write per 300 ms. Each drained write is checked again (token and busy), resolved, and applied with `controller::execute(ui, app, models, "live", command, label, select, vec![])`. Reply `{changed: [ids], poses: [{id, position, rotation}], undo: label}` from the `ChangeSet` (added + updated nodes, poses read from the doc after apply). `Undo`/`Redo` go through the same path the Ctrl+Z / Ctrl+Y actions use.
  - The token starts at 1 and is bumped at `controller.rs:273` (the one place `a.session` is replaced). Responses carry `doc_token.to_string()`.
  - When disabled: no server. A request racing a disable gets `disabled`.
- [ ] **Step 1: Write failing tests** in `agent.rs` for the pure parts, factored as `fn admit(env: &Envelope, token: u64, busy: bool) -> Result<(), LiveError>`: `write_with_old_token_is_doc_changed`, `write_while_busy_is_busy`, `read_ignores_busy`, `write_without_token_is_admitted`. Pacing is factored as `struct Pacer { last: Option<Instant> }`, `fn ready(&mut self, now: Instant) -> bool` (300 ms), with test `pacer_spaces_writes_300ms`.
- [ ] **Step 2: Run** `cargo test -p lab-app --bin labsetup agent`. Expected: FAIL.
- [ ] **Step 3: Implement**, including the settings toggle and `Settings.allow_agents`.
- [ ] **Step 4: Run** the tests, then `cargo build -p lab-app --release` (incremental, about 30 s). Expected: PASS, build ok.
- [ ] **Step 5: Commit** `git commit -m "live: app dispatch, pacing, doc token, settings toggle"`.

### Task 9: The "Agent connected" badge

**Files:** Modify `crates/lab-ui/ui/app.slint` (next to `document-note`, line ~1033) and `crates/lab-app/src/agent.rs`.

**Interfaces:**
- Produces: in `app.slint`, `in property <string> agent-status;` (empty = hidden) and `callback agent-badge-clicked();`. The badge is a small pill using `Theme` tokens and the existing chip style, text `Agent: <last step label without the "Agent: " prefix>`, or `Agent connected` before any write.
- Rules: `agent.rs` sets `agent-status` on every request and runs a 1 s `slint::Timer` that clears it once `Server::last_seen()` is older than 10 s. Clicking opens the Settings sheet (`settings-open = true`).
- [ ] **Step 1: Run the app** with the setting on. `labsetup-cli live status` shows the badge, and it disappears about 10 s later. A write shows its label. Check the window by capturing it with PrintWindow, as in the dev-environment notes.
- [ ] **Step 2: Commit** `git commit -m "live: agent connected badge"`.

### Task 10: CLI `live` subcommand, changelog, end-to-end check

**Files:** Modify `crates/lab-app/src/bin/labsetup-cli.rs` and `CHANGELOG.md`.

**Interfaces:**
- Consumes: `live::pipe::{find_pipes, call}`, `live::protocol`.
- Produces: `labsetup-cli live <cmd> [--pid N] [--doc TOKEN] [--json ARGS] [flags]`.
  - Flags map to `args` keys one-to-one with `-` → `_` (`--axis-height 75` → `axis_height: 75`). Values that parse as JSON numbers or arrays (`--position "[10,0,0]"`) are sent as such, everything else as strings. `--json` is merged over the flags.
  - Pipe choice: `--pid` if given, else the only match of `find_pipes()`. With none: exit 2, `"LabSetup isn't running, or 'Allow AI agents' is off in Settings."` With several: exit 2 and list `pid` per line.
  - Prints the response line as is. Exit 0 if `"ok":true`, else 1. Connection failure: exit 2.
  - `labsetup-cli live --help` prints every command with one example each, as in the spec's command tables, and a first paragraph for agents: "Read `status` first and pass its doc token with --doc on writes. Use `features <node>` before `constrain`. Units are mm and degrees."
- [ ] **Step 1: Write failing tests** in the CLI's `mod tests`:
  - `parses_live_flags`: `live place --part thorlabs:KM100 --platform 1 --hole C14 --rotate-z 90` → envelope args `{"part":"thorlabs:KM100","platform":1,"hole":"C14","rotate_z":90}`. (flat, matching Task 1's flattened `PlaceArgs`).
  - `json_overrides_flags`, and `live` with no command → usage error.
- [ ] **Step 2: Run** `cargo test -p lab-app --bin labsetup-cli`. Expected: FAIL.
- [ ] **Step 3: Implement**, and add a `v0.0.8`-style entry at the top of `CHANGELOG.md`: "**AI agents can build your setup.** Turn on *Allow AI agents to control LabSetup* in Settings, and an agent on this PC can place parts, stack posts and mounts at a beam height, and add constraints through `labsetup-cli live`. You watch each step appear, and Ctrl+Z undoes it."
- [ ] **Step 4: Run** the tests. Expected: PASS.
- [ ] **Step 5: End-to-end, by hand,** with a release build and the setting on:
  - `status`, then `stack` a mirror mount at `C14` at 75 mm and a second one at `C20`, `place` a laser, `beams`, `snapshot`.
  - Each step appears about 300 ms apart and is selected. Ctrl+Z undoes them one at a time. `beams` reports the path.
  - Open a different file, then a write with the old `--doc` → `doc_changed`.
  - Turn the setting off → `labsetup-cli live status` exits 2 with the message above.
- [ ] **Step 6: Commit** `git commit -m "live: labsetup-cli live subcommand"`.
