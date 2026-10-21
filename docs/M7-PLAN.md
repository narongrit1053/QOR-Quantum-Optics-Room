# M7 plan: Quality of life

New milestone after M6 (board request on POLA-94, 2026-10-22: "add that to the plan", meaning the five recommended items). Built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus the one check in its section), other flaws go to the Defect log (POLA-74, one line each), at most 2 attempts at any failure. Read this top part and your own section; open only the files it names. Starts when M6 (POLA-87) is done; branch from the `draft` that has M6 merged.

**Exit criterion (stage 2 walks it once):** open the demo, select a mirror set, Ctrl+D twice and Ctrl+C / Ctrl+V once: three new sets appear one hole apart, each one undo step. Arrow keys move the selected set one hole per press (Shift: 5), Q/E turn it 15°. Rename a part "M3", give it a note; the Top view shows "M3"; the BOM CSV has the label in its Labels column. Save, close, restart: the project reopens by itself and appears under Open Recent. `?` shows the shortcut sheet with every key above.

## What already exists (do not rebuild)

- **Actions:** `lab-app/src/actions.rs` `ACTIONS` (id, title, group, keywords, shortcut) feeds the palette, which already shows `shortcut`. Dispatch: `lab-app/src/controller.rs` `match id { "view.beams" => … }` (~line 349). Global keys: `lab-ui/ui/app.slint` `shortcuts := FocusScope { capture-key-pressed … }` (~line 418); follow its rules (skip when `root.modal`, when a field is focused / `EditState.field-dirty`).
- **Commands** (`lab-core/src/command.rs`): `PlaceAssembly { nodes }` adds a list of nodes (parents earlier in the list) as one undo step; `SetAttach { id, parent, attach, position, rotation }`; `SetPlacement`; `Rename { id, name }`; `SetLocked`. `Attach::{Free, Hole { hole: LatticeIndex { i, j } }, Mate { … }}`. Mated children are posed relative to their parent, so moving a set's root moves the set.
- **Node** (`lab-core/src/model.rs:403`) already has `name` and `locked`. Top-view labels come from node names (`lab-app/src/overlay.rs`). BOM: `lab-app/src/bom.rs` `rows` / `to_csv`, header `Vendor,Part number,Name,Spec,Quantity`.
- **Settings** (`lab-file/src/settings.rs`) is serde with `#[serde(default)]` fields; adding a field needs no migration.

## Decisions

- **Paste lands one hole pitch in +X of the original** (platform frame), not under the cursor: the viewport does not report a hovered hole to the app, and arrow nudge (B) makes the final move one key press. Cursor placement is a later item.
- **The clipboard is in-app** (a `Vec<Node>` in `AppState`), not the Windows clipboard: no format to define, no cross-instance paste needed.
- **Nudge and duplicate act on set roots:** a selected child of a mated set (mount, post) acts on its top-most ancestor that is not a platform. Locked nodes are skipped with the existing locked toast.
- **Reopen last project on start** is on by default (`Settings.reopen_last = true`), only when there is no command-line path and no autosave to restore; a palette action toggles it.
- No schema bump: `Node.note` is `#[serde(default, skip_serializing_if = "String::is_empty")]`.

## Shared files (merge rule)

A, B, C and D each **append** rows to `ACTIONS`, arms to the controller's action `match`, and (B, C) branches to the `capture-key-pressed` handler. Add your lines at the end of the relevant group/block and touch nothing else there. Engineer Sonnet resolves these conflicts at merge by keeping both sides (`Integration fix:` if needed).

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 1 | A | Duplicate and copy/paste | Engineer Opus Low | `lab-app/src/clipboard.rs` (new), its `mod` line, `AppState.clipboard`, action rows + match arms, Ctrl+D/C/V/X in `app.slint` shortcuts |
| 1 | B | Keyboard nudge and rotate | Engineer Opus Low | `lab-app/src/nudge.rs` (new), its `mod` line, action rows + match arms, arrow/Q/E keys in `app.slint` shortcuts |
| 1 | C | Recent files, reopen last, shortcut sheet | Engineer Sonnet | `lab-file/src/settings.rs` (two fields), `lab-app/src/files.rs` (record + reopen), `ProjectMenu` in `lab-ui/ui/overlays.slint`, `lab-ui/ui/shortcuts.slint` (new), its import/instance in `app.slint`, `?` key |
| 1 | D | Part notes and BOM labels | Engineer Opus Low | `Node.note` + `Command::SetNote` in `lab-core`, note field in `lab-ui/ui/inspector.slint` + `lab-app/src/edit.rs` / `ui_models.rs`, `lab-app/src/bom.rs` |
| 2 | Merge | Merge A–D + walkthrough | Engineer Sonnet | merge commits, `Integration fix:` commits, evidence |

Stage-1 tasks own disjoint files apart from the shared rows above.

## A. Duplicate and copy/paste (Engineer Opus Low)

```rust
// lab-app/src/clipboard.rs
/// The selected set roots and all their descendants, in document order (parents first).
pub fn copy(doc: &Document, selection: &[NodeId]) -> Vec<Node>;
/// Fresh ids (`doc` allocator), parents remapped inside the list, roots moved one hole pitch
/// in +X: `Attach::Hole { i + 1, j }` (position recomputed the way place.rs does it), `Free`
/// roots +pitch (25 mm if the parent has no grid) in x. Returns the command and the new root ids.
pub fn paste(doc: &mut Document, nodes: &[Node]) -> Option<(Command, Vec<NodeId>)>;  // Command::PlaceAssembly
```
- Actions: `edit.copy` "Copy" Ctrl+C, `edit.cut` "Cut" Ctrl+X (copy, then the existing delete), `edit.paste` "Paste" Ctrl+V, `edit.duplicate` "Duplicate" Ctrl+D (copy + paste without touching the clipboard). Each paste is one undo step labelled "Paste" / "Duplicate" and selects the new roots, so repeated Ctrl+D walks along the row.
- Paste into an empty document, or with nothing copied: no-op. Nodes whose platform no longer exists are dropped.
- Check: one unit test in `clipboard.rs`: copy a 5-node set, paste, assert 5 new nodes with new ids, the root at `i + 1`, children's parents remapped.

## B. Keyboard nudge and rotate (Engineer Opus Low)

```rust
// lab-app/src/nudge.rs
pub enum Nudge { Step { dx: i32, dy: i32 }, Fine { dx: f64, dy: f64 }, Turn { degrees: f64 } }
/// One SetAttach / SetPlacement per selected root, grouped as one undo step labelled "Nudge" / "Rotate".
pub fn nudge(doc: &Document, selection: &[NodeId], how: Nudge) -> Option<Command>;
```
- Keys (only when the viewport has focus, no field is focused, nothing modal): arrows = 1 hole (`Hole { i + dx, j + dy }`, directions in the platform frame as seen from the Top view), Shift+arrows = 5 holes, Alt+arrows = 1 mm (`Fine`; the node becomes `Attach::Free` at that position), Q / E = −15° / +15° about the vertical axis (for a mated root, its `twist`; otherwise `rotation`).
- A target hole outside the grid or in an exclusion zone: stay put and show a toast "No hole there". If `lab-core` has no ready check, use the platform's hole set the same way `place.rs` snaps.
- Consecutive presses within 600 ms on the same selection coalesce into one undo step if the history already supports coalescing; otherwise one step per press (fine).
- Actions: `edit.nudge_*` rows are not needed; add `tool.rotate_left` "Rotate −15°" Q and `tool.rotate_right` "Rotate +15°" E to `ACTIONS` so they show in the palette and the sheet.
- Check: one unit test: a Hole-attached root nudged by (1, 0) gives `i + 1`; a mated child in the selection moves its root.

## C. Recent files, reopen last, shortcut sheet (Engineer Sonnet)

- `Settings`: `recent_files: Vec<PathBuf>` (`#[serde(default)]`, newest first, max 10, deduplicated, missing files dropped when the menu is built) and `reopen_last: bool` (default true, like `show_beams`).
- `files.rs`: push the path after every successful open, save, save-as and save-as-folder. On start (`initial_session`), when there is no command-line path and no autosave session is offered, open `recent_files[0]` if it exists; if opening fails, start empty and show the usual error toast.
- `ProjectMenu`: an "Open Recent" section listing file names (full path as tooltip), click opens it through the existing open path (dirty-document prompt included). Action `file.reopen_last` "Reopen Last Project on Start" toggles the setting (toast "On"/"Off").
- `shortcuts.slint`: a sheet like the palette's style listing, by group, every `ACTIONS` row with a non-empty shortcut, plus a fixed "Mouse" block: left-drag orbit, right/middle/Shift-drag pan, wheel zoom, Shift = free placement, Ctrl = fine. Opened by `?` (Shift+/) and action `help.shortcuts` "Keyboard Shortcuts" `?`; Esc closes. The rows come from Rust (`ACTIONS`), so A, B and D's new shortcuts appear without editing the sheet.
- Check: start the app twice; the second start reopens the project saved in the first.

## D. Part notes and BOM labels (Engineer Opus Low)

- `Node.note: String` (serde rule above). `Command::SetNote { id, note }` with its inverse, label "Edit Note". Rename already exists for labels.
- Inspector: a multi-line "Note" field under the name for a selected part (commit on focus loss, like the other fields).
- BOM: add a `Labels` column after `Name`: the node names of that row's parts that differ from the part's default name, sorted, joined with `, ` (empty if none). Header becomes `Vendor,Part number,Name,Labels,Spec,Quantity`; update the existing `to_csv` test's expected header.
- Check: `cargo test -p lab-core --offline` for SetNote's undo round trip (one new test), and the updated BOM test.

## Stage 2: Merge and walkthrough (Engineer Sonnet)

Merge `draft-<A>`, `draft-<B>`, `draft-<C>`, `draft-<D>` into `draft` (`--no-ff`), resolving the shared-row conflicts by keeping both sides. Run `cargo build --workspace --offline` and `cargo test -p lab-core -p lab-app --offline` once. Walk the exit criterion once in the app and report each step as met or not; anything not met goes to the Defect log, not into a fix loop.
