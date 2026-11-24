# Multi-select, groups, and bundles from a selection

Date: 2026-11-23. Status: approved approaches (A + A), design awaiting approval.

## Why

The user builds constrained assemblies (e.g. BE1 base → PH-20 post holder → TR20 post → KM100
mount → BB1-E03 mirror) and wants to keep them together while laying out a setup and reuse
them in other projects from the library. Today:

- Selection is a single item (`Session::selection: Option<SelectionTarget>`).
- **Save as Bundle…** (POLA-203) exists but saves only `copy(doc, &[one id])`. `copy` uses
  `views::set_root`, which walks up through `Attach::Mate` only, so right-clicking BB1-E03
  (constrained, not mated, to KM100) saves BB1-E03 alone. Saving from BE1 works.
- There is no way to keep several parts together or move them as one.

## Goals

1. **Multi-select** in the viewport and the left panel (scene tree).
2. **Groups**: a named, saved set of parts that shows as a folder in the left panel and
   moves as one block.
3. **Save as Bundle** from a multi-selection or a group, saving all of it.

## Non-goals

- Editing several parts' fields at once in the inspector (it keeps showing one part).
- Nested groups, groups spanning platforms, grouping platforms or the room.
- Box/marquee selection in the viewport.
- Rotating a group as a block (move only in v1).

## Anti-goals (a failure even if it "works")

- Any change to how positions, mates, constraints or joints are solved. Groups are
  bookkeeping only.
- Old `.labsetup` files failing to open, or a group losing its members' real attachments.
- Undo leaving the document half-moved.

## Approaches considered

**Multi-select**
- **A (picked): main selection + extras.** Keep `selection: Option<SelectionTarget>` as the
  *primary* item; add `extra: Vec<NodeId>` (ordered, no duplicates, never containing the
  primary). The ~400 single-item uses keep working on the primary. Only multi-aware actions
  read the extras. Cheapest and lowest risk.
- B: a full `Vec<SelectionTarget>` everywhere. Rewrites every use, including panels that can
  only edit one item. Rejected: big risk, no visible gain.

**Groups**
- **A (picked): a list in the document.** `Document.groups: Vec<Group { id, name, members:
  Vec<NodeId> }>`. Members keep their real `parent`/`attach`. No solver changes.
- B: a group is a real parent in the tree (`Parent::Group`). Touches every pose, mate,
  constraint and validator. Rejected: invasive and fragile.

## Design

### Data (lab-core)

- `Group { id: GroupId, name: String, members: Vec<NodeId> }`. *(Call:)* `GroupId(u64)` comes
  from the same `next_id` counter as nodes/platforms, so it stays under `MAX_ID`. The
  load-time id bump must also scan groups, and `check_parents`/validation ignores group ids.
- `Document.groups: Vec<Group>` with `#[serde(default)]`, so older files load unchanged.
  *(Call:)* **No `SCHEMA_VERSION` bump.** Precedent: M8 constraints and M9 joints were added at
  schema 3 without a bump. Accepted cost: an older app opening a newer file ignores and drops
  the groups when it saves. The parts and their attachments are untouched.
- **Members are set roots**: the top of a chain, found by walking up through *any*
  node-to-node attachment (Mate, Constrained, Joint), i.e. until the parent is a platform or
  the room. Call this the *chain root*. Grouping any part of a chain adds the chain root; the
  whole chain comes along as its descendants.
- A node is in at most one group. All members share the same top-level parent (the same
  platform, or the room). Grouping across platforms is refused with a toast.
- Commands (undoable, through `Command`): `AddGroup`, `RemoveGroup` (ungroup; parts stay),
  `SetGroupMembers`, `RenameGroup`.
- *(Call:)* Deleting is a `Command::Batch` of `RemoveNodes` + `SetGroupMembers`/`RemoveGroup`
  for any affected group, so undo restores the membership through the batch inverse.
  `RemoveNodes`/`RestoreNodes` stay as they are. Only deleting a *member* (a chain root)
  changes a group. Deleting a part lower in a chain leaves the group alone. A group left with
  fewer than 2 members is removed in the same batch.

### Chain root

Add one helper (e.g. `chain_root(doc, id)`) that walks up all `Parent::Node` links. Use it for
grouping and for bundles. Do **not** change `views::set_root`: the inspector's set rows rely on
its mate-only meaning.

### Selection (lab-app)

- `Session` gains `extra: Vec<NodeId>`. Any code path that sets `selection` to a new item
  without a modifier clears `extra`. Selection stays non-undoable UI state (M1-SPEC D9).
- **Viewport**: plain click = select (as today). Ctrl+click on a node = toggle it in the
  selection (if the primary is toggled off, the last extra becomes the primary).
  *(Call:)* Shift+click in the viewport keeps its current meaning (it picks `select_lock`
  parts). Ctrl+Shift+click toggles a locked part. Ctrl as "1 mm" applies only while dragging,
  so it does not clash with Ctrl+click.
- **Left panel**: Ctrl+click toggles; Shift+click selects the visible node rows between the
  primary and the clicked row. Platforms and the room stay single-select (a click on one
  clears extras).
- **Groups in selection**: a plain click on any part of a group selects the whole group
  (primary = the clicked part's chain root, extras = the other members). Double-click (viewport)
  or clicking the part's own row inside the folder (left panel) selects just that part.
- Every selected node is highlighted with the existing selection outline. The gizmo and
  inspector show the primary.
- Escape / click on empty space clears all.

### Actions that act on the whole selection

| Action | With several parts / a group |
|---|---|
| Delete | removes all selected (one undo step, `RemoveNodes`) |
| Hide / Isolate | applies to all |
| Lock / Unlock | applies to all |
| Move (gizmo drag, nudge keys) | moves all chain roots by the same delta, one `Batch` step |
| Duplicate / Copy / Paste | all chain roots (`copy` already takes a slice) |
| **Group** (new, Ctrl+G, right-click, Edit menu) | creates a group from the chain roots of the selection, default name "Group N" |
| **Ungroup** (new, Ctrl+Shift+G) | removes the group, parts stay where they are |
| **Save as Bundle…** | saves `bundle_nodes(copy(doc, chain roots of all selected))`; default name = group name if a group is selected, else the primary's name |
| Joint, Constrain, Rotate 90°, inspector edits | primary only (unchanged) |

### Moving as a block

- Moving a chain root already carries its whole chain: child positions are stored in the
  parent's frame, and constrained/joint children are posed relative to it. No solver change.
- The move delta is computed for the primary as today. Each other member's chain root gets
  the same delta. *(Call:)* The block always stays rigid:
  - If **any** member is on a hole, the block's delta is snapped to whole holes (the existing
    arrow-key nudge is already hole steps). Hole members shift their hole index by that step,
    and free members move by the same distance in mm.
  - If no member is on a hole, every member moves by the exact delta.
  - If any target hole is off the plate, or any member is locked, the whole move is refused
    with a toast. Nothing moves partly.
  - All members share one platform (see Data), so there is never a platform-vs-room mix.
- The whole move is one `Command::Batch`, so a single undo restores everything.
- Locked members: if any member is locked, the move is refused with the existing locked
  message.

### Left panel (scene tree)

- A group shows as a **folder row** under its platform (or the room), placed where its first
  member would have been, labelled with the group name and member count. It collapses like
  other rows (`scene_collapsed` gets a key kind for groups).
- The members' normal chain rows appear indented inside the folder and not again at the
  platform level.
- Double-click the folder name to rename. Right-click: Rename, Ungroup, Save as Bundle…,
  Delete (deletes the group's parts, with the usual undo).
- `SceneRowData.kind` gains a value for group rows. `selected` is true for every row in the
  selection (primary and extras).

### Bundles

- Saving uses the chain roots of the whole selection, deduplicated, in selection order
  (primary first, so it becomes the bundle's anchor).
- Fix in `clipboard::copy`: never push a node twice. Today it dedups only on the mate-only
  `set_root`, so a Constrained/Joint child listed before its root emits its subtree twice.
  Switch `copy` to `chain_root` plus a seen-set.
- A bundle saved from a group is placed back as a group with the same name (store an optional
  `group: Option<String>` on `lab_file::Bundle`, `#[serde(default)]`). *(Call:)* `place.rs`
  wraps the `PlaceAssembly` and an `AddGroup` (members = the placed roots) in one
  `Command::Batch`, so placing is one undo step.
- Placing a bundle is unchanged otherwise (`place_bundle`).

## Left to the builder

- Exact Slint wiring for modifier keys in the scene tree and viewport picking.
- Where `chain_root` lives (lab-core next to `descendants` is the natural spot).
- Folder icon and row styling (reuse existing scene row styles).
- Live-control/agent API (`crates/lab-app/src/live/`): expose group commands only if it falls
  out naturally; not required.

## Tests

- lab-core: group commands are undoable and bit-identical after undo; deleting a member updates
  the group; old files without `groups` load.
- `chain_root` walks through Mate, Constrained and Joint links.
- `copy` emits no duplicates when a child is listed before its root.
- Block move: hole members shift by the same hole delta; off-plate refuses the whole move;
  one undo restores all.
- Bundle from a 5-part constrained chain selected by its *bottom* part saves all 5.
