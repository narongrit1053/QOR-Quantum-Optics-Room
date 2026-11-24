# Multi-select, Groups and Bundles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ctrl/Shift multi-select in the viewport and scene tree, saved groups that show as folders and move as one rigid block, and Save as Bundle from a whole selection or group.

**Architecture:** Selection keeps its single primary (`Session::selection`) plus an ordered `Session::extra: Vec<NodeId>`; only multi-aware actions read the extras. Groups are bookkeeping in `Document::groups` (members are chain roots; real parents/attachments are untouched) with four undoable commands. Block moves and deletes are `Command::Batch`es so one undo restores everything.

**Tech Stack:** Rust 2024, Slint UI (`crates/lab-ui/ui/*.slint`), Bevy viewport (`crates/lab-viewport`), serde JSON project files.

**Spec:** `docs/superpowers/specs/2026-11-23-multi-select-groups-bundles-design.md`

## Global Constraints

- No change to how positions, mates, constraints or joints are solved (`lab-mates` untouched).
- No `SCHEMA_VERSION` bump; new document/settings fields are `#[serde(default)]`. Old files must load.
- Group ids come from `Document::next_free_id()` and stay `<= MAX_ID`.
- `views::set_root` keeps its mate-only meaning; new code uses `Document::chain_root`.
- Every user edit is one undo step; on any refusal nothing moves partly.
- Selection stays non-undoable UI state.
- Build/test env (Bash): `export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH`. Incremental builds ~30 s; run long ones in the background.
- After code changes: `graphify update .`

## Review Focus

1. **Ctrl+clicking the primary off** — the last extra must become the primary and the gizmo/inspector follow it (Task 4 test `toggle_off_primary_promotes_last_extra`).
2. **Deleting a group member then undo** — group membership must come back exactly (Task 2 test `remove_member_then_undo_restores_group`).
3. **Block move where one member would leave the plate** — nothing moves, toast shown (Task 7 test `block_step_off_plate_refuses_all`).
4. **Saving a bundle by selecting only the bottom/top-most constrained part** — all 5 parts saved, none twice (Task 3 test `copy_constrained_child_before_root_no_duplicates`).
5. **Opening a project saved before this feature** — loads with no groups, saves without error (Task 1 test `old_document_without_groups_loads`).

---

### Task 1: Group model and chain root (lab-core)

**Files:**
- Modify: `crates/lab-core/src/model.rs` (Document struct ~L29, `contains_id` ~L189, add types near `NodeId`)
- Modify: `crates/lab-core/src/lib.rs` (re-export `Group`, `GroupId`)
- Test: `crates/lab-core/src/model.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub struct GroupId(pub u64)` — same derives as `NodeId` (incl. `Serialize, Deserialize, Hash, Ord`).
  - `pub struct Group { pub id: GroupId, pub name: String, pub members: Vec<NodeId> }` — `Clone, Debug, PartialEq, Serialize, Deserialize`.
  - `Document.groups: Vec<Group>` with `#[serde(default)]`; `Document::new` sets it empty.
  - `Document::group(&self, id: GroupId) -> Option<&Group>`
  - `Document::group_of(&self, node: NodeId) -> Option<&Group>` (group whose `members` contains `node`)
  - `Document::chain_root(&self, id: NodeId) -> NodeId` — walks up every `Parent::Node(p)` link (any `Attach`) until the parent is a platform or the room; cycle-guarded by node count.
  - `Document::top_parent(&self, id: NodeId) -> Parent` — the `parent` of `chain_root(id)`.
  - `contains_id` also returns true for group ids.

- [ ] **Step 1: Write failing tests**
  - `old_document_without_groups_loads`: serialize a `Document`, strip the `"groups"` key from the JSON value, deserialize → `doc.groups.is_empty()`.
  - `chain_root_walks_mate_constrained_and_joint`: nodes 1 (on platform/room, Free), 2 `Parent::Node(1)` Mate, 3 `Parent::Node(2)` Constrained, 4 `Parent::Node(3)` Joint → `chain_root(NodeId(4)) == NodeId(1)`, `chain_root(NodeId(1)) == NodeId(1)`, `top_parent(NodeId(4)) == node1.parent`.
  - `contains_id_sees_groups`: push `Group { id: GroupId(77), .. }` → `doc.contains_id(77)`.
- [ ] **Step 2:** `cargo test -p lab-core chain_root old_document contains_id_sees` → FAIL (not defined).
- [ ] **Step 3:** Implement the items above.
- [ ] **Step 4:** `cargo test -p lab-core` → all PASS (existing tests constructing `Document` literally need `groups: Vec::new()`; `Document` has a private field so most use `Document::new`).
- [ ] **Step 5:** Commit `lab-core: Group model and Document::chain_root`.

### Task 2: Group commands and group-aware delete (lab-core)

**Files:**
- Modify: `crates/lab-core/src/command.rs` (enum ~L18, `apply` ~L307, `label` ~L600, tests)
- Modify: `crates/lab-core/src/lib.rs` (export `remove_nodes_command`)
- Modify callers of `Command::RemoveNodes` for user deletes: `crates/lab-app/src/edit.rs:925`, `crates/lab-app/src/scene.rs:59`

**Interfaces:**
- Consumes: Task 1 types and `chain_root`, `top_parent`, `group_of`.
- Produces (new `Command` variants):
  - `AddGroup { group: Group, index: Option<usize> }` → inverse `RemoveGroup(id)`
  - `RemoveGroup(GroupId)` → inverse `AddGroup { group, index: Some(old_index) }`
  - `SetGroupMembers { id: GroupId, members: Vec<NodeId> }` → inverse with old members
  - `RenameGroup { id: GroupId, name: String }` → inverse with old name
  - Validation in `AddGroup`/`SetGroupMembers` (new `CommandError` variants, names your choice): every member exists, `chain_root(m) == m`, all share one `top_parent`, no member is in another group, `members.len() >= 2`, no duplicates. `AddGroup` also runs `check_new_id` and `bump_next_id`.
  - Group commands return `ChangeSet::default()` plus a new `pub groups_changed: bool` field set to true (scene tree refreshes on it; `is_empty()` must treat `groups_changed` as non-empty).
  - Labels: "Group {name}", "Ungroup {name}", "Edit Group {name}", "Rename Group".
  - `pub fn remove_nodes_command(doc: &Document, ids: Vec<NodeId>) -> Command` — plain `RemoveNodes(ids)` when no group is affected; otherwise `Batch([RemoveNodes(ids), then per affected group: SetGroupMembers with removed members dropped, or RemoveGroup when < 2 remain])`. A node counts as removed if it or an ancestor is in `ids`.

- [ ] **Step 1: Write failing tests** (use the file's existing `round_trip` / `assert_rejected` helpers)
  - `group_commands_round_trip`: `round_trip` for `AddGroup`, then on a doc with a group: `RemoveGroup`, `SetGroupMembers`, `RenameGroup`.
  - `add_group_rejects_non_root_and_cross_platform`: member that is a `Parent::Node` child → Err; members on two platforms → Err; one member → Err.
  - `remove_member_then_undo_restores_group`: group {1,2,3}; `History::execute(remove_nodes_command(&doc, vec![1]))` → group members `[2,3]`; undo → `[1,2,3]` and doc equals original.
  - `remove_member_below_two_drops_group`: group {1,2}; remove 1 → `doc.groups.is_empty()`; undo → group back.
  - `remove_chain_child_keeps_group`: group {1,5}, node 2 child of 1; remove 2 → group unchanged and command is plain `RemoveNodes`.
- [ ] **Step 2:** `cargo test -p lab-core group remove_member remove_chain` → FAIL.
- [ ] **Step 3:** Implement variants, validation, labels, `remove_nodes_command`; switch `edit.rs` delete and `scene.rs` row delete to `remove_nodes_command`.
- [ ] **Step 4:** `cargo test -p lab-core && cargo test -p lab-app edit` → PASS (update `edit.rs:2213` expectation only if it now builds a Batch — it should not, no groups there).
- [ ] **Step 5:** Commit `lab-core: undoable group commands; deletes keep groups consistent`.

### Task 3: Copy fix and bundles from a selection (lab-app, lab-file)

**Files:**
- Modify: `crates/lab-app/src/clipboard.rs:11-25` (`copy`)
- Modify: `crates/lab-file/src/settings.rs:75` (`Bundle`)
- Modify: `crates/lab-app/src/library.rs:334-346` (`save_bundle_prompt`), `:406-417` (`bundle_save`)
- Modify: `crates/lab-app/src/place.rs:73-87` (`place_bundle`)

**Interfaces:**
- Consumes: `Document::chain_root`, `Session::selected_nodes()` (Task 4 — if Task 4 is not done yet, take `&[NodeId]` and pass `&[primary]`), `Command::AddGroup`.
- Produces:
  - `copy(doc, selection)` uses `doc.chain_root(id)` and a seen-set of node ids; never emits a node twice; order = selection order, parents first within each chain.
  - `Bundle { name, nodes, #[serde(default)] group: Option<String> }`.
  - `bundle_save` saves `bundle_nodes(copy(doc, &selected_nodes))`; `group` = `Some(group.name)` when the selection is exactly one whole group. Default prompt name: that group's name, else the primary's name (as today).
  - `place_bundle` (place.rs): when `bundle.group` is `Some(name)` and ≥ 2 roots were placed, executes `Command::Batch([PlaceAssembly{..}, AddGroup{ Group{ id: next id after the placed nodes, name, members: placed roots }, index: None }])`, else unchanged.

- [ ] **Step 1: Write failing tests** (clipboard.rs tests)
  - `copy_constrained_child_before_root_no_duplicates`: chain 1←2(Constrained)←3(Constrained) plus unrelated root 9; `copy(&doc, &[NodeId(3), NodeId(1), NodeId(9)])` → ids `[1,2,3,9]` exactly.
  - `bundle_of_whole_chain_from_bottom_part`: 5-node chain (Hole root, then Mate, Constrained, Constrained, Constrained); `copy(&doc, &[bottom-most leaf])` → 5 nodes.
  - settings.rs: extend `old_settings_load_and_libraries_round_trip` so a bundle JSON without `group` loads with `group == None`.
- [ ] **Step 2:** `cargo test -p lab-app clipboard && cargo test -p lab-file` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same commands → PASS.
- [ ] **Step 5:** Commit `Bundles: save whole selection and constrained chains; groups round-trip through bundles`.

### Task 4: Selection model with extras (lab-app, pure logic)

**Files:**
- Create: `crates/lab-app/src/multi_select.rs` (pure functions + tests); register in `main.rs`/`lib.rs` like sibling modules
- Modify: `crates/lab-app/src/session.rs` (`Session.extra`, constructors, `session.rs:100` clear)

**Interfaces:**
- Produces:
  - `Session.extra: Vec<NodeId>` (empty in all constructors; cleared wherever `selection` is reset to `None` at `session.rs:100`).
  - `Session::selected_nodes(&self) -> Vec<NodeId>` — primary (if a Node) then extras.
  - `pub struct Sel { pub primary: Option<SelectionTarget>, pub extra: Vec<NodeId> }`
  - `pub fn click(doc: &Document, prev: &Sel, hit: Option<SelectionTarget>) -> Sel` — plain click: `None` → empty; platform/room → that alone; node in a group → if `prev` is exactly that whole group (same member set) select `hit` alone, else primary = `chain_root(hit)`, extra = other members; else `hit` alone.
  - `pub fn toggle(prev: &Sel, node: NodeId) -> Sel` — Ctrl+click: add if absent; remove if present; removing the primary promotes the last extra; a platform/room primary is dropped when a node is toggled in.
  - `pub fn range(rows: &[(i32, i32)], prev: &Sel, node: NodeId) -> Sel` — Shift+click in the tree: `rows` are visible `(kind,id)`; select all node rows (kind 0) between the primary's row and the clicked row inclusive; primary unchanged (or = node if none).
  - `pub fn is_whole_group(doc: &Document, sel: &Sel) -> Option<GroupId>`

- [ ] **Step 1: Write failing tests** in `multi_select.rs`
  - `toggle_adds_and_removes`, `toggle_off_primary_promotes_last_extra` (sel 1+[2,3], toggle 1 → primary 3, extra [2]).
  - `click_group_member_selects_whole_group` then `click_again_selects_member_alone`.
  - `range_selects_visible_nodes_between` (rows `[(2,0),(1,1),(0,5),(0,6),(0,7)]`, primary 5, shift-click 7 → extra [6,7]).
- [ ] **Step 2:** `cargo test -p lab-app multi_select` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** → PASS.
- [ ] **Step 5:** Commit `Selection: primary plus extras, click/toggle/range rules`.

### Task 5: Viewport — highlight extras, report Ctrl+click (lab-viewport)

**Files:**
- Modify: `crates/lab-viewport/src/lib.rs` (`ViewportRequest` ~L135, `ViewportEvent` ~L290, `FrameRequest` ~L369)
- Modify: `crates/lab-viewport/src/picking.rs:37-137` (`Selection`, `pick`, `apply_highlight`)

**Interfaces:**
- Produces:
  - `ViewportRequest::SelectExtra(Vec<NodeId>)` (+ matching `FrameRequest`) — sets `Selection.extra`.
  - `Selection.extra: Vec<NodeId>`; `apply_highlight` treats a look as selected if its owner is `selected` **or** a `Node` in `extra`.
  - `ViewportEvent::SelectionToggled(SelectionTarget)` — sent for a left click with `ctrl` held on a hit; `selected` is not changed locally.
  - When `extra` is non-empty, a plain click on a hit that is the primary or in `extra` sends `SelectionChanged(Some(hit))` even if unchanged (no "click again clears").
  - Shift keeps its meaning (ray through select-locked items is controlled by shift, unchanged); Ctrl+Shift+click toggles a locked part.

- [ ] **Step 1:** No unit test harness for Bevy picking here; verification is the manual check in Task 8. Implement.
- [ ] **Step 2:** `cargo build -p lab-viewport` → OK; `cargo test -p lab-viewport` → PASS.
- [ ] **Step 3:** Commit `Viewport: highlight extra selection, report Ctrl+click toggles`.

### Task 6: App wiring — selection, scene tree folders, actions (lab-app, lab-ui)

**Files:**
- Modify: `crates/lab-app/src/controller.rs` (`on_scene_row_clicked` ~L95, `select` ~L1048, `on_viewport_selection` ~L1064, action dispatch: lock ~L456, hide ~L513, copy/dup ~L639, nudge ~L686, `library.save_bundle` ~L451; handle `SelectionToggled` where viewport events are dispatched)
- Modify: `crates/lab-app/src/views.rs:24-35` (`SceneRowData`), `:233-333` (`scene_rows`)
- Modify: `crates/lab-app/src/ui_models.rs:105` (`scene_row`)
- Modify: `crates/lab-app/src/actions.rs` (register actions), `crates/lab-app/src/context_menu.rs:16-31`, `crates/lab-app/src/scene.rs` (row actions)
- Modify: `crates/lab-ui/ui/app.slint` (L400 callback signature, L1146 click, key handler ~L617), `crates/lab-ui/ui/types.slint` if `SceneRow` is declared there, `crates/lab-ui/examples/gallery.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces:
  - `callback scene-row-clicked(int /*kind*/, int /*id*/, bool /*ctrl*/, bool /*shift*/)`.
  - `SceneRowData.kind == 3` = group folder row, `id` = group id; `detail` = member count. `scene_rows(doc, selection, extra: &[NodeId], collapsed)` — new `extra` param; a group's folder row sits under the top parent (platform/room) at the position of its first member; its members' chain rows are indented beneath it and skipped at the platform level; collapse key `(3, gid)`; `selected` true for primary and extras; folder row `selected` when the selection is that whole group.
  - Clicking a folder row selects the whole group. Ctrl/Shift on node rows use `multi_select::toggle`/`range`; a plain click uses `multi_select::click`. All paths store `Sel` into `session.selection`/`session.extra` and send `ViewportRequest::Select(primary)` + `ViewportRequest::SelectExtra(extra)`.
  - Action ids: `edit.group` "Group" (Edit menu, Ctrl+G, keywords "group folder block"), `edit.ungroup` "Ungroup" (Ctrl+Shift+G). `edit.group` takes chain roots of `selected_nodes()`, dedups, executes `AddGroup` named `"Group {n}"` (lowest unused n ≥ 1), then selects the whole group; refusals (fewer than 2 roots, different platforms, already grouped) show the command error as a toast.
  - Context menu for a node adds "Group" when ≥ 2 nodes selected and "Ungroup" when the node is in a group. Folder rows: rename (existing `scene-row-renamed` with kind 3 → `RenameGroup`), row actions `"ungroup"`, `"delete"` (deletes all members via `remove_nodes_command`), and "Save as Bundle…".
  - Multi-aware actions over `selected_nodes()`: `selection.delete` (one `remove_nodes_command`), `selection.lock`/`unlock` (Batch of `SetLocked`), `view.hide`, `view.isolate_selection`, `edit.copy`/`cut`/`duplicate`, arrow-key nudge (pass all selected nodes to `nudge::nudge`). Everything else keeps using the primary.

- [ ] **Step 1: Write failing tests** in `views.rs` tests (next to `v3_scene_rows`)
  - `group_folder_row_holds_members`: platform with nodes 1, 2, 3; group {1,3} → rows: platform, folder(kind 3, detail "2"), 1, 3 (depth+1), then 2; node 1 and 3 not repeated.
  - `extras_marked_selected`: primary 1, extra [2] → both rows `selected`.
  - In `edit.rs` or `multi_select.rs`: `group_action_names_group_n` (existing "Group 1" → new is "Group 2").
- [ ] **Step 2:** `cargo test -p lab-app views multi_select` → FAIL.
- [ ] **Step 3:** Implement wiring and Slint changes.
- [ ] **Step 4:** `cargo test -p lab-app && cargo build -p lab-ui --examples` → PASS.
- [ ] **Step 5:** Commit `App: multi-select wiring, group folders in the scene tree, Group/Ungroup actions`.

### Task 7: Block move for the gizmo drag (lab-app)

**Files:**
- Create: `crates/lab-app/src/block_move.rs` (pure) + tests
- Modify: `crates/lab-app/src/transform.rs:19-74`

**Interfaces:**
- Consumes: `Session::selected_nodes()`, `Document::chain_root`, `compute_holes`, `LatticeIndex`.
- Produces:
  - `pub enum BlockMoveError { OffPlate, Locked }`
  - `pub fn block_move(doc: &Document, primary: NodeId, primary_cmd: Command, others: &[NodeId]) -> Result<Command, BlockMoveError>` — `primary_cmd` is the `SetAttach` transform.rs already builds. Delta = new primary position − current primary position (in the shared platform/room frame). If any member (primary included) is `Attach::Hole`: snap the delta to whole holes (`round(delta / pitch)`), use that snapped delta for the primary too, shift hole members' `LatticeIndex` by the step (refuse `OffPlate` if `!holes.contains`), add `step * pitch` mm to free members. Otherwise add the exact delta to every member. Any locked member → `Locked`. Returns `Batch([primary, others...])` of `SetAttach` (keeping each member's parent, attach kind, rotation). Rotation drags (`rotation != current`) apply to the primary only (spec: no block rotation in v1).
  - transform.rs: when `selected_nodes()` has more than one chain root, route through `block_move` (others = the other chain roots) and execute the Batch with the same `execute_coalescing(.., GESTURE)`; on `Err` show toast "Can't move: a part would leave the plate" / the existing locked message, and send `ApplyChanges` with the unchanged doc so the viewport snaps back.

- [ ] **Step 1: Write failing tests** in `block_move.rs`
  - `block_step_moves_hole_and_free_members_rigidly`: table with metric grid; member A on hole (10,10), member B Free at A+ (5,5) mm; primary A dragged by (+2 pitch, 0) → A at hole (12,10), B position + (2*pitch, 0).
  - `block_snaps_free_primary_when_any_member_on_hole`: primary Free dragged by 1.4 pitch, other on hole → step 1.
  - `block_step_off_plate_refuses_all`: other member on the last column, drag +1 → `Err(OffPlate)`.
  - `block_locked_member_refuses`.
  - `block_free_only_moves_exact_delta`.
- [ ] **Step 2:** `cargo test -p lab-app block_move` → FAIL.
- [ ] **Step 3:** Implement `block_move` and the transform.rs routing.
- [ ] **Step 4:** → PASS; `cargo test -p lab-app` → PASS.
- [ ] **Step 5:** Commit `Move a multi-selection or group as one rigid block`.

### Task 8: End-to-end check and docs

**Files:**
- Modify: `CHANGELOG.md` (Unreleased: multi-select, groups, bundles from selection)

- [ ] **Step 1:** `cargo test --workspace` → all PASS. `cargo clippy --workspace` → no new warnings.
- [ ] **Step 2:** Run the app (`cargo run -p lab-app --release` with APPDATA/LOCALAPPDATA pointed at a scratch folder) and check by hand, capturing screenshots:
  - Ctrl+click 3 parts in the viewport → all highlighted; inspector shows the primary.
  - Shift+click a range in the scene tree.
  - Ctrl+G → folder appears; click a member in 3D → whole group; click again → that part alone.
  - Arrow keys and gizmo drag move the group rigidly; one Ctrl+Z restores.
  - Build BE1→PH-20→TR20→KM100→BB1-E03, right-click BB1-E03 → Save as Bundle → drag bundle into a new project → all 5 parts.
  - Save group as bundle → placed back as a folder with the same name.
  - Open an older `.labsetup` → loads, no groups.
- [ ] **Step 3:** `graphify update .`
- [ ] **Step 4:** Commit `Changelog: multi-select, groups, bundles from a selection`.

---

**Deviation from spec (plan call):** "Double-click picks a part inside a group" is implemented as "click a member again while its whole group is selected" (`multi_select::click`). Same outcome without adding double-click plumbing to the Bevy viewport.
